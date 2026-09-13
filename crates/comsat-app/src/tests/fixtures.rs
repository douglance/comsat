use std::sync::Arc;

use async_trait::async_trait;
use comsat_engine::SourceCatalog;
use comsat_source::{
    RecordStream, SourceDescriptor, SourceProfile, SourceRunContext, SourceRuntime, source_commands,
};
use comsat_store::{
    ClaimDueWatch, ClaimPendingDelivery, CompleteDelivery, CompleteWatchRun, CreateWatch,
    DeleteWatch, Delivery, HistoryRequest, ListDeliveries, RecordObservation, StartWatchRun, Store,
    Watch, WatchRunOutcome, empty_object,
};
use comsat_types::{ErrorClass, Query, Record, SourceError, SourceId, Target};
use incurs::cli::Cli;
use incurs_codemode::{ReplayPolicy, ToolAnnotations, ToolOrigin, ToolPolicy, ToolPolicyResolver};
use serde_json::json;

use crate::ComsatApp;

pub(super) struct NoApprovalPolicy;

impl ToolPolicyResolver for NoApprovalPolicy {
    fn resolve(&self, _origin: ToolOrigin, _annotations: &ToolAnnotations) -> ToolPolicy {
        ToolPolicy {
            requires_approval: false,
            replay: ReplayPolicy::Reexecute,
        }
    }
}

pub(super) async fn create_fixture_watch(app: &ComsatApp) {
    let watch = app
        .store()
        .unwrap()
        .create_watch(CreateWatch {
            tenant_id: "default".into(),
            watch_id: "fixture-watch".into(),
            query: Query {
                text: "MCP OAuth".into(),
                limit: Some(1),
                since: None,
                until: None,
            },
            source_ids: vec![SourceId::new("fixture").unwrap()],
            interval_seconds: 1,
            cursor: empty_object(),
            enabled: true,
            created_at_epoch_seconds: 1,
            first_due_epoch_seconds: Some(1),
        })
        .await
        .unwrap();
    assert_eq!(watch.watch_id, "fixture-watch");
}

pub(super) fn fixture_catalog() -> Arc<SourceCatalog> {
    Arc::new(SourceCatalog::try_new(vec![fixture_catalog_source()]).expect("fixture source"))
}

pub(super) fn strict_fixture_catalog() -> Arc<SourceCatalog> {
    Arc::new(
        SourceCatalog::try_new(vec![fixture_catalog_source(), failing_catalog_source()])
            .expect("strict fixture sources"),
    )
}

pub(super) fn class_failure_catalog() -> Arc<SourceCatalog> {
    Arc::new(
        SourceCatalog::try_new(vec![
            class_failing_catalog_source("rate-limited", ErrorClass::RateLimit),
            class_failing_catalog_source("auth-failing", ErrorClass::Authentication),
        ])
        .expect("class failure sources"),
    )
}

pub(super) fn fixture_catalog_source() -> comsat_engine::CatalogSource {
    let descriptor = SourceDescriptor::new(
        SourceId::new("fixture").unwrap(),
        "Fixture",
        SourceProfile {
            search: true,
            fetch: true,
            follow: true,
        },
    );
    let runtime: Arc<dyn SourceRuntime> = Arc::new(FixtureSource);
    let mut cli = Cli::create("fixture");
    for command in source_commands(&descriptor, runtime) {
        cli = cli.command(command.name.clone(), command);
    }
    comsat_engine::CatalogSource::new(descriptor, cli.tool_catalog())
}

fn failing_catalog_source() -> comsat_engine::CatalogSource {
    let source = SourceId::new("failing").unwrap();
    catalog_source_with_runtime("Failing", source, Arc::new(FailingSource))
}

fn class_failing_catalog_source(id: &str, class: ErrorClass) -> comsat_engine::CatalogSource {
    let source = SourceId::new(id).unwrap();
    catalog_source_with_runtime(
        id,
        source.clone(),
        Arc::new(ClassFailingSource { source, class }),
    )
}

fn catalog_source_with_runtime(
    name: &str,
    source: SourceId,
    runtime: Arc<dyn SourceRuntime>,
) -> comsat_engine::CatalogSource {
    let descriptor = SourceDescriptor::new(
        source,
        name,
        SourceProfile {
            search: true,
            fetch: false,
            follow: true,
        },
    );
    let mut cli = Cli::create(name);
    for command in source_commands(&descriptor, runtime) {
        cli = cli.command(command.name.clone(), command);
    }
    comsat_engine::CatalogSource::new(descriptor, cli.tool_catalog())
}

