use async_trait::async_trait;
use comsat_store::{
    ClaimDueWatch, ClaimPendingDelivery, ClaimedWatch, CompleteDelivery, CompleteWatchRun,
    CreateWatch, DeleteWatch, Delivery, HistoryRequest, ListDeliveries, RecordObservation,
    StartWatchRun, Store, StoreError, StoreResult, UpsertSource, Watch, WatchRun, WatchRunOutcome,
    validate_tenant_id,
};
use serde::Deserialize;
use worker::{
    D1Database, D1PreparedStatement, D1Result,
    send::{IntoSendFuture, SendWrapper},
    wasm_bindgen::JsValue,
};

use crate::d1_rows::{CompletionGateRow, DeliveryRow, HistoryRow, WatchRow, WatchRunRow};
use d1_sql::{
    CLAIM_DELIVERY_SQL, CLAIM_DUE_WATCH_SQL, COMPLETE_DELIVERY_SQL, COMPLETE_GATE_SQL,
    COMPLETE_RUN_SQL, COMPLETE_WATCH_SQL, DELETE_WATCH_SQL, HISTORY_TENANT_SQL, HISTORY_WATCH_SQL,
    INSERT_RECORDS_SQL, INSERT_RUN_DELIVERY_SQL, INSERT_WATCH_RECORDS_SQL, INSERT_WATCH_SQL,
    LIST_DELIVERIES_SQL, LIST_DELIVERIES_STATUS_SQL, LIST_WATCHES_SQL, LOAD_RUN_SQL, START_RUN_SQL,
    UPDATE_RECORDS_SQL, UPDATE_WATCH_RECORDS_SQL, UPSERT_SOURCE_SQL,
};
use d1_values::{
    bool_value, bulk_record_values, claim_delivery_values, complete_delivery_values,
    complete_run_values, complete_watch_values, completion_gate_values, i64_from_u64, int,
    records_json, run_delivery_values, status_for, text, watch_values,
};

#[path = "d1_sql.rs"]
mod d1_sql;
#[path = "d1_values.rs"]
mod d1_values;

pub struct D1Store {
    db: SendWrapper<D1Database>,
}

impl D1Store {
    pub fn new(db: D1Database) -> Self {
        Self {
            db: SendWrapper::new(db),
        }
    }

    async fn run(&self, sql: &str, values: Vec<JsValue>) -> StoreResult<D1Result> {
        self.statement(sql, &values)?
            .run()
            .into_send()
            .await
            .map_err(backend)
    }

    async fn all<T>(&self, sql: &str, values: Vec<JsValue>) -> StoreResult<Vec<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let result = self
            .statement(sql, &values)?
            .all()
            .into_send()
            .await
            .map_err(backend)?;
        rows(&result)
    }

    async fn first<T>(&self, sql: &str, values: Vec<JsValue>) -> StoreResult<Option<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.statement(sql, &values)?
            .first(None)
            .into_send()
            .await
            .map_err(backend)
    }

    async fn batch(&self, statements: Vec<D1PreparedStatement>) -> StoreResult<Vec<D1Result>> {
        self.db.batch(statements).into_send().await.map_err(backend)
    }

    fn statement(&self, sql: &str, values: &[JsValue]) -> StoreResult<D1PreparedStatement> {
        self.db.prepare(sql).bind(values).map_err(backend)
    }
}

#[async_trait]
impl Store for D1Store {
    async fn upsert_source(&self, request: UpsertSource) -> StoreResult<()> {
        request.validate()?;
        self.run(
            UPSERT_SOURCE_SQL,
            vec![
                text(&request.tenant_id),
                text(request.source_id.as_str()),
                bool_value(request.enabled),
                int(request.observed_at_epoch_seconds),
            ],
        )
        .await?;
        Ok(())
    }

    async fn create_watch(&self, request: CreateWatch) -> StoreResult<Watch> {
        request.validate()?;
        let watch = request.into_watch();
        self.run(INSERT_WATCH_SQL, watch_values(&watch)?).await?;
        Ok(watch)
    }

    async fn list_watches(&self, tenant_id: &str) -> StoreResult<Vec<Watch>> {
        validate_tenant_id(tenant_id)?;
        let rows = self
            .all::<WatchRow>(LIST_WATCHES_SQL, vec![text(tenant_id)])
            .await?;
        rows.into_iter().map(WatchRow::into_watch).collect()
    }

    async fn delete_watch(&self, request: DeleteWatch) -> StoreResult<bool> {
        request.validate()?;
        let result = self
            .run(
                DELETE_WATCH_SQL,
                vec![text(&request.tenant_id), text(&request.watch_id)],
            )
            .await?;
        Ok(changes(&result)? > 0)
    }

