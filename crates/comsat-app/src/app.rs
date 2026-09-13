use std::{pin::Pin, sync::Arc};

use comsat_engine::{EngineLimits, SearchRequest, SourceCatalog};
use comsat_store::{
    ClaimDueWatch, ClaimedWatch, CompleteWatchRun, StartWatchRun, Store, WatchRunOutcome,
};
use comsat_types::Target;
use futures::Stream;

use crate::notifications::WatchDeliveryConfig;

#[derive(Clone)]
pub struct ComsatApp {
    pub(crate) catalog: Arc<SourceCatalog>,
    pub(crate) store: Option<Arc<dyn Store>>,
    pub(crate) limits: EngineLimits,
    pub(crate) lease_owner: String,
    pub(crate) tenant_id: String,
    pub(crate) target_stream_provider: Option<Arc<dyn TargetStreamProvider>>,
    delivery_config: Option<WatchDeliveryConfig>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl ComsatApp {
    pub fn new(
        catalog: Arc<SourceCatalog>,
        tenant_id: impl Into<String>,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            catalog,
            store: None,
            limits: EngineLimits::default(),
            lease_owner: "comsat-native".to_string(),
            tenant_id: tenant_id.into(),
            target_stream_provider: None,
            delivery_config: None,
            clock,
        }
    }

    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    #[must_use]
    pub const fn with_limits(mut self, limits: EngineLimits) -> Self {
        self.limits = limits;
        self
    }

    #[must_use]
    pub fn with_lease_owner(mut self, lease_owner: impl Into<String>) -> Self {
        self.lease_owner = lease_owner.into();
        self
    }

    #[must_use]
    pub fn with_target_stream_provider(mut self, provider: Arc<dyn TargetStreamProvider>) -> Self {
        self.target_stream_provider = Some(provider);
        self
    }

    #[must_use]
    pub fn with_delivery_config(mut self, config: WatchDeliveryConfig) -> Self {
        self.delivery_config = Some(config);
        self
    }

    #[must_use]
    pub fn without_target_stream_provider(mut self) -> Self {
        self.target_stream_provider = None;
        self
    }

    pub const fn catalog(&self) -> &Arc<SourceCatalog> {
        &self.catalog
    }

    pub async fn deliver_pending_once(
        &self,
        client: Arc<dyn comsat_source::HttpClient>,
    ) -> AppResult<Option<comsat_store::Delivery>> {
        let Some(config) = &self.delivery_config else {
            return Ok(None);
        };
        crate::notifications::deliver_pending_webhook(
            &self.store()?,
            &client,
            config,
            crate::notifications::DeliveryAttempt {
                tenant_id: &self.tenant_id,
                lease_owner: &format!("{}:{}", self.lease_owner, self.now()),
                now_epoch_seconds: self.now(),
            },
            self.clock.as_ref(),
        )
        .await
        .map_err(Into::into)
    }

    pub async fn run_due_watch_once(&self) -> AppResult<Option<WatchRunOutcome>> {
        let Some(store) = &self.store else {
            return Err(AppError::StoreUnavailable);
        };
        let now = self.now();
        let Some(claim) = self.claim_due_watch(store, now).await? else {
            return Ok(None);
        };

        let run_id = self.watch_run_id(&claim);
        self.start_watch_run(store, &claim, &run_id, now).await?;

        let outcome = self
            .catalog
            .collect_search(
                SearchRequest {
                    query: claim.watch.query.clone(),
                    sources: claim.watch.source_ids.clone(),
                    strict: false,
                },
                self.limits,
            )
            .await;
        self.complete_watch_run(store, claim, run_id, outcome).await
    }

    async fn claim_due_watch(
        &self,
        store: &Arc<dyn Store>,
        now: i64,
    ) -> AppResult<Option<ClaimedWatch>> {
        Ok(store
            .claim_due_watch(ClaimDueWatch {
                tenant_id: self.tenant_id.clone(),
                now_epoch_seconds: now,
                lease_owner: self.lease_owner.clone(),
                lease_seconds: 60,
            })
            .await?)
    }

    fn watch_run_id(&self, claim: &ClaimedWatch) -> String {
        format!(
            "{}:{}:{}",
            claim.watch.watch_id, self.lease_owner, claim.watch.next_due_epoch_seconds
        )
    }

    async fn start_watch_run(
        &self,
        store: &Arc<dyn Store>,
        claim: &ClaimedWatch,
        run_id: &str,
        now: i64,
    ) -> AppResult<()> {
        store
            .start_watch_run(StartWatchRun {
                tenant_id: claim.watch.tenant_id.clone(),
                watch_id: claim.watch.watch_id.clone(),
                run_id: run_id.to_string(),
                idempotency_key: run_id.to_string(),
                lease_owner: self.lease_owner.clone(),
                started_at_epoch_seconds: now,
            })
            .await?;
        Ok(())
    }

    async fn complete_watch_run(
        &self,
        store: &Arc<dyn Store>,
        claim: ClaimedWatch,
        run_id: String,
        outcome: comsat_engine::SearchOutcome,
    ) -> AppResult<Option<WatchRunOutcome>> {
        let tenant_id = claim.watch.tenant_id;
        let watch_id = claim.watch.watch_id;
        let finished_at = self.now();
        let completed = store
            .complete_watch_run(CompleteWatchRun {
                tenant_id: tenant_id.clone(),
                watch_id: watch_id.clone(),
                run_id: run_id.clone(),
                lease_owner: self.lease_owner.clone(),
                finished_at_epoch_seconds: finished_at,
                records: outcome.records,
                next_cursor: claim.watch.cursor.clone(),
                error: diagnostics_error(&outcome.diagnostics),
                delivery: self
                    .delivery_config
                    .as_ref()
                    .map(|config| config.delivery_for_run(&tenant_id, &watch_id, &run_id)),
            })
            .await?;
        Ok(Some(completed))
    }

    pub(crate) fn store(&self) -> AppResult<Arc<dyn Store>> {
        self.store.clone().ok_or(AppError::StoreUnavailable)
    }

    pub(crate) fn now(&self) -> i64 {
        (self.clock)()
    }
}

pub type AppResult<T> = Result<T, AppError>;
pub type TargetStream = Pin<Box<dyn Stream<Item = Result<Target, String>> + Send>>;

pub trait TargetStreamProvider: Send + Sync {
    fn read_targets(&self) -> TargetStream;
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("COMSAT persistence is not configured")]
    StoreUnavailable,
    #[error("{0}")]
    Store(#[from] comsat_store::StoreError),
}

fn diagnostics_error(diagnostics: &[comsat_engine::EngineDiagnostic]) -> Option<String> {
    (!diagnostics.is_empty()).then(|| {
        diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.source, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ")
    })
}