#[derive(Clone)]
struct FixtureSource;

#[derive(Clone)]
struct FailingSource;

#[derive(Clone)]
struct ClassFailingSource {
    source: SourceId,
    class: ErrorClass,
}

#[async_trait]
impl SourceRuntime for FixtureSource {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        Box::pin(async_stream::stream! {
            yield Ok(record("fixture:search"));
        })
    }

    async fn fetch(
        &self,
        target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, comsat_types::SourceError> {
        let id = match target {
            Target::Native { id, .. } => id,
            _ => "record".to_string(),
        };
        Ok(record(&format!("fixture:fetch:{id}")))
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        Box::pin(async_stream::stream! {
            yield Ok(record("fixture:follow"));
        })
    }
}

#[async_trait]
impl SourceRuntime for ClassFailingSource {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        let error = class_error(self.source.clone(), self.class);
        Box::pin(async_stream::stream! {
            yield Err(error);
        })
    }

    async fn fetch(
        &self,
        _target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, comsat_types::SourceError> {
        Err(class_error(self.source.clone(), self.class))
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        let error = class_error(self.source.clone(), self.class);
        Box::pin(async_stream::stream! {
            yield Err(error);
        })
    }
}

fn class_error(source: SourceId, class: ErrorClass) -> SourceError {
    let mut error = SourceError::new(source, class, "fixture failure");
    if class == ErrorClass::RateLimit {
        error.retry_after_seconds = Some(60);
    }
    error
}

#[async_trait]
impl SourceRuntime for FailingSource {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        Box::pin(async_stream::stream! {
            yield Err(comsat_types::SourceError::upstream(
                SourceId::new("failing").unwrap(),
                "fixture failure",
            ));
        })
    }

    async fn fetch(
        &self,
        _target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, comsat_types::SourceError> {
        Err(comsat_types::SourceError::unsupported(
            SourceId::new("failing").unwrap(),
            "fetch unsupported",
        ))
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        Box::pin(async_stream::stream! {
            yield Err(comsat_types::SourceError::unsupported(
                SourceId::new("failing").unwrap(),
                "follow unsupported",
            ));
        })
    }
}

pub(super) fn record(id: &str) -> Record {
    Record {
        id: comsat_types::RecordId::new(id).unwrap(),
        source: SourceId::new("fixture").unwrap(),
        kind: "item".into(),
        url: "https://example.com/item".into(),
        title: Some("Fixture".into()),
        text: Some("Fixture text".into()),
        author: Some("fixture".into()),
        created_at: None,
        updated_at: None,
        metadata: json!({}),
    }
}

#[derive(Default)]
pub(super) struct FixtureStore {
    watches: tokio::sync::Mutex<Vec<Watch>>,
    records: tokio::sync::Mutex<Vec<RecordObservation>>,
    deliveries: tokio::sync::Mutex<Vec<Delivery>>,
}

#[async_trait]
impl Store for FixtureStore {
    async fn upsert_source(
        &self,
        _request: comsat_store::UpsertSource,
    ) -> comsat_store::StoreResult<()> {
        Ok(())
    }

    async fn create_watch(&self, request: CreateWatch) -> comsat_store::StoreResult<Watch> {
        let watch = request.into_watch();
        self.watches.lock().await.push(watch.clone());
        Ok(watch)
    }

    async fn list_watches(&self, _tenant_id: &str) -> comsat_store::StoreResult<Vec<Watch>> {
        Ok(self.watches.lock().await.clone())
    }

    async fn delete_watch(&self, request: DeleteWatch) -> comsat_store::StoreResult<bool> {
        let mut watches = self.watches.lock().await;
        let before = watches.len();
        watches.retain(|watch| watch.watch_id != request.watch_id);
        Ok(before != watches.len())
    }

    async fn claim_due_watch(
        &self,
        request: ClaimDueWatch,
    ) -> comsat_store::StoreResult<Option<comsat_store::ClaimedWatch>> {
        let mut watches = self.watches.lock().await;
        let Some(watch) = watches.iter_mut().find(|watch| {
            watch.tenant_id == request.tenant_id
                && watch.next_due_epoch_seconds <= request.now_epoch_seconds
        }) else {
            return Ok(None);
        };
        watch.lease_owner = Some(request.lease_owner.clone());
        watch.lease_expires_epoch_seconds =
            Some(request.now_epoch_seconds + request.lease_seconds as i64);
        Ok(Some(comsat_store::ClaimedWatch {
            watch: watch.clone(),
            lease_owner: request.lease_owner,
            lease_expires_epoch_seconds: request.now_epoch_seconds + request.lease_seconds as i64,
        }))
    }

