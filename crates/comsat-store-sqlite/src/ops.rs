use comsat_store::{
    ClaimDueWatch, CompleteWatchRun, HistoryRequest, RecordObservation, StartWatchRun, StoreError,
    StoreResult, Watch, WatchRun,
};
use comsat_types::Record;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::{
    codec::{
        collect_rows, json_text, map_sql, observation_from_row, watch_from_row, watch_run_from_row,
    },
    sql::DUE_WATCH_SQL,
};

pub fn select_due_watch(
    transaction: &Transaction<'_>,
    request: &ClaimDueWatch,
) -> StoreResult<Option<Watch>> {
    transaction
        .query_row(
            DUE_WATCH_SQL,
            params![request.tenant_id, request.now_epoch_seconds],
            watch_from_row,
        )
        .optional()
        .map_err(map_sql)
}

pub fn watch_interval_under_lease(
    transaction: &Transaction<'_>,
    request: &CompleteWatchRun,
) -> StoreResult<i64> {
    transaction
        .query_row(
            "SELECT interval_seconds FROM watches
             WHERE tenant_id = ?1 AND watch_id = ?2 AND lease_owner = ?3
               AND lease_expires_epoch_seconds > ?4",
            params![
                request.tenant_id,
                request.watch_id,
                request.lease_owner,
                request.finished_at_epoch_seconds
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sql)?
        .ok_or_else(|| StoreError::Conflict("watch lease is not held by this worker".into()))
}

pub fn ensure_running_run(
    transaction: &Transaction<'_>,
    request: &CompleteWatchRun,
) -> StoreResult<()> {
    transaction
        .query_row(
            "SELECT 1 FROM watch_runs
             WHERE tenant_id = ?1 AND watch_id = ?2 AND run_id = ?3
               AND lease_owner = ?4 AND status = 'running'",
            params![
                request.tenant_id,
                request.watch_id,
                request.run_id,
                request.lease_owner
            ],
            |_| Ok(()),
        )
        .optional()
        .map_err(map_sql)?
        .ok_or_else(|| StoreError::Conflict("watch run is not running under this lease".into()))
}

pub fn upsert_record(
    transaction: &Transaction<'_>,
    request: &CompleteWatchRun,
    record: &Record,
) -> StoreResult<usize> {
    let exists = record_exists(transaction, &request.tenant_id, record)?;
    let metadata = json_text(&record.metadata)?;
    transaction
        .execute(
            UPSERT_RECORD_SQL,
            params![
                request.tenant_id,
                record.source.as_str(),
                record.id.as_str(),
                record.kind,
                record.url,
                record.title,
                record.text,
                record.author,
                record.created_at,
                record.updated_at,
                metadata,
                request.finished_at_epoch_seconds,
            ],
        )
        .map_err(map_sql)?;
    Ok(usize::from(!exists))
}

pub fn upsert_watch_record(
    transaction: &Transaction<'_>,
    request: &CompleteWatchRun,
    record: &Record,
) -> StoreResult<usize> {
    let exists = watch_record_exists(transaction, request, record)?;
    transaction
        .execute(
            UPSERT_WATCH_RECORD_SQL,
            params![
                request.tenant_id,
                request.watch_id,
                record.source.as_str(),
                record.id.as_str(),
                request.finished_at_epoch_seconds,
            ],
        )
        .map_err(map_sql)?;
    Ok(usize::from(!exists))
}

pub fn finish_run_and_watch(
    transaction: &Transaction<'_>,
    request: &CompleteWatchRun,
    status: &str,
    interval: i64,
) -> StoreResult<()> {
    transaction
        .execute(
            "UPDATE watch_runs SET status = ?1, finished_at_epoch_seconds = ?2, error = ?3
         WHERE tenant_id = ?4 AND watch_id = ?5 AND run_id = ?6",
            params![
                status,
                request.finished_at_epoch_seconds,
                request.error,
                request.tenant_id,
                request.watch_id,
                request.run_id
            ],
        )
        .map_err(map_sql)?;
    transaction
        .execute(
            "UPDATE watches
         SET cursor_json = ?1, next_due_epoch_seconds = ?2,
             lease_owner = NULL, lease_expires_epoch_seconds = NULL
         WHERE tenant_id = ?3 AND watch_id = ?4",
            params![
                json_text(&request.next_cursor)?,
                request.finished_at_epoch_seconds + interval,
                request.tenant_id,
                request.watch_id
            ],
        )
        .map_err(map_sql)?;
    Ok(())
}

pub fn history_for_tenant(
    connection: &Connection,
    request: &HistoryRequest,
) -> StoreResult<Vec<RecordObservation>> {
    let since = request.since_epoch_seconds.unwrap_or(i64::MIN);
    let mut statement = connection.prepare(HISTORY_TENANT_SQL).map_err(map_sql)?;
    collect_rows(
        statement
            .query_map(params![request.tenant_id, since, request.limit], |row| {
                observation_from_row(row, None)
            })
            .map_err(map_sql)?,
    )
}

pub fn history_for_watch(
    connection: &Connection,
    request: &HistoryRequest,
) -> StoreResult<Vec<RecordObservation>> {
    let since = request.since_epoch_seconds.unwrap_or(i64::MIN);
    let watch_id = request.watch_id.as_deref().unwrap_or_default();
    let mut statement = connection.prepare(HISTORY_WATCH_SQL).map_err(map_sql)?;
    collect_rows(
        statement
            .query_map(
                params![request.tenant_id, watch_id, since, request.limit],
                |row| observation_from_row(row, Some(watch_id.to_owned())),
            )
            .map_err(map_sql)?,
    )
}

pub fn fetch_watch_run(connection: &Connection, request: &StartWatchRun) -> StoreResult<WatchRun> {
    connection
        .query_row(
            FETCH_RUN_SQL,
            params![request.tenant_id, request.watch_id, request.idempotency_key],
            watch_run_from_row,
        )
        .optional()
        .map_err(map_sql)?
        .ok_or_else(|| StoreError::Conflict("watch is not currently claimed by this worker".into()))
}

fn record_exists(
    transaction: &Transaction<'_>,
    tenant_id: &str,
    record: &Record,
) -> StoreResult<bool> {
    exists(
        transaction,
        "records",
        params![tenant_id, record.source.as_str(), record.id.as_str()],
    )
}

fn watch_record_exists(
    transaction: &Transaction<'_>,
    request: &CompleteWatchRun,
    record: &Record,
) -> StoreResult<bool> {
    exists(
        transaction,
        "watch_records",
        params![
            request.tenant_id,
            request.watch_id,
            record.source.as_str(),
            record.id.as_str(),
        ],
    )
}

fn exists(
    transaction: &Transaction<'_>,
    table: &str,
    params: impl rusqlite::Params,
) -> StoreResult<bool> {
    let keys = if table == "records" {
        "tenant_id = ?1 AND source_id = ?2 AND record_id = ?3"
    } else {
        "tenant_id = ?1 AND watch_id = ?2 AND source_id = ?3 AND record_id = ?4"
    };
    let sql = format!("SELECT 1 FROM {table} WHERE {keys}");
    transaction
        .query_row(&sql, params, |_| Ok(()))
        .optional()
        .map_err(map_sql)
        .map(|value| value.is_some())
}

const UPSERT_RECORD_SQL: &str = "INSERT INTO records (
    tenant_id, source_id, record_id, kind, url, title, text, author, created_at,
    updated_at, metadata_json, first_observed_epoch_seconds, last_observed_epoch_seconds
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
ON CONFLICT (tenant_id, source_id, record_id) DO UPDATE SET
    kind = excluded.kind, url = excluded.url, title = excluded.title,
    text = excluded.text, author = excluded.author, created_at = excluded.created_at,
    updated_at = excluded.updated_at, metadata_json = excluded.metadata_json,
    last_observed_epoch_seconds = excluded.last_observed_epoch_seconds";

const UPSERT_WATCH_RECORD_SQL: &str = "INSERT INTO watch_records (
    tenant_id, watch_id, source_id, record_id, first_observed_epoch_seconds,
    last_observed_epoch_seconds
) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
ON CONFLICT (tenant_id, watch_id, source_id, record_id) DO UPDATE SET
    last_observed_epoch_seconds = excluded.last_observed_epoch_seconds";

