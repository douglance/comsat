use comsat_store::{Delivery, RecordObservation, StoreError, StoreResult, Watch, WatchRun};
use comsat_types::{Record, RecordId, SourceId};
use serde_json::Value;

pub fn watch_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Watch> {
    Ok(Watch {
        tenant_id: row.get(0)?,
        watch_id: row.get(1)?,
        query: serde_json::from_str(&row.get::<_, String>(2)?).map_err(json_err)?,
        source_ids: source_ids_from_text(&row.get::<_, String>(3)?)?,
        interval_seconds: row.get(4)?,
        cursor: serde_json::from_str(&row.get::<_, String>(5)?).map_err(json_err)?,
        enabled: int_bool(row.get(6)?),
        created_at_epoch_seconds: row.get(7)?,
        next_due_epoch_seconds: row.get(8)?,
        lease_owner: row.get(9)?,
        lease_expires_epoch_seconds: row.get(10)?,
    })
}

pub fn watch_run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WatchRun> {
    Ok(WatchRun {
        tenant_id: row.get(0)?,
        watch_id: row.get(1)?,
        run_id: row.get(2)?,
        idempotency_key: row.get(3)?,
        lease_owner: row.get(4)?,
        status: row.get(5)?,
        started_at_epoch_seconds: row.get(6)?,
        finished_at_epoch_seconds: row.get(7)?,
        error: row.get(8)?,
    })
}

pub fn observation_from_row(
    row: &rusqlite::Row<'_>,
    watch_id: Option<String>,
) -> rusqlite::Result<RecordObservation> {
    let metadata: Value = serde_json::from_str(&row.get::<_, String>(9)?).map_err(json_err)?;
    Ok(RecordObservation {
        record: Record {
            source: SourceId::new(row.get::<_, String>(0)?).map_err(validation_err)?,
            id: RecordId::new(row.get::<_, String>(1)?).map_err(validation_err)?,
            kind: row.get(2)?,
            url: row.get(3)?,
            title: row.get(4)?,
            text: row.get(5)?,
            author: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            metadata,
        },
        watch_id,
        first_observed_epoch_seconds: row.get(10)?,
        last_observed_epoch_seconds: row.get(11)?,
    })
}

pub fn delivery_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Delivery> {
    Ok(Delivery {
        tenant_id: row.get(0)?,
        delivery_id: row.get(1)?,
        watch_id: row.get(2)?,
        run_id: row.get(3)?,
        status: row.get(4)?,
        target: serde_json::from_str(&row.get::<_, String>(5)?).map_err(json_err)?,
        payload: serde_json::from_str(&row.get::<_, String>(6)?).map_err(json_err)?,
        created_at_epoch_seconds: row.get(7)?,
        next_attempt_epoch_seconds: row.get(8)?,
        attempts: row.get(9)?,
        lease_owner: row.get(10)?,
        lease_expires_epoch_seconds: row.get(11)?,
        delivered_at_epoch_seconds: row.get(12)?,
        error: row.get(13)?,
    })
}

pub fn source_ids_text(values: &[SourceId]) -> StoreResult<String> {
    json_text(&values.iter().map(SourceId::as_str).collect::<Vec<_>>())
}

pub fn collect_rows<T, F>(rows: rusqlite::MappedRows<'_, F>) -> StoreResult<Vec<T>>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sql)
}

pub fn json_text(value: &impl serde::Serialize) -> StoreResult<String> {
    serde_json::to_string(value).map_err(|error| StoreError::Backend(error.to_string()))
}

pub const fn bool_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

pub fn i64_from_u64(value: u64, name: &str) -> StoreResult<i64> {
    i64::try_from(value).map_err(|_| StoreError::Validation(format!("{name} is too large")))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rusqlite map_err passes owned errors to conversion functions."
)]
pub fn map_sql(error: rusqlite::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}

fn source_ids_from_text(value: &str) -> rusqlite::Result<Vec<SourceId>> {
    let raw: Vec<String> = serde_json::from_str(value).map_err(json_err)?;
    raw.into_iter()
        .map(SourceId::new)
        .map(|value| value.map_err(validation_err))
        .collect()
}

const fn int_bool(value: i64) -> bool {
    value != 0
}

fn json_err(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn validation_err(error: comsat_types::ValidationError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