    async fn start_watch_run(
        &self,
        request: StartWatchRun,
    ) -> comsat_store::StoreResult<comsat_store::WatchRun> {
        Ok(comsat_store::WatchRun {
            tenant_id: request.tenant_id,
            watch_id: request.watch_id,
            run_id: request.run_id,
            idempotency_key: request.idempotency_key,
            lease_owner: request.lease_owner,
            status: "running".into(),
            started_at_epoch_seconds: request.started_at_epoch_seconds,
            finished_at_epoch_seconds: None,
            error: None,
        })
    }

    async fn complete_watch_run(
        &self,
        request: CompleteWatchRun,
    ) -> comsat_store::StoreResult<WatchRunOutcome> {
        let records_seen = request.records.len();
        let mut observations = self.records.lock().await;
        for record in request.records {
            observations.push(RecordObservation {
                record,
                watch_id: Some(request.watch_id.clone()),
                first_observed_epoch_seconds: request.finished_at_epoch_seconds,
                last_observed_epoch_seconds: request.finished_at_epoch_seconds,
            });
        }
        Ok(WatchRunOutcome {
            run_id: request.run_id,
            status: "succeeded".into(),
            records_seen,
            records_inserted: records_seen,
            watch_records_inserted: records_seen,
        })
    }

    async fn history(
        &self,
        request: HistoryRequest,
    ) -> comsat_store::StoreResult<Vec<RecordObservation>> {
        Ok(self
            .records
            .lock()
            .await
            .iter()
            .filter(|record| {
                request
                    .watch_id
                    .as_ref()
                    .is_none_or(|watch_id| record.watch_id.as_ref() == Some(watch_id))
            })
            .cloned()
            .collect())
    }

    async fn claim_pending_delivery(
        &self,
        request: ClaimPendingDelivery,
    ) -> comsat_store::StoreResult<Option<Delivery>> {
        let mut deliveries = self.deliveries.lock().await;
        let Some(delivery) = deliveries.iter_mut().find(|delivery| {
            delivery.tenant_id == request.tenant_id
                && delivery.status == "pending"
                && delivery.next_attempt_epoch_seconds <= request.now_epoch_seconds
        }) else {
            return Ok(None);
        };
        delivery.status = "delivering".into();
        delivery.attempts += 1;
        delivery.lease_owner = Some(request.lease_owner.clone());
        delivery.lease_expires_epoch_seconds =
            Some(request.now_epoch_seconds + request.lease_seconds as i64);
        Ok(Some(delivery.clone()))
    }

    async fn complete_delivery(
        &self,
        request: CompleteDelivery,
    ) -> comsat_store::StoreResult<Delivery> {
        let mut deliveries = self.deliveries.lock().await;
        let Some(delivery) = deliveries.iter_mut().find(|delivery| {
            delivery.tenant_id == request.tenant_id
                && delivery.delivery_id == request.delivery_id
                && delivery.lease_owner.as_ref() == Some(&request.lease_owner)
        }) else {
            return Err(comsat_store::StoreError::Conflict(
                "delivery is not leased by this worker".into(),
            ));
        };
        delivery.status = if request.success {
            "succeeded".into()
        } else if delivery.attempts >= comsat_store::MAX_DELIVERY_ATTEMPTS {
            "failed".into()
        } else {
            "pending".into()
        };
        delivery.error = request.error;
        delivery.delivered_at_epoch_seconds =
            request.success.then_some(request.finished_at_epoch_seconds);
        delivery.lease_owner = None;
        delivery.lease_expires_epoch_seconds = None;
        Ok(delivery.clone())
    }

    async fn list_deliveries(
        &self,
        request: ListDeliveries,
    ) -> comsat_store::StoreResult<Vec<Delivery>> {
        Ok(self
            .deliveries
            .lock()
            .await
            .iter()
            .filter(|delivery| {
                delivery.tenant_id == request.tenant_id
                    && request
                        .status
                        .as_ref()
                        .is_none_or(|status| &delivery.status == status)
            })
            .cloned()
            .collect())
    }
}
