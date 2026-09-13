use comsat_store::{Delivery, RecordObservation, StoreError, StoreResult, Watch, WatchRun};
use comsat_types::{Record, RecordId, SourceId};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct WatchRow {
    pub tenant_id: String,
    pub watch_id: String,
    query_json: String,
    source_ids_json: String,
    interval_seconds: u64,
    cursor_json: String,
    enabled: i64,
    created_at_epoch_seconds: i64,
    next_due_epoch_seconds: i64,
    lease_owner: Option<String>,
    lease_expires_epoch_seconds: Option<i64>,
}

impl WatchRow {
    pub fn into_watch(self) -> StoreResult<Watch> {
        Ok(Watch {
            tenant_id: self.tenant_id,
            watch_id: self.watch_id,
            query: serde_json::from_str(&self.query_json).map_err(|error| json_error(&error))?,
            source_ids: serde_json::from_str(&self.source_ids_json)
                .map_err(|error| json_error(&error))?,
            interval_seconds: self.interval_seconds,
            cursor: serde_json::from_str(&self.cursor_json).map_err(|error| json_error(&error))?,
            enabled: self.enabled != 0,
            created_at_epoch_seconds: self.created_at_epoch_seconds,
            next_due_epoch_seconds: self.next_due_epoch_seconds,
            lease_owner: self.lease_owner,
            lease_expires_epoch_seconds: self.lease_expires_epoch_seconds,
        })
    }
}

#[derive(Deserialize)]
pub struct WatchRunRow {
    tenant_id: String,
    watch_id: String,
    run_id: String,
    idempotency_key: String,
    lease_owner: String,
    status: String,
    started_at_epoch_seconds: i64,
    finished_at_epoch_seconds: Option<i64>,
    error: Option<String>,
}

impl WatchRunRow {
    pub fn into_run(self) -> WatchRun {
        WatchRun {
            tenant_id: self.tenant_id,
            watch_id: self.watch_id,
            run_id: self.run_id,
            idempotency_key: self.idempotency_key,
            lease_owner: self.lease_owner,
            status: self.status,
            started_at_epoch_seconds: self.started_at_epoch_seconds,
            finished_at_epoch_seconds: self.finished_at_epoch_seconds,
            error: self.error,
        }
    }
}

#[derive(Deserialize)]
pub struct CompletionGateRow {
    pub interval_seconds: i64,
}

#[derive(Deserialize)]
pub struct HistoryRow {
    source_id: String,
    record_id: String,
    kind: String,
    url: String,
    title: Option<String>,
    text: Option<String>,
    author: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    metadata_json: String,
    watch_id: Option<String>,
    first_observed_epoch_seconds: i64,
    last_observed_epoch_seconds: i64,
}

impl HistoryRow {
    pub fn into_observation(self) -> StoreResult<RecordObservation> {
        let metadata = serde_json::from_str::<Value>(&self.metadata_json)
            .map_err(|error| json_error(&error))?;
        let record = record_from_row(&self, metadata)?;
        Ok(RecordObservation {
            record,
            watch_id: self.watch_id,
            first_observed_epoch_seconds: self.first_observed_epoch_seconds,
            last_observed_epoch_seconds: self.last_observed_epoch_seconds,
        })
    }
}

#[derive(Deserialize)]
pub struct DeliveryRow {
    tenant_id: String,
    delivery_id: String,
    watch_id: String,
    run_id: Option<String>,
    status: String,
    target_json: String,
    payload_json: String,
    created_at_epoch_seconds: i64,
    next_attempt_epoch_seconds: i64,
    attempts: u32,
    lease_owner: Option<String>,
    lease_expires_epoch_seconds: Option<i64>,
    delivered_at_epoch_seconds: Option<i64>,
    error: Option<String>,
}

impl DeliveryRow {
    pub fn into_delivery(self) -> StoreResult<Delivery> {
        Ok(Delivery {
            tenant_id: self.tenant_id,
            delivery_id: self.delivery_id,
            watch_id: self.watch_id,
            run_id: self.run_id,
            status: self.status,
            target: serde_json::from_str(&self.target_json).map_err(|error| json_error(&error))?,
            payload: serde_json::from_str(&self.payload_json)
                .map_err(|error| json_error(&error))?,
            created_at_epoch_seconds: self.created_at_epoch_seconds,
            next_attempt_epoch_seconds: self.next_attempt_epoch_seconds,
            attempts: self.attempts,
            lease_owner: self.lease_owner,
            lease_expires_epoch_seconds: self.lease_expires_epoch_seconds,
            delivered_at_epoch_seconds: self.delivered_at_epoch_seconds,
            error: self.error,
        })
    }
}

fn record_from_row(row: &HistoryRow, metadata: Value) -> StoreResult<Record> {
    Ok(Record {
        id: RecordId::new(row.record_id.clone())?,
        source: SourceId::new(row.source_id.clone())?,
        kind: row.kind.clone(),
        url: row.url.clone(),
        title: row.title.clone(),
        text: row.text.clone(),
        author: row.author.clone(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        metadata,
    })
}

fn json_error(error: &serde_json::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}
