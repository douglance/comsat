use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use async_trait::async_trait;
use comsat_source::{
    OperationKind, RecordStream, SourceProfile, SourceRunContext, SourceRuntime, source_commands,
};
use comsat_types::{ErrorClass, Query, Record, SourceError, Target};
use futures::executor::block_on;
use futures::future::pending;
use futures::stream;
use futures::{Future, StreamExt};
use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler, McpCommandOptions};
use incurs::output::CommandResult;
use serde_json::json;

use super::*;

struct CountingRuntime {
    records: Vec<Record>,
    calls: Arc<AtomicUsize>,
}

struct FailingRuntime {
    source: comsat_types::SourceId,
    class: ErrorClass,
}

struct PendingHandler {
    dropped: Arc<AtomicUsize>,
}

struct DropCounter {
    dropped: Arc<AtomicUsize>,
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl SourceRuntime for CountingRuntime {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(stream::iter(self.records.clone().into_iter().map(Ok)))
    }

    async fn fetch(
        &self,
        _target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, SourceError> {
        Ok(self.records[0].clone())
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        Box::pin(stream::empty())
    }
}

#[async_trait]
impl SourceRuntime for FailingRuntime {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        let error = SourceError::new(self.source.clone(), self.class, "boom");
        Box::pin(stream::once(async move { Err(error) }))
    }

    async fn fetch(
        &self,
        _target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, SourceError> {
        Err(SourceError::new(self.source.clone(), self.class, "boom"))
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        Box::pin(stream::empty())
    }
}

#[async_trait]
impl CommandHandler for PendingHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        let _drop_counter = DropCounter {
            dropped: Arc::clone(&self.dropped),
        };
        pending::<()>().await;
        unreachable!("pending command never completes")
    }
}

#[test]
fn duplicate_selected_sources_are_invoked_once_and_missing_diagnostics_are_deduped() {
    let calls = Arc::new(AtomicUsize::new(0));
    let source = counted_source("alpha", vec![record("alpha", "1")], Arc::clone(&calls));
    let catalog = SourceCatalog::try_new(vec![source]).expect("unique sources");
    let outcome = block_on(catalog.collect_search(
        SearchRequest {
            query: query(),
            sources: vec![
                "alpha".parse().expect("valid source id"),
                "missing".parse().expect("valid source id"),
                "alpha".parse().expect("valid source id"),
                "missing".parse().expect("valid source id"),
            ],
            strict: false,
        },
        EngineLimits::default(),
    ));

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(outcome.records.len(), 1);
    assert_eq!(outcome.diagnostics.len(), 1);
    assert_eq!(outcome.diagnostics[0].source.as_str(), "missing");

    let metrics = catalog.metrics();
    assert_eq!(metrics.sources["alpha"].search.requests, 1);
    assert_eq!(metrics.sources["alpha"].search.successes, 1);
    assert!(!metrics.sources.contains_key("missing"));
}

#[test]
fn source_error_diagnostic_uses_invoked_catalog_source() {
    let runtime: Arc<dyn SourceRuntime> = Arc::new(FailingRuntime {
        source: "web".parse().expect("valid source id"),
        class: ErrorClass::Upstream,
    });
    let source = catalog_from_runtime("alpha", runtime);
    let catalog = SourceCatalog::try_new(vec![source]).expect("unique sources");
    let outcome =
        block_on(catalog.collect_search(SearchRequest::all(query()), EngineLimits::default()));

    assert!(outcome.records.is_empty());
    assert_eq!(outcome.diagnostics.len(), 1);
    assert_eq!(outcome.diagnostics[0].source.as_str(), "alpha");

    let metrics = catalog.metrics();
    assert_eq!(metrics.sources["alpha"].search.requests, 1);
    assert_eq!(metrics.sources["alpha"].search.successes, 0);
    assert_eq!(metrics.sources["alpha"].search.failures, 1);
}