const HISTORY_TENANT_SQL: &str = "SELECT source_id, record_id, kind, url, title,
    text, author, created_at, updated_at, metadata_json, first_observed_epoch_seconds,
    last_observed_epoch_seconds
FROM records
WHERE tenant_id = ?1 AND last_observed_epoch_seconds >= ?2
ORDER BY last_observed_epoch_seconds DESC, source_id, record_id
LIMIT ?3";

const HISTORY_WATCH_SQL: &str = "SELECT r.source_id, r.record_id, r.kind, r.url,
    r.title, r.text, r.author, r.created_at, r.updated_at, r.metadata_json,
    wr.first_observed_epoch_seconds, wr.last_observed_epoch_seconds
FROM watch_records wr
JOIN records r ON r.tenant_id = wr.tenant_id
 AND r.source_id = wr.source_id AND r.record_id = wr.record_id
WHERE wr.tenant_id = ?1 AND wr.watch_id = ?2 AND wr.last_observed_epoch_seconds >= ?3
ORDER BY wr.last_observed_epoch_seconds DESC, r.source_id, r.record_id
LIMIT ?4";

const FETCH_RUN_SQL: &str = "SELECT tenant_id, watch_id, run_id, idempotency_key,
    lease_owner, status, started_at_epoch_seconds, finished_at_epoch_seconds, error
FROM watch_runs
WHERE tenant_id = ?1 AND watch_id = ?2 AND idempotency_key = ?3";
