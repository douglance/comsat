#![cfg(not(target_arch = "wasm32"))]

#[allow(
    dead_code,
    reason = "Shared SQL tests include the complete production D1 statement module but exercise selected statements per test."
)]
#[path = "../../src/d1_sql.rs"]
mod d1_sql;
#[allow(
    dead_code,
    reason = "Native SQL tests include production D1 value helpers but exercise only JSON batching."
)]
#[path = "../../src/d1_values.rs"]
mod d1_values;

use comsat_store::{
    CompleteDelivery, CompleteWatchRun, DELIVERY_RETRY_SECONDS, MAX_DELIVERY_ATTEMPTS,
};
use comsat_types::{Record, RecordId, SourceId};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

const MIGRATIONS: &str = concat!(
    include_str!("../../../../migrations/0001_initial.sql"),
    "\n",
    include_str!("../../../../migrations/0002_delivery_notifications.sql")
);

pub fn database() -> rusqlite::Result<Connection> {
    let connection = Connection::open_in_memory()?;
    connection.execute_batch(MIGRATIONS)?;
    Ok(connection)
}

pub fn create_watch(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
    due_at: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        d1_sql::INSERT_WATCH_SQL,
        params![
            tenant_id,
            watch_id,
            r#"{"text":"MCP OAuth"}"#,
            r#"["github"]"#,
            60,
            "{}",
            1,
            1,
            due_at
        ],
    )?;
    Ok(())
}

pub fn upsert_source(
    connection: &Connection,
    tenant_id: &str,
    source_id: &str,
    enabled: bool,
    observed_at: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        d1_sql::UPSERT_SOURCE_SQL,
        params![tenant_id, source_id, enabled, observed_at],
    )?;
    Ok(())
}

pub fn source_enabled(
    connection: &Connection,
    tenant_id: &str,
    source_id: &str,
) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT enabled FROM sources WHERE tenant_id = ?1 AND source_id = ?2",
        params![tenant_id, source_id],
        |row| row.get(0),
    )
}

pub fn list_watch_ids(connection: &Connection, tenant_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(d1_sql::LIST_WATCHES_SQL)?;
    statement
        .query_map(params![tenant_id], |row| row.get::<_, String>(1))?
        .collect()
}

pub fn delete_watch(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
) -> rusqlite::Result<usize> {
    connection.execute(d1_sql::DELETE_WATCH_SQL, params![tenant_id, watch_id])
}

pub fn claim_watch(
    connection: &Connection,
    tenant_id: &str,
    lease_owner: &str,
    now: i64,
    lease_expires: i64,
) -> rusqlite::Result<String> {
    connection.query_row(
        d1_sql::CLAIM_DUE_WATCH_SQL,
        params![lease_owner, lease_expires, tenant_id, now],
        |row| row.get(1),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Test helper mirrors START_RUN_SQL parameters."
)]
pub fn start_run(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
    run_id: &str,
    idempotency_key: &str,
    lease_owner: &str,
    started_at: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        d1_sql::START_RUN_SQL,
        params![
            tenant_id,
            watch_id,
            run_id,
            idempotency_key,
            lease_owner,
            started_at
        ],
    )?;
    Ok(())
}

pub fn idempotent_run_id(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
    idempotency_key: &str,
) -> rusqlite::Result<String> {
    connection.query_row(
        d1_sql::LOAD_RUN_SQL,
        params![tenant_id, watch_id, idempotency_key],
        |row| row.get(2),
    )
}

pub fn watch_run_count(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COUNT(*) FROM watch_runs WHERE tenant_id = ?1 AND watch_id = ?2",
        params![tenant_id, watch_id],
        |row| row.get(0),
    )
}

