use std::{env, sync::Arc};

use comsat_app::{ComsatApp, notifications::WatchDeliveryConfig};
use comsat_store::StoreResult;

pub fn configure(app: ComsatApp) -> StoreResult<ComsatApp> {
    let Ok(url) = env::var("COMSAT_WEBHOOK_URL") else {
        return Ok(app);
    };
    let config = WatchDeliveryConfig::new(url, env::var("COMSAT_WEBHOOK_TOKEN").ok())?;
    Ok(app.with_delivery_config(config))
}

pub async fn deliver_pending(app: &ComsatApp) {
    let client = Arc::new(crate::http_client::SecureHttpClient::new());
    match app.deliver_pending_once(client).await {
        Ok(Some(delivery)) => eprintln!(
            "{}",
            serde_json::json!({
                "event": "notification_delivery",
                "delivery_id": delivery.delivery_id,
                "status": delivery.status,
                "attempts": delivery.attempts
            })
        ),
        Ok(None) => {}
        Err(_) => eprintln!(
            "{}",
            serde_json::json!({"event": "notification_storage_failure"})
        ),
    }
}
