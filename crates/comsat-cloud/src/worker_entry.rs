#![allow(
    unused_must_use,
    reason = "worker event macro expansion reports a must-use result for scheduled handlers"
)]
#![allow(
    clippy::future_not_send,
    reason = "Cloudflare Workers run wasm futures on an isolate event loop; worker bindings are non-Send by design."
)]

use std::sync::Arc;

use comsat_store::{Delivery, StoreError};
use worker::{
    Context, Env, Message, MessageBatch, MessageExt, Request, Response, Result, ScheduleContext,
    console_error, event,
};

use crate::{
    D1Store,
    cloud_notifications::{DeliveryRequest, TenantWebhookConfig, delivery_lease_owner},
    runtime::{
        WatchClaimConfig, WatchClaimMessage, enqueue_due_watches_for_tenants, route_request,
        run_claimed_watch, tenant_ids_from_env,
    },
    source_graph::source_catalog,
};

const MIN_WATCH_LEASE_SECONDS: u64 = 300;
const MAX_WATCH_CLAIM_LIMIT: usize = 12;

#[event(fetch)]
async fn fetch(request: Request, env: Env, _context: Context) -> Result<Response> {
    let store = Arc::new(D1Store::new(env.d1("COMSAT_DB")?));
    let sources = source_catalog(&env);
    let result = route_request(request, env, store, sources.clone()).await;
    crate::telemetry::log_catalog(&sources);
    if result.is_err() {
        worker::console_error!("{}", serde_json::json!({"event": "worker_request_failure"}));
    }
    result
}

#[event(scheduled)]
async fn scheduled(
    _event: worker::ScheduledEvent,
    env: Env,
    _context: ScheduleContext,
) -> Result<()> {
    let store = Arc::new(D1Store::new(env.d1("COMSAT_DB")?));
    let tenant_ids = tenant_ids_from_env(&env)?;
    let notifications = TenantWebhookConfig::from_env(&env, &tenant_ids).map_err(worker_error)?;
    let now = current_epoch_seconds();
    enqueue_watch_jobs(&env, store.clone(), &tenant_ids, now).await?;
    retry_scheduled_deliveries(store, notifications.as_ref(), now).await;
    crate::telemetry::log_storage_snapshot(&env).await;
    Ok(())
}

#[event(queue)]
async fn queue(batch: MessageBatch<WatchClaimMessage>, env: Env, _context: Context) -> Result<()> {
    let store = Arc::new(D1Store::new(env.d1("COMSAT_DB")?));
    let catalog = source_catalog(&env);
    let tenant_ids = tenant_ids_from_env(&env)?;
    let notifications = TenantWebhookConfig::from_env(&env, &tenant_ids).map_err(worker_error)?;
    for message in batch.messages()? {
        process_watch_message(
            &message,
            store.clone(),
            catalog.clone(),
            notifications.as_ref(),
        )
        .await;
    }
    crate::telemetry::log_catalog(&catalog);
    Ok(())
}

async fn enqueue_watch_jobs(
    env: &Env,
    store: Arc<D1Store>,
    tenant_ids: &[String],
    now: i64,
) -> Result<()> {
    let queued = enqueue_due_watches_for_tenants(
        store.as_ref(),
        &env.queue("COMSAT_WATCH_QUEUE")?,
        tenant_ids,
        WatchClaimConfig {
            now_epoch_seconds: now,
            claim_limit: watch_claim_limit(env),
            lease_seconds: watch_lease_seconds(env),
        },
    )
    .await
    .map_err(worker_error)?;
    worker::console_log!(
        "{}",
        serde_json::json!({"event": "watch_jobs_enqueued", "count": queued})
    );
    Ok(())
}

async fn retry_scheduled_deliveries(
    store: Arc<D1Store>,
    notifications: Option<&TenantWebhookConfig>,
    now: i64,
) {
    let Some(notifications) = notifications else {
        return;
    };
    match notifications
        .deliver_scheduled(store, now, &current_epoch_seconds)
        .await
    {
        Ok(stats) => {
            log_notification_summary("scheduled", stats.configured_tenants, stats.attempted);
        }
        Err(error) => log_notification_failure(&error, false),
    }
}