pub fn complete_run(
    connection: &Connection,
    input: CompleteInput<'_>,
) -> rusqlite::Result<CompleteOutcome> {
    let records_json = test_records_json(input.records);
    let gate_count = connection
        .query_row(
            d1_sql::COMPLETE_GATE_SQL,
            params![
                input.tenant_id,
                input.watch_id,
                input.run_id,
                input.lease_owner,
                input.finished_at
            ],
            |_| Ok(1),
        )
        .optional()?
        .unwrap_or_default();
    let record_inserts = insert_records(connection, &input, &records_json)?;
    let record_updates = update_records(connection, &input, &records_json)?;
    let delivery_inserts = insert_delivery(connection, &input, &records_json)?;
    let watch_record_inserts = insert_watch_records(connection, &input, &records_json)?;
    let watch_record_updates = update_watch_records(connection, &input, &records_json)?;
    let run_updates = finish_run(connection, &input)?;
    let watch_updates = finish_watch(connection, &input)?;
    Ok(CompleteOutcome {
        gate_count,
        record_inserts,
        record_updates,
        delivery_inserts,
        watch_record_inserts,
        watch_record_updates,
        run_updates,
        watch_updates,
    })
}

fn insert_records(
    connection: &Connection,
    input: &CompleteInput<'_>,
    records_json: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        d1_sql::INSERT_RECORDS_SQL,
        params![
            records_json,
            input.tenant_id,
            input.watch_id,
            input.run_id,
            input.lease_owner,
            input.finished_at
        ],
    )
}

fn update_records(
    connection: &Connection,
    input: &CompleteInput<'_>,
    records_json: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        d1_sql::UPDATE_RECORDS_SQL,
        params![
            records_json,
            input.tenant_id,
            input.watch_id,
            input.run_id,
            input.lease_owner,
            input.finished_at
        ],
    )
}

fn insert_delivery(
    connection: &Connection,
    input: &CompleteInput<'_>,
    records_json: &str,
) -> rusqlite::Result<usize> {
    let Some(delivery_id) = input.delivery_id else {
        return Ok(0);
    };
    connection.execute(
        d1_sql::INSERT_RUN_DELIVERY_SQL,
        params![
            records_json,
            input.tenant_id,
            input.watch_id,
            input.run_id,
            input.lease_owner,
            input.finished_at,
            delivery_id,
            r#"{"type":"webhook","url":"https://example.com/hook"}"#,
        ],
    )
}

fn insert_watch_records(
    connection: &Connection,
    input: &CompleteInput<'_>,
    records_json: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        d1_sql::INSERT_WATCH_RECORDS_SQL,
        params![
            records_json,
            input.tenant_id,
            input.watch_id,
            input.run_id,
            input.lease_owner,
            input.finished_at
        ],
    )
}

fn update_watch_records(
    connection: &Connection,
    input: &CompleteInput<'_>,
    records_json: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        d1_sql::UPDATE_WATCH_RECORDS_SQL,
        params![
            records_json,
            input.tenant_id,
            input.watch_id,
            input.run_id,
            input.lease_owner,
            input.finished_at
        ],
    )
}

fn finish_run(connection: &Connection, input: &CompleteInput<'_>) -> rusqlite::Result<usize> {
    connection.execute(
        d1_sql::COMPLETE_RUN_SQL,
        params![
            input.status,
            input.finished_at,
            input.error,
            input.tenant_id,
            input.watch_id,
            input.run_id,
            input.lease_owner
        ],
    )
}

fn finish_watch(connection: &Connection, input: &CompleteInput<'_>) -> rusqlite::Result<usize> {
    connection.execute(
        d1_sql::COMPLETE_WATCH_SQL,
        params![
            "{}",
            input.finished_at,
            input.tenant_id,
            input.watch_id,
            input.lease_owner,
            input.finished_at,
            input.run_id
        ],
    )
}

fn test_records_json(records: &[TestRecord]) -> String {
    serde_json::to_string(
        &records
            .iter()
            .map(|record| {
                serde_json::json!({
                    "source_id": record.source,
                    "record_id": record.record_id,
                    "kind": "issue",
                    "url": record.url,
                    "title": "title",
                    "text": "body",
                    "author": "author",
                    "created_at": "2026-09-13T00:00:00Z",
                    "updated_at": "2026-09-13T00:00:01Z",
                    "metadata_json": "{}"
                })
            })
            .collect::<Vec<_>>(),
    )
    .expect("test records serialize")
}

