#![allow(
    clippy::future_not_send,
    reason = "Cloudflare Worker bindings and fetch futures are non-Send on the isolate event loop."
)]

use std::{collections::BTreeMap, sync::Arc};

use comsat_app::notifications::{DeliveryAttempt, WatchDeliveryConfig, deliver_pending_webhook};
use comsat_source::HttpClient;
use comsat_store::{Delivery, Store, StoreError, StoreResult};
use comsat_types::SourceId;
use serde::Deserialize;
use worker::Env;

use crate::{cloud_http::CloudflareHttpClient, tenant_rotation::scheduled_tenants};

const NOTIFICATION_TENANT_LIMIT: usize = 4;

#[derive(Clone)]
pub struct TenantWebhookConfig {
    configs: BTreeMap<String, WatchDeliveryConfig>,
    client: Arc<dyn HttpClient>,
}

impl TenantWebhookConfig {
    pub fn from_env(env: &Env, authorized_tenants: &[String]) -> StoreResult<Option<Self>> {
        let Some(raw) = webhook_secret(env) else {
            return Ok(None);
        };
        let entries = parse_entries(&raw)?;
        let configs = tenant_configs(entries, authorized_tenants)?;
        if configs.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            configs,
            client: Arc::new(CloudflareHttpClient::new(webhook_source_id()?)),
        }))
    }

    pub fn delivery_for_run(
        &self,
        tenant_id: &str,
        watch_id: &str,
        run_id: &str,
    ) -> Option<comsat_store::WatchRunDelivery> {
        self.configs
            .get(tenant_id)
            .map(|config| config.delivery_for_run(tenant_id, watch_id, run_id))
    }

    pub async fn deliver_one<S>(
        &self,
        store: Arc<S>,
        request: DeliveryRequest<'_>,
    ) -> StoreResult<Option<Delivery>>
    where
        S: Store + 'static,
    {
        let Some(config) = self.configs.get(request.tenant_id) else {
            return Ok(None);
        };
        let store: Arc<dyn Store> = store;
        deliver_pending_webhook(
            &store,
            &self.client,
            config,
            DeliveryAttempt {
                tenant_id: request.tenant_id,
                lease_owner: &request.lease_owner,
                now_epoch_seconds: request.now_epoch_seconds,
            },
            request.clock,
        )
        .await
    }

    pub async fn deliver_scheduled<S>(
        &self,
        store: Arc<S>,
        now_epoch_seconds: i64,
        clock: &(dyn Fn() -> i64 + Send + Sync),
    ) -> StoreResult<DeliveryStats>
    where
        S: Store + 'static,
    {
        let mut stats = DeliveryStats::default();
        let configured_tenants = self.configs.keys().cloned().collect::<Vec<_>>();
        for tenant_id in scheduled_tenants(
            &configured_tenants,
            now_epoch_seconds,
            NOTIFICATION_TENANT_LIMIT,
        ) {
            if !self.configs.contains_key(tenant_id) {
                continue;
            }
            stats.configured_tenants += 1;
            if self
                .deliver_one(
                    store.clone(),
                    DeliveryRequest {
                        tenant_id,
                        lease_owner: delivery_lease_owner(now_epoch_seconds),
                        now_epoch_seconds,
                        clock,
                    },
                )
                .await?
                .is_some()
            {
                stats.attempted += 1;
            }
        }
        Ok(stats)
    }
}

pub struct DeliveryRequest<'a> {
    pub tenant_id: &'a str,
    pub lease_owner: String,
    pub now_epoch_seconds: i64,
    pub clock: &'a (dyn Fn() -> i64 + Send + Sync),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DeliveryStats {
    pub configured_tenants: usize,
    pub attempted: usize,
}

#[derive(Deserialize)]
struct WebhookEntry {
    url: String,
    token: Option<String>,
}

fn tenant_configs(
    entries: BTreeMap<String, WebhookEntry>,
    authorized_tenants: &[String],
) -> StoreResult<BTreeMap<String, WatchDeliveryConfig>> {
    let mut configs = BTreeMap::new();
    for (tenant_id, entry) in entries {
        ensure_authorized(&tenant_id, authorized_tenants)?;
        configs.insert(
            tenant_id,
            WatchDeliveryConfig::new(entry.url, entry.token.filter(|token| !token.is_empty()))?,
        );
    }
    Ok(configs)
}

fn ensure_authorized(tenant_id: &str, authorized_tenants: &[String]) -> StoreResult<()> {
    if authorized_tenants
        .iter()
        .any(|authorized| authorized == tenant_id)
    {
        return Ok(());
    }
    Err(StoreError::Validation(
        "webhook tenant is not authorized".into(),
    ))
}

fn parse_entries(raw: &str) -> StoreResult<BTreeMap<String, WebhookEntry>> {
    serde_json::from_str(raw)
        .map_err(|_| StoreError::Validation("webhook configuration is invalid".into()))
}

fn webhook_secret(env: &Env) -> Option<String> {
    env.secret("TENANT_WEBHOOKS_JSON")
        .ok()
        .map(|secret| secret.to_string())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env.var("TENANT_WEBHOOKS_JSON")
                .ok()
                .map(|value| value.to_string())
                .filter(|value| !value.trim().is_empty())
        })
}

fn webhook_source_id() -> StoreResult<SourceId> {
    SourceId::new("webhook").map_err(|error| StoreError::Validation(error.to_string()))
}

pub fn delivery_lease_owner(now_epoch_seconds: i64) -> String {
    let nonce = worker::js_sys::Math::random().to_bits();
    format!("cloud-delivery:{now_epoch_seconds}:{nonce:x}")
}