async fn process_watch_message(
    message: &Message<WatchClaimMessage>,
    store: Arc<D1Store>,
    catalog: Arc<comsat_engine::SourceCatalog>,
    notifications: Option<&TenantWebhookConfig>,
) {
    let body = message.body().clone();
    let result = run_claimed_watch(
        store.as_ref(),
        catalog,
        body.clone(),
        current_epoch_seconds(),
        notifications,
    )
    .await;
    if let Err(error) = result {
        handle_watch_error(message, &error);
        return;
    }
    deliver_after_watch(message, store, notifications, &body.tenant_id).await;
}

async fn deliver_after_watch(
    message: &Message<WatchClaimMessage>,
    store: Arc<D1Store>,
    notifications: Option<&TenantWebhookConfig>,
    tenant_id: &str,
) {
    let Some(notifications) = notifications else {
        return;
    };
    let now = current_epoch_seconds();
    let request = DeliveryRequest {
        tenant_id,
        lease_owner: delivery_lease_owner(now),
        now_epoch_seconds: now,
        clock: &current_epoch_seconds,
    };
    match notifications.deliver_one(store, request).await {
        Ok(delivery) => log_notification_delivery("queue", delivery.as_ref()),
        Err(error) => handle_notification_error(message, &error),
    }
}

fn handle_watch_error(message: &Message<WatchClaimMessage>, error: &StoreError) {
    let retry = crate::queue_policy::should_retry(error);
    console_error!("watch queue message failed (retry={retry}): {error}");
    if retry {
        message.retry();
    } else {
        message.ack();
    }
}

fn handle_notification_error(message: &Message<WatchClaimMessage>, error: &StoreError) {
    let retry = crate::queue_policy::should_retry(error);
    log_notification_failure(error, retry);
    if retry {
        message.retry();
    }
}

fn log_notification_summary(scope: &str, configured_tenants: usize, attempted: usize) {
    worker::console_log!(
        "{}",
        serde_json::json!({
            "event": "notification_delivery",
            "scope": scope,
            "configured_tenants": configured_tenants,
            "attempted": attempted,
        })
    );
}

fn log_notification_delivery(scope: &str, delivery: Option<&Delivery>) {
    let Some(delivery) = delivery else {
        log_notification_summary(scope, 0, 0);
        return;
    };
    worker::console_log!(
        "{}",
        serde_json::json!({
            "event": "notification_delivery",
            "scope": scope,
            "attempted": 1,
            "status": delivery.status,
            "attempts": delivery.attempts,
        })
    );
}

fn log_notification_failure(error: &StoreError, retry: bool) {
    let class = match error {
        StoreError::Backend(_) => "backend",
        StoreError::Conflict(_) => "conflict",
        StoreError::NotFound(_) => "not_found",
        StoreError::Validation(_) => "validation",
    };
    console_error!(
        "{}",
        serde_json::json!({
            "event": "notification_storage_failure",
            "class": class,
            "retry": retry,
        })
    );
}

fn watch_claim_limit(env: &Env) -> usize {
    env_usize(env, "WATCH_CLAIM_LIMIT", MAX_WATCH_CLAIM_LIMIT).min(MAX_WATCH_CLAIM_LIMIT)
}

fn watch_lease_seconds(env: &Env) -> u64 {
    env_u64(env, "WATCH_LEASE_SECONDS", MIN_WATCH_LEASE_SECONDS).max(MIN_WATCH_LEASE_SECONDS)
}

fn env_usize(env: &Env, name: &str, default: usize) -> usize {
    env.var(name)
        .ok()
        .and_then(|value| value.to_string().parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_u64(env: &Env, name: &str, default: u64) -> u64 {
    env.var(name)
        .ok()
        .and_then(|value| value.to_string().parse::<u64>().ok())
        .unwrap_or(default)
}

fn worker_error(error: impl std::fmt::Display) -> worker::Error {
    worker::Error::RustError(error.to_string())
}

fn current_epoch_seconds() -> i64 {
    i64::try_from(worker::Date::now().as_millis() / 1000).unwrap_or(i64::MAX)
}