pub fn history_for_tenant(
    connection: &Connection,
    tenant_id: &str,
) -> rusqlite::Result<Vec<String>> {
    collect_record_ids(
        connection,
        d1_sql::HISTORY_TENANT_SQL,
        params![tenant_id, i64::MIN, 100],
    )
}

pub fn history_for_watch(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
) -> rusqlite::Result<Vec<String>> {
    collect_record_ids(
        connection,
        d1_sql::HISTORY_WATCH_SQL,
        params![tenant_id, watch_id, i64::MIN, 100],
    )
}

fn collect_record_ids<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(sql)?;
    statement
        .query_map(params, |row| row.get::<_, String>(1))?
        .collect()
}

pub fn watch_next_due(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
) -> rusqlite::Result<Option<i64>> {
    connection
        .query_row(
            "SELECT next_due_epoch_seconds FROM watches WHERE tenant_id = ?1 AND watch_id = ?2",
            params![tenant_id, watch_id],
            |row| row.get(0),
        )
        .optional()
}

pub fn watch_lease_owner(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
) -> rusqlite::Result<Option<String>> {
    connection.query_row(
        "SELECT lease_owner FROM watches WHERE tenant_id = ?1 AND watch_id = ?2",
        params![tenant_id, watch_id],
        |row| row.get(0),
    )
}

pub fn run_status(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
    run_id: &str,
) -> rusqlite::Result<String> {
    connection.query_row(
        "SELECT status FROM watch_runs WHERE tenant_id = ?1 AND watch_id = ?2 AND run_id = ?3",
        params![tenant_id, watch_id, run_id],
        |row| row.get(0),
    )
}

pub fn run_error(
    connection: &Connection,
    tenant_id: &str,
    watch_id: &str,
    run_id: &str,
) -> rusqlite::Result<Option<String>> {
    connection.query_row(
        "SELECT error FROM watch_runs WHERE tenant_id = ?1 AND watch_id = ?2 AND run_id = ?3",
        params![tenant_id, watch_id, run_id],
        |row| row.get(0),
    )
}

pub const fn completion_statement_count(has_delivery: bool) -> usize {
    if has_delivery { 8 } else { 7 }
}

pub fn delivery_payload_record_ids(
    connection: &Connection,
    tenant_id: &str,
    delivery_id: &str,
) -> rusqlite::Result<Vec<String>> {
    let payload: String = connection.query_row(
        "SELECT payload_json FROM deliveries WHERE tenant_id = ?1 AND delivery_id = ?2",
        params![tenant_id, delivery_id],
        |row| row.get(0),
    )?;
    let value: serde_json::Value = serde_json::from_str(&payload)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(error.into()))?;
    Ok(value
        .get("records")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|record| record.get("id").and_then(serde_json::Value::as_str))
        .map(ToOwned::to_owned)
        .collect())
}

pub fn delivery_count(connection: &Connection, tenant_id: &str) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COUNT(*) FROM deliveries WHERE tenant_id = ?1",
        params![tenant_id],
        |row| row.get(0),
    )
}

pub fn delivery_status(
    connection: &Connection,
    tenant_id: &str,
    delivery_id: &str,
) -> rusqlite::Result<String> {
    connection.query_row(
        "SELECT status FROM deliveries WHERE tenant_id = ?1 AND delivery_id = ?2",
        params![tenant_id, delivery_id],
        |row| row.get(0),
    )
}

pub fn delivery_next_attempt(
    connection: &Connection,
    tenant_id: &str,
    delivery_id: &str,
) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT next_attempt_epoch_seconds FROM deliveries WHERE tenant_id = ?1 AND delivery_id = ?2",
        params![tenant_id, delivery_id],
        |row| row.get(0),
    )
}

