#![allow(
    clippy::future_not_send,
    reason = "Cloudflare queue bindings are non-Send on the isolate event loop."
)]

use std::sync::Arc;

use comsat_engine::{EngineDiagnostic, SearchRequest, SourceCatalog};
use comsat_store::{ClaimDueWatch, CompleteWatchRun, StartWatchRun, Store, StoreResult};
use comsat_types::{Query, SourceId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{cloud_config::cloud_limits, cloud_notifications::TenantWebhookConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchClaimMessage {
    pub tenant_id: String,
    pub watch_id: String,
    pub query: Query,
    pub source_ids: Vec<SourceId>,
    pub cursor: Value,
    pub lease_owner: String,
    pub lease_expires_epoch_seconds: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct WatchClaimConfig {
    pub now_epoch_seconds: i64,
    pub claim_limit: usize,
    pub lease_seconds: u64,
}

pub async fn enqueue_due_watches<S>(
    store: &S,
    queue: &worker::Queue,
    tenant_id: &str,
    config: WatchClaimConfig,
) -> StoreResult<usize>
where
    S: Store,
{
    let mut count = 0;
    while count < config.claim_limit {
        let claim = claim_request(tenant_id, config, count);
        let Some(claimed) = store.claim_due_watch(claim).await? else {
            break;
        };
        queue
            .send(WatchClaimMessage {
                tenant_id: claimed.watch.tenant_id,
                watch_id: claimed.watch.watch_id,
                query: claimed.watch.query,
                source_ids: claimed.watch.source_ids,
                cursor: claimed.watch.cursor,
                lease_owner: claimed.lease_owner,
                lease_expires_epoch_seconds: claimed.lease_expires_epoch_seconds,
            })
            .await
            .map_err(store_backend)?;
        count += 1;
    }
    Ok(count)
}

pub async fn enqueue_due_watches_for_tenants<S>(
    store: &S,
    queue: &worker::Queue,
    tenant_ids: &[String],
    config: WatchClaimConfig,
) -> StoreResult<usize>
where
    S: Store,
{
    let mut total = 0;
    for tenant_id in crate::tenant_rotation::scheduled_tenants(
        tenant_ids,
        config.now_epoch_seconds,
        config.claim_limit,
    ) {
        if total >= config.claim_limit {
            break;
        }
        let tenant_config = WatchClaimConfig {
            claim_limit: config.claim_limit - total,
            ..config
        };
        total += enqueue_due_watches(store, queue, tenant_id, tenant_config).await?;
    }
    Ok(total)
}

pub async fn run_claimed_watch<S>(
    store: &S,
    catalog: Arc<SourceCatalog>,
    message: WatchClaimMessage,
    now_epoch_seconds: i64,
    delivery_config: Option<&TenantWebhookConfig>,
) -> StoreResult<()>
where
    S: Store,
{
    let run = store
        .start_watch_run(start_request(&message, now_epoch_seconds))
        .await?;
    if run.status != "running" {
        return Ok(());
    }
    let outcome = catalog
        .collect_search(search_request(&message), cloud_limits())
        .await;
    let finished_at_epoch_seconds = current_epoch_seconds();
    let delivery = delivery_config.and_then(|config| {
        config.delivery_for_run(&message.tenant_id, &message.watch_id, &run.run_id)
    });
    store
        .complete_watch_run(CompleteWatchRun {
            tenant_id: message.tenant_id,
            watch_id: message.watch_id,
            run_id: run.run_id,
            lease_owner: message.lease_owner,
            finished_at_epoch_seconds,
            records: outcome.records,
            next_cursor: message.cursor,
            error: diagnostics_error(&outcome.diagnostics),
            delivery,
        })
        .await?;
    Ok(())
}

fn claim_request(tenant_id: &str, config: WatchClaimConfig, count: usize) -> ClaimDueWatch {
    ClaimDueWatch {
        tenant_id: tenant_id.to_string(),
        now_epoch_seconds: config.now_epoch_seconds,
        lease_owner: lease_owner(config.now_epoch_seconds, count),
        lease_seconds: config.lease_seconds,
    }
}

fn start_request(message: &WatchClaimMessage, now_epoch_seconds: i64) -> StartWatchRun {
    StartWatchRun {
        tenant_id: message.tenant_id.clone(),
        watch_id: message.watch_id.clone(),
        run_id: run_id(message),
        idempotency_key: message.lease_owner.clone(),
        lease_owner: message.lease_owner.clone(),
        started_at_epoch_seconds: now_epoch_seconds,
    }
}

fn search_request(message: &WatchClaimMessage) -> SearchRequest {
    SearchRequest {
        query: message.query.clone(),
        sources: message.source_ids.clone(),
        strict: false,
    }
}

fn run_id(message: &WatchClaimMessage) -> String {
    format!("{}:{}", message.watch_id, message.lease_owner)
}

fn lease_owner(now_epoch_seconds: i64, count: usize) -> String {
    let nonce = worker::js_sys::Math::random().to_bits();
    format!("cloud:{now_epoch_seconds}:{nonce:x}:{count}")
}

fn current_epoch_seconds() -> i64 {
    i64::try_from(worker::Date::now().as_millis() / 1000).unwrap_or(i64::MAX)
}

fn diagnostics_error(diagnostics: &[EngineDiagnostic]) -> Option<String> {
    if diagnostics.is_empty() {
        return None;
    }
    Some(
        diagnostics
            .iter()
            .map(|diagnostic| {
                format!(
                    "{}:{:?}:{}",
                    diagnostic.source, diagnostic.class, diagnostic.message
                )
            })
            .collect::<Vec<_>>()
            .join("; "),
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "worker queue send returns an owned error and map_err requires this callback shape."
)]
fn store_backend(error: worker::Error) -> comsat_store::StoreError {
    comsat_store::StoreError::Backend(error.to_string())
}
