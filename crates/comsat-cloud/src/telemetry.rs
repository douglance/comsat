#![allow(
    clippy::future_not_send,
    reason = "Worker D1 bindings execute on the isolate event loop."
)]

use serde_json::json;
use worker::{Env, Result};

thread_local! {
    static ISOLATE_ID: String = format!("{}-{:x}", worker::Date::now().as_millis(), worker::js_sys::Math::random().to_bits());
}

pub fn log_catalog(catalog: &comsat_engine::SourceCatalog) {
    ISOLATE_ID.with(|isolate_id| {
        worker::console_log!(
            "{}",
            json!({"event": "source_metrics", "isolate_id": isolate_id, "cumulative": catalog.metrics()})
        );
    });
}

const STORAGE_SNAPSHOT: &str = "SELECT
    (SELECT COUNT(DISTINCT tenant_id) FROM watches WHERE enabled = 1) AS active_tenants,
    (SELECT COUNT(*) FROM watches WHERE enabled = 1) AS active_watches,
    (SELECT COUNT(*) FROM watches WHERE enabled = 1 AND next_due_epoch_seconds <= ?1)
        AS due_watches,
    (SELECT COUNT(*) FROM watches WHERE lease_expires_epoch_seconds > ?1)
        AS leased_watches,
    (SELECT COUNT(*) FROM records) AS records_stored,
    (SELECT COALESCE(SUM(LENGTH(CAST(metadata_json AS BLOB))
        + COALESCE(LENGTH(CAST(text AS BLOB)), 0)
        + COALESCE(LENGTH(CAST(title AS BLOB)), 0)), 0) FROM records)
        AS record_content_bytes,
    (SELECT COUNT(*) FROM watch_runs) AS watch_runs,
    (SELECT COUNT(*) FROM watch_runs WHERE status = 'failed') AS watch_failures,
    (SELECT COUNT(*) FROM deliveries WHERE status = 'failed') AS notification_failures";

pub async fn log_storage_snapshot(env: &Env) {
    match storage_snapshot(env).await {
        Ok(snapshot) => worker::console_log!(
            "{}",
            json!({"event": "storage_snapshot", "values": snapshot})
        ),
        Err(_) => worker::console_error!(
            "{}",
            json!({"event": "d1_failure", "operation": "storage_snapshot"})
        ),
    }
}

async fn storage_snapshot(env: &Env) -> Result<Option<serde_json::Value>> {
    let now = worker::Date::now().as_millis() / 1000;
    env.d1("COMSAT_DB")?
        .prepare(STORAGE_SNAPSHOT)
        .bind(&[worker::wasm_bindgen::JsValue::from_str(&now.to_string())])?
        .first(None)
        .await
}