#[test]
fn rate_limit_source_error_is_counted_separately() {
    let runtime: Arc<dyn SourceRuntime> = Arc::new(FailingRuntime {
        source: "alpha".parse().expect("valid source id"),
        class: ErrorClass::RateLimit,
    });
    let source = catalog_from_runtime("alpha", runtime);
    let catalog = SourceCatalog::try_new(vec![source]).expect("unique sources");
    let outcome =
        block_on(catalog.collect_search(SearchRequest::all(query()), EngineLimits::default()));

    assert!(outcome.records.is_empty());
    let metrics = catalog.metrics();
    assert_eq!(metrics.sources["alpha"].search.failures, 1);
    assert_eq!(metrics.sources["alpha"].search.rate_limits, 1);
    assert_eq!(metrics.sources["alpha"].search.successes, 0);
}

#[test]
fn dropping_aggregate_stream_cancels_running_tool_call() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let source = pending_source("alpha", OperationKind::Search, Arc::clone(&dropped));
    let catalog = SourceCatalog::try_new(vec![source]).expect("unique sources");
    let mut stream =
        Box::pin(catalog.search_stream(SearchRequest::all(query()), EngineLimits::default()));

    let mut next = Box::pin(stream.next());
    let waker = futures::task::noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(matches!(next.as_mut().poll(&mut context), Poll::Pending));
    drop(next);
    drop(stream);

    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    let metrics = catalog.metrics();
    assert_eq!(metrics.sources["alpha"].search.requests, 1);
    assert_eq!(metrics.sources["alpha"].search.cancelled, 1);
    assert_eq!(metrics.sources["alpha"].search.successes, 0);
}

#[test]
fn dropping_fetch_future_cancels_running_tool_call() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let source = pending_source("alpha", OperationKind::Fetch, Arc::clone(&dropped));
    let catalog = SourceCatalog::try_new(vec![source]).expect("unique sources");
    let mut future = Box::pin(catalog.fetch(
        Target::Url {
            source: "alpha".parse().expect("valid source id"),
            url: "https://example.com/alpha/1".to_string(),
        },
        EngineLimits::default(),
    ));

    let waker = futures::task::noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
    drop(future);

    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    let metrics = catalog.metrics();
    assert_eq!(metrics.sources["alpha"].fetch.requests, 1);
    assert_eq!(metrics.sources["alpha"].fetch.cancelled, 1);
    assert_eq!(metrics.sources["alpha"].fetch.successes, 0);
}

fn counted_source(source: &str, records: Vec<Record>, calls: Arc<AtomicUsize>) -> CatalogSource {
    let runtime: Arc<dyn SourceRuntime> = Arc::new(CountingRuntime { records, calls });
    catalog_from_runtime(source, runtime)
}

fn pending_source(
    source: &str,
    operation: OperationKind,
    dropped: Arc<AtomicUsize>,
) -> CatalogSource {
    let descriptor = descriptor(source);
    let command = CommandDef::build(operation.as_str(), PendingHandler { dropped })
        .description("Pending command")
        .mcp(McpCommandOptions {
            name: descriptor.tool_name(operation).map(ToOwned::to_owned),
            description: Some("Pending command".to_string()),
            ..McpCommandOptions::default()
        })
        .done();
    let cli = Cli::create(source).command(command.name.clone(), command);
    CatalogSource::new(descriptor, cli.tool_catalog())
}

fn catalog_from_runtime(source: &str, runtime: Arc<dyn SourceRuntime>) -> CatalogSource {
    let descriptor = descriptor(source);
    let mut cli = Cli::create(source);
    for command in source_commands(&descriptor, runtime) {
        cli = cli.command(command.name.clone(), command);
    }
    CatalogSource::new(descriptor, cli.tool_catalog())
}

fn descriptor(source: &str) -> comsat_source::SourceDescriptor {
    comsat_source::SourceDescriptor::new(
        source.parse().expect("valid source id"),
        source,
        SourceProfile {
            search: true,
            fetch: true,
            follow: true,
        },
    )
}

fn query() -> Query {
    Query {
        text: "oauth".to_string(),
        limit: None,
        since: None,
        until: None,
    }
}

fn record(source: &str, id: &str) -> Record {
    Record {
        id: format!("{source}:{id}").parse().expect("valid record id"),
        source: source.parse().expect("valid source id"),
        kind: "fixture".to_string(),
        url: format!("https://example.com/{source}/{id}"),
        title: Some(id.to_string()),
        text: None,
        author: None,
        created_at: None,
        updated_at: None,
        metadata: json!({}),
    }
}
