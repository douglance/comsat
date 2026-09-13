use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

use comsat_source::HttpClient;
use comsat_store::{
    ClaimPendingDelivery, CompleteDelivery, DELIVERY_RETRY_SECONDS, Store, StoreError, StoreResult,
    WatchRunDelivery,
};
use serde_json::Value;

const DELIVERY_LEASE_SECONDS: u64 = 60;

#[derive(Clone)]
pub struct WatchDeliveryConfig {
    url: String,
    auth_secret: Option<String>,
}

impl WatchDeliveryConfig {
    pub fn new(url: impl Into<String>, auth_secret: Option<String>) -> StoreResult<Self> {
        let url = url.into();
        validate_webhook_url(&url)?;
        if let Some(secret) = &auth_secret {
            http::HeaderValue::from_str(&format!("Bearer {secret}")).map_err(|_| {
                StoreError::Validation("invalid webhook authorization header".into())
            })?;
        }
        Ok(Self { url, auth_secret })
    }

    pub fn delivery_for_run(
        &self,
        tenant_id: &str,
        watch_id: &str,
        run_id: &str,
    ) -> WatchRunDelivery {
        WatchRunDelivery {
            delivery_id: delivery_id(tenant_id, watch_id, run_id),
            target: serde_json::json!({"type": "webhook", "url": self.url}),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeliveryAttempt<'a> {
    pub tenant_id: &'a str,
    pub lease_owner: &'a str,
    pub now_epoch_seconds: i64,
}

pub async fn deliver_pending_webhook(
    store: &Arc<dyn Store>,
    client: &Arc<dyn HttpClient>,
    config: &WatchDeliveryConfig,
    attempt: DeliveryAttempt<'_>,
    clock: &(dyn Fn() -> i64 + Send + Sync),
) -> StoreResult<Option<comsat_store::Delivery>> {
    let Some(delivery) = store
        .claim_pending_delivery(ClaimPendingDelivery {
            tenant_id: attempt.tenant_id.to_owned(),
            now_epoch_seconds: attempt.now_epoch_seconds,
            lease_owner: attempt.lease_owner.to_owned(),
            lease_seconds: DELIVERY_LEASE_SECONDS,
        })
        .await?
    else {
        return Ok(None);
    };
    let result = if delivery.attempts > comsat_store::MAX_DELIVERY_ATTEMPTS {
        Err("webhook retry limit exhausted".into())
    } else if delivery.target["url"].as_str() != Some(config.url.as_str()) {
        Err("configured webhook destination does not match queued delivery".into())
    } else {
        send_delivery(client, config, &delivery.delivery_id, &delivery.payload).await
    };
    store
        .complete_delivery(CompleteDelivery {
            tenant_id: delivery.tenant_id,
            delivery_id: delivery.delivery_id,
            lease_owner: attempt.lease_owner.to_owned(),
            finished_at_epoch_seconds: clock(),
            success: result.is_ok(),
            error: result.err(),
        })
        .await
        .map(Some)
}

fn delivery_id(tenant_id: &str, watch_id: &str, run_id: &str) -> String {
    let mut hasher = DefaultHasher::new();
    tenant_id.hash(&mut hasher);
    watch_id.hash(&mut hasher);
    run_id.hash(&mut hasher);
    format!("webhook:{:016x}", hasher.finish())
}

async fn send_delivery(
    client: &Arc<dyn HttpClient>,
    config: &WatchDeliveryConfig,
    delivery_id: &str,
    payload: &Value,
) -> Result<(), String> {
    let request =
        webhook_request(config, delivery_id, payload).map_err(|error| error.to_string())?;
    let response = client
        .send(request)
        .await
        .map_err(|error| format!("webhook request failed ({:?})", error.class))?;
    if response.status().is_success() {
        return Ok(());
    }
    Err(format!(
        "webhook returned HTTP {}",
        response.status().as_u16()
    ))
}

fn webhook_request(
    config: &WatchDeliveryConfig,
    delivery_id: &str,
    payload: &Value,
) -> StoreResult<http::Request<Vec<u8>>> {
    let body = serde_json::to_vec(payload)
        .map_err(|_| StoreError::Validation("webhook payload is not JSON".into()))?;
    if body.len() > 1024 * 1024 {
        return Err(StoreError::Validation(
            "webhook payload exceeds 1 MiB".into(),
        ));
    }
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(&config.url)
        .header("content-type", "application/json")
        .header("user-agent", "comsat-webhook")
        .header("idempotency-key", delivery_id)
        .header(
            "x-comsat-retry-after-seconds",
            DELIVERY_RETRY_SECONDS.to_string(),
        );
    if let Some(secret) = &config.auth_secret {
        builder = builder.header("authorization", format!("Bearer {secret}"));
    }
    builder
        .body(body)
        .map_err(|_| StoreError::Validation("invalid webhook request configuration".into()))
}

fn validate_webhook_url(value: &str) -> StoreResult<()> {
    let uri = value
        .parse::<http::Uri>()
        .map_err(|_| StoreError::Validation("webhook URL is invalid".into()))?;
    if uri.scheme_str() != Some("https") {
        return Err(StoreError::Validation("webhook URL must use https".into()));
    }
    let Some(authority) = uri.authority() else {
        return Err(StoreError::Validation("webhook URL has no host".into()));
    };
    let authority = authority.as_str();
    if authority.contains('@') {
        return Err(StoreError::Validation(
            "webhook URL must not contain credentials".into(),
        ));
    }
    validate_public_host(host_without_port(authority))
}

fn host_without_port(authority: &str) -> &str {
    if let Some(end) = authority.strip_prefix('[').and_then(|rest| rest.find(']')) {
        return &authority[1..=end];
    }
    authority.split(':').next().unwrap_or(authority)
}

fn validate_public_host(host: &str) -> StoreResult<()> {
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(StoreError::Validation(
            "webhook URL rejects local targets".into(),
        ));
    }
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return reject_private_ip(rejected_ipv4(ip));
    }
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<Ipv6Addr>() {
        return reject_private_ip(rejected_ipv6(ip));
    }
    Ok(())
}