pub fn claim_delivery(
    connection: &Connection,
    tenant_id: &str,
    lease_owner: &str,
    now: i64,
    lease_expires: i64,
) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            d1_sql::CLAIM_DELIVERY_SQL,
            params![lease_owner, lease_expires, tenant_id, now],
            |row| row.get(1),
        )
        .optional()
}

pub fn complete_delivery(
    connection: &Connection,
    identity: (&str, &str, &str),
    finished_at: i64,
    success: bool,
    error: Option<&str>,
) -> rusqlite::Result<String> {
    let (tenant_id, delivery_id, lease_owner) = identity;
    let request = CompleteDelivery {
        tenant_id: tenant_id.into(),
        delivery_id: delivery_id.into(),
        lease_owner: lease_owner.into(),
        finished_at_epoch_seconds: finished_at,
        success,
        error: error.map(str::to_owned),
    };
    connection.query_row(
        d1_sql::COMPLETE_DELIVERY_SQL,
        params![
            success,
            MAX_DELIVERY_ATTEMPTS,
            next_attempt_epoch_seconds(finished_at, success),
            success.then_some(finished_at),
            request.error,
            tenant_id,
            delivery_id,
            lease_owner,
            finished_at,
        ],
        |row| row.get(4),
    )
}

const fn next_attempt_epoch_seconds(finished_at: i64, success: bool) -> i64 {
    if success {
        finished_at
    } else {
        finished_at + DELIVERY_RETRY_SECONDS
    }
}

pub fn oversized_records_error_message() -> String {
    let record = Record {
        id: RecordId::new("oversized").expect("valid record id"),
        source: SourceId::new("github").expect("valid source id"),
        kind: "issue".into(),
        url: "https://example.com/oversized".into(),
        title: None,
        text: Some("x".repeat(d1_values::MAX_RECORDS_JSON_BYTES + 1)),
        author: None,
        created_at: None,
        updated_at: None,
        metadata: json!({}),
    };
    let request = CompleteWatchRun {
        tenant_id: "tenant-a".into(),
        watch_id: "watch-a".into(),
        run_id: "run-a".into(),
        lease_owner: "worker-a".into(),
        finished_at_epoch_seconds: 30,
        records: vec![record],
        next_cursor: json!({}),
        error: None,
        delivery: None,
    };
    d1_values::records_json(&request)
        .expect_err("oversized D1 record batch must fail before database execution")
        .to_string()
}

pub fn record(source: &str, record_id: &str, url: &str) -> TestRecord {
    TestRecord {
        source: source.into(),
        record_id: record_id.into(),
        url: url.into(),
    }
}

#[derive(Clone, Copy)]
pub struct CompleteInput<'a> {
    pub tenant_id: &'a str,
    pub watch_id: &'a str,
    pub run_id: &'a str,
    pub lease_owner: &'a str,
    pub finished_at: i64,
    pub status: &'a str,
    pub error: Option<&'a str>,
    pub records: &'a [TestRecord],
    pub delivery_id: Option<&'a str>,
}

#[derive(Clone)]
pub struct TestRecord {
    source: String,
    record_id: String,
    url: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CompleteOutcome {
    gate_count: usize,
    record_inserts: usize,
    record_updates: usize,
    delivery_inserts: usize,
    watch_record_inserts: usize,
    watch_record_updates: usize,
    run_updates: usize,
    watch_updates: usize,
}

impl CompleteOutcome {
    pub const fn new(
        gate_count: usize,
        record_inserts: usize,
        record_updates: usize,
        watch_record_inserts: usize,
        watch_record_updates: usize,
    ) -> Self {
        Self {
            gate_count,
            record_inserts,
            record_updates,
            delivery_inserts: 0,
            watch_record_inserts,
            watch_record_updates,
            run_updates: gate_count,
            watch_updates: gate_count,
        }
    }
    pub const fn with_delivery(mut self, count: usize) -> Self {
        self.delivery_inserts = count;
        self
    }
}