    async fn claim_due_watch(&self, request: ClaimDueWatch) -> StoreResult<Option<ClaimedWatch>> {
        request.validate()?;
        let lease_seconds = i64_from_u64(request.lease_seconds, "lease_seconds")?;
        let lease_expires = request.now_epoch_seconds + lease_seconds;
        let row = self
            .first::<WatchRow>(
                CLAIM_DUE_WATCH_SQL,
                vec![
                    text(&request.lease_owner),
                    int(lease_expires),
                    text(&request.tenant_id),
                    int(request.now_epoch_seconds),
                ],
            )
            .await?;
        row.map(|row| claimed_watch(row, request.lease_owner, lease_expires))
            .transpose()
    }

    async fn start_watch_run(&self, request: StartWatchRun) -> StoreResult<WatchRun> {
        request.validate()?;
        self.run(
            START_RUN_SQL,
            vec![
                text(&request.tenant_id),
                text(&request.watch_id),
                text(&request.run_id),
                text(&request.idempotency_key),
                text(&request.lease_owner),
                int(request.started_at_epoch_seconds),
            ],
        )
        .await?;
        load_run(self, &request).await
    }

    async fn complete_watch_run(&self, request: CompleteWatchRun) -> StoreResult<WatchRunOutcome> {
        request.validate()?;
        let records_json = records_json(&request)?;
        let mut statements =
            vec![self.statement(COMPLETE_GATE_SQL, &completion_gate_values(&request))?];
        statements.extend(record_statements(self, &request, &records_json)?);
        statements.extend(finish_statements(self, &request)?);
        let results = self.batch(statements).await?;
        complete_outcome(request, &results)
    }

    async fn history(&self, request: HistoryRequest) -> StoreResult<Vec<RecordObservation>> {
        request.validate()?;
        let rows = if request.watch_id.is_some() {
            history_for_watch(self, &request).await?
        } else {
            history_for_tenant(self, &request).await?
        };
        rows.into_iter().map(HistoryRow::into_observation).collect()
    }

    async fn claim_pending_delivery(
        &self,
        request: ClaimPendingDelivery,
    ) -> StoreResult<Option<Delivery>> {
        request.validate()?;
        let lease_seconds = i64_from_u64(request.lease_seconds, "lease_seconds")?;
        let lease_expires = request.now_epoch_seconds + lease_seconds;
        self.first::<DeliveryRow>(
            CLAIM_DELIVERY_SQL,
            claim_delivery_values(&request, lease_expires),
        )
        .await?
        .map(DeliveryRow::into_delivery)
        .transpose()
    }

    async fn complete_delivery(&self, request: CompleteDelivery) -> StoreResult<Delivery> {
        request.validate()?;
        self.first::<DeliveryRow>(COMPLETE_DELIVERY_SQL, complete_delivery_values(&request))
            .await?
            .map(DeliveryRow::into_delivery)
            .transpose()?
            .ok_or_else(|| StoreError::Conflict("delivery is not leased by this worker".into()))
    }

    async fn list_deliveries(&self, request: ListDeliveries) -> StoreResult<Vec<Delivery>> {
        request.validate()?;
        let rows = if let Some(status) = &request.status {
            self.all::<DeliveryRow>(
                LIST_DELIVERIES_STATUS_SQL,
                vec![
                    text(&request.tenant_id),
                    text(status),
                    int(i64::from(request.limit)),
                ],
            )
            .await?
        } else {
            self.all::<DeliveryRow>(
                LIST_DELIVERIES_SQL,
                vec![text(&request.tenant_id), int(i64::from(request.limit))],
            )
            .await?
        };
        rows.into_iter().map(DeliveryRow::into_delivery).collect()
    }
}

async fn load_run(store: &D1Store, request: &StartWatchRun) -> StoreResult<WatchRun> {
    store
        .first::<WatchRunRow>(
            LOAD_RUN_SQL,
            vec![
                text(&request.tenant_id),
                text(&request.watch_id),
                text(&request.idempotency_key),
            ],
        )
        .await?
        .map(WatchRunRow::into_run)
        .ok_or_else(|| StoreError::Conflict("watch is not currently claimed by this worker".into()))
}

fn record_statements(
    store: &D1Store,
    request: &CompleteWatchRun,
    records_json: &str,
) -> StoreResult<Vec<D1PreparedStatement>> {
    let values = bulk_record_values(request, records_json);
    let mut statements = vec![
        store.statement(INSERT_RECORDS_SQL, &values)?,
        store.statement(UPDATE_RECORDS_SQL, &values)?,
    ];
    if let Some(values) = run_delivery_values(request, records_json)? {
        statements.push(store.statement(INSERT_RUN_DELIVERY_SQL, &values)?);
    }
    statements.extend([
        store.statement(INSERT_WATCH_RECORDS_SQL, &values)?,
        store.statement(UPDATE_WATCH_RECORDS_SQL, &values)?,
    ]);
    Ok(statements)
}

