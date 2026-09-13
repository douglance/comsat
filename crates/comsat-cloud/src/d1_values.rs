use comsat_store::{
    CompleteDelivery, CompleteWatchRun, DELIVERY_RETRY_SECONDS, MAX_DELIVERY_ATTEMPTS, StoreError,
    StoreResult, Watch,
};
use comsat_types::Record;
use serde::Serialize;
use worker::wasm_bindgen::JsValue;

pub(super) const MAX_RECORDS_JSON_BYTES: usize = 1_048_576;

pub(super) fn watch_values(watch: &Watch) -> StoreResult<Vec<JsValue>> {
    Ok(vec![
        text(&watch.tenant_id),
        text(&watch.watch_id),
        json_text(&watch.query)?,
        json_text(&watch.source_ids)?,
        int(i64_from_u64(watch.interval_seconds, "interval_seconds")?),
        json_text(&watch.cursor)?,
        bool_value(watch.enabled),
        int(watch.created_at_epoch_seconds),
        int(watch.next_due_epoch_seconds),
    ])
}

pub(super) fn completion_gate_values(request: &CompleteWatchRun) -> Vec<JsValue> {
    vec![
        text(&request.tenant_id),
        text(&request.watch_id),
        text(&request.run_id),
        text(&request.lease_owner),
        int(request.finished_at_epoch_seconds),
    ]
}

pub(super) fn records_json(request: &CompleteWatchRun) -> StoreResult<String> {
    let records = request
        .records
        .iter()
        .map(BulkRecord::from_record)
        .collect::<StoreResult<Vec<_>>>()?;
    let json = serde_json::to_string(&records).map_err(json_backend)?;
    if json.len() <= MAX_RECORDS_JSON_BYTES {
        return Ok(json);
    }
    Err(StoreError::Validation(format!(
        "serialized D1 completion records exceed {MAX_RECORDS_JSON_BYTES} bytes"
    )))
}

pub(super) fn bulk_record_values(request: &CompleteWatchRun, records_json: &str) -> Vec<JsValue> {
    vec![
        text(records_json),
        text(&request.tenant_id),
        text(&request.watch_id),
        text(&request.run_id),
        text(&request.lease_owner),
        int(request.finished_at_epoch_seconds),
    ]
}

pub(super) fn complete_run_values(request: &CompleteWatchRun) -> Vec<JsValue> {
    vec![
        text(status_for(request.error.as_ref())),
        int(request.finished_at_epoch_seconds),
        optional_text(request.error.as_deref()),
        text(&request.tenant_id),
        text(&request.watch_id),
        text(&request.run_id),
        text(&request.lease_owner),
    ]
}

pub(super) fn complete_watch_values(request: &CompleteWatchRun) -> StoreResult<Vec<JsValue>> {
    Ok(vec![
        json_text(&request.next_cursor)?,
        int(request.finished_at_epoch_seconds),
        text(&request.tenant_id),
        text(&request.watch_id),
        text(&request.lease_owner),
        int(request.finished_at_epoch_seconds),
        text(&request.run_id),
    ])
}

pub(super) fn run_delivery_values(
    request: &CompleteWatchRun,
    records_json: &str,
) -> StoreResult<Option<Vec<JsValue>>> {
    let Some(delivery) = &request.delivery else {
        return Ok(None);
    };
    Ok(Some(vec![
        text(records_json),
        text(&request.tenant_id),
        text(&request.watch_id),
        text(&request.run_id),
        text(&request.lease_owner),
        int(request.finished_at_epoch_seconds),
        text(&delivery.delivery_id),
        json_text(&delivery.target)?,
    ]))
}

pub(super) fn claim_delivery_values(
    request: &comsat_store::ClaimPendingDelivery,
    lease_expires: i64,
) -> Vec<JsValue> {
    vec![
        text(&request.lease_owner),
        int(lease_expires),
        text(&request.tenant_id),
        int(request.now_epoch_seconds),
    ]
}

pub(super) fn complete_delivery_values(request: &CompleteDelivery) -> Vec<JsValue> {
    vec![
        bool_value(request.success),
        int(i64::from(MAX_DELIVERY_ATTEMPTS)),
        int(next_attempt_epoch_seconds(request)),
        optional_int(request.success.then_some(request.finished_at_epoch_seconds)),
        optional_text(request.error.as_deref()),
        text(&request.tenant_id),
        text(&request.delivery_id),
        text(&request.lease_owner),
        int(request.finished_at_epoch_seconds),
    ]
}

pub(super) const fn status_for(error: Option<&String>) -> &'static str {
    if error.is_some() {
        "failed"
    } else {
        "succeeded"
    }
}

pub(super) fn text(value: &str) -> JsValue {
    JsValue::from_str(value)
}

fn optional_text(value: Option<&str>) -> JsValue {
    value.map_or_else(JsValue::null, JsValue::from_str)
}

fn optional_int(value: Option<i64>) -> JsValue {
    value.map_or_else(JsValue::null, int)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "D1 binds integers as JavaScript numbers; COMSAT epoch values stay below 53-bit precision."
)]
pub(super) fn int(value: i64) -> JsValue {
    JsValue::from_f64(value as f64)
}

pub(super) const fn bool_value(value: bool) -> JsValue {
    JsValue::from_bool(value)
}

fn json_text(value: &impl serde::Serialize) -> StoreResult<JsValue> {
    serde_json::to_string(value)
        .map(|value| text(&value))
        .map_err(json_backend)
}

pub(super) fn i64_from_u64(value: u64, name: &str) -> StoreResult<i64> {
    i64::try_from(value).map_err(|_| StoreError::Validation(format!("{name} is too large")))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "serde_json::Error is received by value from map_err callbacks."
)]
fn json_backend(error: serde_json::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}

const fn next_attempt_epoch_seconds(request: &CompleteDelivery) -> i64 {
    if request.success {
        request.finished_at_epoch_seconds
    } else {
        request.finished_at_epoch_seconds + DELIVERY_RETRY_SECONDS
    }
}

#[derive(Serialize)]
struct BulkRecord {
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
}

impl BulkRecord {
    fn from_record(record: &Record) -> StoreResult<Self> {
        Ok(Self {
            source_id: record.source.as_str().to_owned(),
            record_id: record.id.as_str().to_owned(),
            kind: record.kind.clone(),
            url: record.url.clone(),
            title: record.title.clone(),
            text: record.text.clone(),
            author: record.author.clone(),
            created_at: record.created_at.clone(),
            updated_at: record.updated_at.clone(),
            metadata_json: serde_json::to_string(&record.metadata).map_err(json_backend)?,
        })
    }
}
