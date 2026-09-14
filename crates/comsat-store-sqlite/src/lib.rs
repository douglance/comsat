//! SQLite implementation of the COMSAT persistence contracts.
#![forbid(unsafe_code)]

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

mod codec;
#[cfg(feature = "codemode")]
mod codemode;
mod deliveries;
mod migrations;
mod ops;
mod sql;

use async_trait::async_trait;
use codec::{
    bool_int, collect_rows, i64_from_u64, json_text, map_sql, source_ids_text, watch_from_row,
};
use comsat_store::{
    ClaimDueWatch, ClaimPendingDelivery, ClaimedWatch, CompleteDelivery, CompleteWatchRun,
    CreateWatch, DeleteWatch, Delivery, HistoryRequest, ListDeliveries, RecordObservation,
    StartWatchRun, Store, StoreError, StoreResult, UpsertSource, Watch, WatchRun, WatchRunOutcome,
    validate_tenant_id,
};
use deliveries::{claim_pending_delivery, complete_delivery, insert_run_delivery, list_deliveries};
use ops::{
    ensure_running_run, fetch_watch_run, finish_run_and_watch, history_for_tenant,
    history_for_watch, select_due_watch, upsert_record, upsert_watch_record,
    watch_interval_under_lease,
};
use rusqlite::{Connection, params};
use sql::{CLAIM_WATCH_SQL, START_RUN_SQL, WATCH_SELECT_SQL};

#[cfg(feature = "codemode")]
pub use codemode::SqliteCodeModeStore;