fn reject_private_ip(rejected: bool) -> StoreResult<()> {
    if rejected {
        return Err(StoreError::Validation(
            "webhook URL rejects non-public IP targets".into(),
        ));
    }
    Ok(())
}

const fn rejected_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
}

const fn rejected_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return rejected_ipv4(mapped);
    }
    ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local() || ip.is_unicast_link_local()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_url_rejects_local_and_credential_targets() {
        for url in [
            "http://example.com/hook",
            "https://user:pass@example.com/hook",
            "https://localhost/hook",
            "https://service.localhost/hook",
            "https://127.0.0.1/hook",
            "https://[::1]/hook",
            "https://[::ffff:10.0.0.1]/hook",
        ] {
            assert!(WatchDeliveryConfig::new(url, None).is_err(), "{url}");
        }
    }

    #[test]
    fn delivery_identity_is_stable() {
        let config = WatchDeliveryConfig::new("https://example.com/hook", None).unwrap();
        let first = config.delivery_for_run("tenant", "watch", "run");
        let second = config.delivery_for_run("tenant", "watch", "run");
        assert_eq!(first.delivery_id, second.delivery_id);
    }

    #[tokio::test]
    async fn sender_preserves_batch_and_idempotency_without_exposing_transport_secrets() {
        let client: Arc<dyn HttpClient> = Arc::new(RejectingClient);
        let config = WatchDeliveryConfig::new(
            "https://example.com/hook",
            Some("private-test-token".into()),
        )
        .unwrap();
        let payload = serde_json::json!({"records": [{"id": "first"}, {"id": "second"}]});
        let error = send_delivery(&client, &config, "stable-delivery", &payload)
            .await
            .unwrap_err();
        assert_eq!(error, "webhook request failed (Upstream)");
        assert!(!error.contains("private-test-token"));
    }

    struct RejectingClient;

    #[async_trait::async_trait]
    impl HttpClient for RejectingClient {
        async fn send(
            &self,
            request: http::Request<Vec<u8>>,
        ) -> comsat_source::SourceResult<http::Response<Vec<u8>>> {
            assert_eq!(request.method(), http::Method::POST);
            assert_eq!(request.headers()["idempotency-key"], "stable-delivery");
            assert_eq!(
                request.headers()["authorization"],
                "Bearer private-test-token"
            );
            let payload: Value = serde_json::from_slice(request.body()).unwrap();
            assert_eq!(payload["records"].as_array().unwrap().len(), 2);
            Err(comsat_types::SourceError::new(
                comsat_types::SourceId::new("webhook").unwrap(),
                comsat_types::ErrorClass::Upstream,
                "transport accidentally included private-test-token",
            ))
        }
    }
}