fn finish_statements(
    store: &D1Store,
    request: &CompleteWatchRun,
) -> StoreResult<Vec<D1PreparedStatement>> {
    Ok(vec![
        store.statement(COMPLETE_RUN_SQL, &complete_run_values(request))?,
        store.statement(COMPLETE_WATCH_SQL, &complete_watch_values(request)?)?,
    ])
}

fn complete_outcome(
    request: CompleteWatchRun,
    results: &[D1Result],
) -> StoreResult<WatchRunOutcome> {
    ensure_completion_gate(results.first(), &request)?;
    let status = status_for(request.error.as_ref());
    let records_inserted = changes(
        results
            .get(1)
            .ok_or_else(|| StoreError::Backend("D1 completion batch was truncated".into()))?,
    )?;
    let delivery_offset = usize::from(request.delivery.is_some());
    let watch_insert_index = 3 + delivery_offset;
    let run_index = 5 + delivery_offset;
    let watch_index = 6 + delivery_offset;
    let watch_records_inserted = changes(
        results
            .get(watch_insert_index)
            .ok_or_else(|| StoreError::Backend("D1 completion batch was truncated".into()))?,
    )?;
    ensure_changed(
        results.get(run_index),
        "watch run is not running under this lease",
    )?;
    ensure_changed(
        results.get(watch_index),
        "watch lease is not held by this worker",
    )?;
    Ok(WatchRunOutcome {
        run_id: request.run_id,
        status: status.into(),
        records_seen: request.records.len(),
        records_inserted,
        watch_records_inserted,
    })
}

fn ensure_completion_gate(
    result: Option<&D1Result>,
    request: &CompleteWatchRun,
) -> StoreResult<()> {
    let Some(result) = result else {
        return Err(StoreError::Backend(
            "D1 completion batch returned no gate".into(),
        ));
    };
    let gate = rows::<CompletionGateRow>(result)?;
    if matches!(gate.as_slice(), [row] if row.interval_seconds > 0) {
        return Ok(());
    }
    let message = if request.error.is_some() {
        "watch run is not running under this lease"
    } else {
        "watch lease is not held by this worker"
    };
    Err(StoreError::Conflict(message.into()))
}

fn ensure_changed(result: Option<&D1Result>, message: &str) -> StoreResult<()> {
    let Some(result) = result else {
        return Err(StoreError::Backend(
            "D1 completion batch was truncated".into(),
        ));
    };
    if changes(result)? == 1 {
        return Ok(());
    }
    Err(StoreError::Conflict(message.into()))
}

async fn history_for_tenant(
    store: &D1Store,
    request: &HistoryRequest,
) -> StoreResult<Vec<HistoryRow>> {
    store
        .all::<HistoryRow>(
            HISTORY_TENANT_SQL,
            vec![
                text(&request.tenant_id),
                int(request.since_epoch_seconds.unwrap_or(i64::MIN)),
                int(i64::from(request.limit)),
            ],
        )
        .await
}

async fn history_for_watch(
    store: &D1Store,
    request: &HistoryRequest,
) -> StoreResult<Vec<HistoryRow>> {
    let watch_id = request.watch_id.as_deref().unwrap_or_default();
    store
        .all::<HistoryRow>(
            HISTORY_WATCH_SQL,
            vec![
                text(&request.tenant_id),
                text(watch_id),
                int(request.since_epoch_seconds.unwrap_or(i64::MIN)),
                int(i64::from(request.limit)),
            ],
        )
        .await
}

fn claimed_watch(
    row: WatchRow,
    lease_owner: String,
    lease_expires_epoch_seconds: i64,
) -> StoreResult<ClaimedWatch> {
    Ok(ClaimedWatch {
        watch: row.into_watch()?,
        lease_owner,
        lease_expires_epoch_seconds,
    })
}

fn rows<T>(result: &D1Result) -> StoreResult<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    ensure_success(result)?;
    result.results().map_err(backend)
}

fn changes(result: &D1Result) -> StoreResult<usize> {
    ensure_success(result)?;
    Ok(result
        .meta()
        .map_err(backend)?
        .and_then(|meta| meta.changes)
        .unwrap_or_default())
}

fn ensure_success(result: &D1Result) -> StoreResult<()> {
    if result.success() {
        return Ok(());
    }
    Err(StoreError::Backend(
        result
            .error()
            .unwrap_or_else(|| "D1 statement failed".into()),
    ))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "worker futures pass backend errors by value through map_err."
)]
fn backend(error: worker::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}