#[derive(Clone)]
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::from_connection(Connection::open(path).map_err(map_sql)?)
    }

    pub fn in_memory() -> StoreResult<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(map_sql)?)
    }

    /// The shared connection, so sibling stores in this crate reuse one handle.
    #[cfg(feature = "codemode")]
    pub(crate) fn raw_connection(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.connection)
    }

    pub fn apply_migrations(&self) -> StoreResult<()> {
        migrations::apply(&mut *self.connection()?)
    }

    fn from_connection(connection: Connection) -> StoreResult<Self> {
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(map_sql)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(map_sql)?;
        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        store.apply_migrations()?;
        Ok(store)
    }

    fn connection(&self) -> StoreResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| StoreError::Backend("SQLite mutex poisoned".into()))
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn upsert_source(&self, request: UpsertSource) -> StoreResult<()> {
        request.validate()?;
        self.connection()?
            .execute(
                "INSERT INTO sources (tenant_id, source_id, enabled, observed_at_epoch_seconds)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (tenant_id, source_id) DO UPDATE SET
                enabled = excluded.enabled,
                observed_at_epoch_seconds = excluded.observed_at_epoch_seconds",
                params![
                    request.tenant_id,
                    request.source_id.as_str(),
                    bool_int(request.enabled),
                    request.observed_at_epoch_seconds,
                ],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    async fn create_watch(&self, request: CreateWatch) -> StoreResult<Watch> {
        request.validate()?;
        let watch = request.into_watch();
        self.connection()?
            .execute(
                "INSERT INTO watches (
                tenant_id, watch_id, query_json, source_ids_json, interval_seconds,
                cursor_json, enabled, created_at_epoch_seconds, next_due_epoch_seconds
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    watch.tenant_id,
                    watch.watch_id,
                    json_text(&watch.query)?,
                    source_ids_text(&watch.source_ids)?,
                    i64_from_u64(watch.interval_seconds, "interval_seconds")?,
                    json_text(&watch.cursor)?,
                    bool_int(watch.enabled),
                    watch.created_at_epoch_seconds,
                    watch.next_due_epoch_seconds,
                ],
            )
            .map_err(map_sql)?;
        Ok(watch)
    }

    async fn list_watches(&self, tenant_id: &str) -> StoreResult<Vec<Watch>> {
        #[allow(
            clippy::significant_drop_tightening,
            reason = "rusqlite statement rows borrow the connection guard until collection completes."
        )]
        fn inner(store: &SqliteStore, tenant_id: &str) -> StoreResult<Vec<Watch>> {
            let connection = store.connection()?;
            let mut statement = connection.prepare(WATCH_SELECT_SQL).map_err(map_sql)?;
            let rows = statement
                .query_map(params![tenant_id], watch_from_row)
                .map_err(map_sql)?;
            collect_rows(rows)
        }
        validate_tenant_id(tenant_id)?;
        inner(self, tenant_id)
    }

    async fn delete_watch(&self, request: DeleteWatch) -> StoreResult<bool> {
        request.validate()?;
        let changed = self
            .connection()?
            .execute(
                "DELETE FROM watches WHERE tenant_id = ?1 AND watch_id = ?2",
                params![request.tenant_id, request.watch_id],
            )
            .map_err(map_sql)?;
        Ok(changed > 0)
    }

    async fn claim_due_watch(&self, request: ClaimDueWatch) -> StoreResult<Option<ClaimedWatch>> {
        #[allow(
            clippy::significant_drop_tightening,
            reason = "SQLite transactions borrow the connection guard until commit or rollback."
        )]
        fn inner(store: &SqliteStore, request: ClaimDueWatch) -> StoreResult<Option<ClaimedWatch>> {
            let mut connection = store.connection()?;
            let transaction = connection.transaction().map_err(map_sql)?;
            let Some(watch) = select_due_watch(&transaction, &request)? else {
                transaction.commit().map_err(map_sql)?;
                return Ok(None);
            };
            let lease_expires =
                request.now_epoch_seconds + i64_from_u64(request.lease_seconds, "lease_seconds")?;
            let changed = transaction
                .execute(
                    CLAIM_WATCH_SQL,
                    params![
                        request.lease_owner,
                        lease_expires,
                        request.tenant_id,
                        watch.watch_id,
                        request.now_epoch_seconds,
                    ],
                )
                .map_err(map_sql)?;
            transaction.commit().map_err(map_sql)?;
            if changed == 0 {
                return Ok(None);
            }
            Ok(Some(ClaimedWatch {
                watch,
                lease_owner: request.lease_owner,
                lease_expires_epoch_seconds: lease_expires,
            }))
        }
        request.validate()?;
        inner(self, request)
    }

    async fn start_watch_run(&self, request: StartWatchRun) -> StoreResult<WatchRun> {
        #[allow(
            clippy::significant_drop_tightening,
            reason = "fetching the idempotent run reuses the same SQLite connection guard."
        )]
        fn inner(store: &SqliteStore, request: &StartWatchRun) -> StoreResult<WatchRun> {
            let connection = store.connection()?;
            connection
                .execute(
                    START_RUN_SQL,
                    params![
                        request.tenant_id,
                        request.watch_id,
                        request.run_id,
                        request.idempotency_key,
                        request.lease_owner,
                        request.started_at_epoch_seconds,
                    ],
                )
                .map_err(map_sql)?;
            fetch_watch_run(&connection, request)
        }
        request.validate()?;
        inner(self, &request)
    }

    async fn complete_watch_run(&self, request: CompleteWatchRun) -> StoreResult<WatchRunOutcome> {
        #[allow(
            clippy::significant_drop_tightening,
            reason = "SQLite transactions borrow the connection guard for the full atomic completion."
        )]
        fn inner(store: &SqliteStore, request: CompleteWatchRun) -> StoreResult<WatchRunOutcome> {
            let mut connection = store.connection()?;
            let transaction = connection.transaction().map_err(map_sql)?;
            let interval = watch_interval_under_lease(&transaction, &request)?;
            ensure_running_run(&transaction, &request)?;
            let mut records_inserted = 0;
            let mut watch_records_inserted = 0;
            let mut new_records = Vec::new();
            for record in &request.records {
                records_inserted += upsert_record(&transaction, &request, record)?;
                let inserted = upsert_watch_record(&transaction, &request, record)?;
                watch_records_inserted += inserted;
                if inserted == 1 {
                    new_records.push(record.clone());
                }
            }
            insert_run_delivery(&transaction, &request, &new_records)?;
            let status = if request.error.is_some() {
                "failed"
            } else {
                "succeeded"
            };
            finish_run_and_watch(&transaction, &request, status, interval)?;
            transaction.commit().map_err(map_sql)?;
            Ok(WatchRunOutcome {
                run_id: request.run_id,
                status: status.into(),
                records_seen: request.records.len(),
                records_inserted,
                watch_records_inserted,
            })
        }
        request.validate()?;
        inner(self, request)
    }

    async fn history(&self, request: HistoryRequest) -> StoreResult<Vec<RecordObservation>> {
        request.validate()?;
        let connection = self.connection()?;
        if request.watch_id.is_some() {
            history_for_watch(&connection, &request)
        } else {
            history_for_tenant(&connection, &request)
        }
    }

    async fn claim_pending_delivery(
        &self,
        request: ClaimPendingDelivery,
    ) -> StoreResult<Option<Delivery>> {
        #[allow(
            clippy::significant_drop_tightening,
            reason = "SQLite transactions borrow the connection guard until commit or rollback."
        )]
        fn inner(
            store: &SqliteStore,
            request: &ClaimPendingDelivery,
        ) -> StoreResult<Option<Delivery>> {
            let mut connection = store.connection()?;
            let transaction = connection.transaction().map_err(map_sql)?;
            let delivery = claim_pending_delivery(&transaction, request)?;
            transaction.commit().map_err(map_sql)?;
            Ok(delivery)
        }
        request.validate()?;
        inner(self, &request)
    }

    async fn complete_delivery(&self, request: CompleteDelivery) -> StoreResult<Delivery> {
        request.validate()?;
        let connection = self.connection()?;
        complete_delivery(&connection, &request)
    }

    async fn list_deliveries(&self, request: ListDeliveries) -> StoreResult<Vec<Delivery>> {
        request.validate()?;
        let connection = self.connection()?;
        list_deliveries(&connection, &request)
    }
}
