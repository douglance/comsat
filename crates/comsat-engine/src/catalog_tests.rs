use std::sync::Arc;

use async_trait::async_trait;
use comsat_source::{
    OperationKind, RecordStream, SourceProfile, SourceRunContext, SourceRuntime, source_commands,
};
use comsat_types::{ErrorClass, Query, Record, SourceError, SourceId, Target};
use futures::channel::oneshot;
use futures::executor::block_on;
use futures::stream;
use futures::{FutureExt, StreamExt};
use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler, McpCommandOptions};
use incurs::output::CommandResult;
use serde_json::json;

use super::*;

#[derive(Clone)]
struct StaticRuntime {
    source: SourceId,
    records: Vec<Record>,
}

struct GatedRuntime {
    first: Record,
    second: Record,
    gate: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

struct DelayedRuntime {
    record: Record,
    gate: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

struct FailingRuntime {
    source: SourceId,
}

struct FinalDataHandler {
    records: Vec<Record>,
}

#[async_trait]
impl SourceRuntime for StaticRuntime {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        let records = self.records.clone().into_iter().map(Ok);
        Box::pin(stream::iter(records))
    }

    async fn fetch(
        &self,
        target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, SourceError> {
        self.records
            .iter()
            .find(|record| record.source == *target.source())
            .cloned()
            .ok_or_else(|| {
                SourceError::new(
                    self.source.clone(),
                    ErrorClass::NotFound,
                    "record not found",
                )
            })
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        let records = self.records.clone().into_iter().map(Ok);
        Box::pin(stream::iter(records))
    }
}

#[async_trait]
impl SourceRuntime for GatedRuntime {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        let first = self.first.clone();
        let second = self.second.clone();
        let gate = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("gate used once");
        let stream = stream::once(async move { Ok(first) }).chain(stream::once(async move {
            let _ = gate.await;
            Ok(second)
        }));
        Box::pin(stream)
    }

    async fn fetch(
        &self,
        _target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, SourceError> {
        Ok(self.first.clone())
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        Box::pin(stream::empty())
    }
}

#[async_trait]
impl SourceRuntime for DelayedRuntime {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        let record = self.record.clone();
        let gate = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("gate used once");
        Box::pin(stream::once(async move {
            let _ = gate.await;
            Ok(record)
        }))
    }

    async fn fetch(
        &self,
        _target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, SourceError> {
        Ok(self.record.clone())
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        Box::pin(stream::empty())
    }
}

#[async_trait]
impl SourceRuntime for FailingRuntime {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        let error = SourceError::new(self.source.clone(), ErrorClass::Upstream, "boom");
        Box::pin(stream::once(async move { Err(error) }))
    }

    async fn fetch(
        &self,
        _target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, SourceError> {
        Err(SourceError::new(
            self.source.clone(),
            ErrorClass::Upstream,
            "boom",
        ))
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        Box::pin(stream::empty())
    }
}

#[async_trait]
impl CommandHandler for FinalDataHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: serde_json::to_value(&self.records).expect("records serialize"),
            cta: None,
            exit_code: None,
        }
    }
}

#[test]
fn collect_search_runs_without_tokio_runtime() {
    let source = catalog_source("alpha", vec![record("alpha", "1")]);
    let catalog = SourceCatalog::try_new(vec![source]).expect("unique sources");
    let outcome = block_on(catalog.collect_search(
        SearchRequest::all(Query {
            text: "oauth".to_string(),
            limit: None,
            since: None,
            until: None,
        }),
        EngineLimits::default(),
    ));

    assert_eq!(outcome.records.len(), 1);
    assert!(outcome.diagnostics.is_empty());
}

#[test]
fn selected_unknown_source_reports_diagnostic() {
    let source = catalog_source("alpha", vec![record("alpha", "1")]);
    let catalog = SourceCatalog::try_new(vec![source]).expect("unique sources");
    let outcome = block_on(catalog.collect_search(
        SearchRequest {
            query: Query {
                text: "oauth".to_string(),
                limit: None,
                since: None,
                until: None,
            },
            sources: vec!["missing".parse().expect("valid source id")],
            strict: false,
        },
        EngineLimits::default(),
    ));

    assert!(outcome.records.is_empty());
    assert_eq!(outcome.diagnostics[0].class, ErrorClass::NotFound);
}

#[test]
fn duplicate_sources_are_rejected() {
    let first = catalog_source("alpha", vec![record("alpha", "1")]);
    let second = catalog_source("alpha", vec![record("alpha", "2")]);

    assert!(matches!(
        SourceCatalog::try_new(vec![first, second]),
        Err(EngineError::DuplicateSourceId(_))
    ));
}

#[test]
fn search_progresses_without_waiting_for_blocked_source_completion() {
    let (gate_tx, gate_rx) = oneshot::channel();
    let gated = gated_source("alpha", gate_rx);
    let failing = failing_source("beta");
    let catalog = SourceCatalog::try_new(vec![gated, failing]).expect("unique sources");
    let mut stream = Box::pin(catalog.search_stream(
        SearchRequest::all(Query {
            text: "oauth".to_string(),
            limit: None,
            since: None,
            until: None,
        }),
        EngineLimits {
            max_active_sources: 2,
            event_buffer: 1,
            max_records_per_source: 10,
            max_bytes_per_source: 64 * 1024,
        },
    ));

    let first = block_on(stream.next()).expect("first event");
    assert!(matches!(first, crate::EngineEvent::Record(_)));
    let second = block_on(stream.next()).expect("second event");
    assert!(matches!(second, crate::EngineEvent::SourceFailed(_)));
    assert!(stream.next().now_or_never().is_none());
    gate_tx.send(()).expect("gate receiver alive");
    let third = block_on(stream.next()).expect("third event");
    assert!(matches!(third, crate::EngineEvent::Record(_)));
}

#[test]
fn collect_search_reads_nonstreaming_final_record_array() {
    let catalog = SourceCatalog::try_new(vec![final_data_source(
        "alpha",
        vec![record("alpha", "1"), record("alpha", "2")],
    )])
    .expect("unique sources");

    let outcome = block_on(catalog.collect_search(
        SearchRequest::all(Query {
            text: "oauth".to_string(),
            limit: None,
            since: None,
            until: None,
        }),
        EngineLimits::default(),
    ));

    assert_eq!(outcome.records.len(), 2);
    assert!(outcome.diagnostics.is_empty());
}

#[test]
fn search_order_and_dedupe_winner_are_source_id_stable() {
    let shared_url = "https://example.com/shared";
    let (alpha_gate_tx, alpha_gate_rx) = oneshot::channel();
    let catalog = SourceCatalog::try_new(vec![
        delayed_source(
            "alpha",
            shared_record("alpha", "winner", shared_url),
            alpha_gate_rx,
        ),
        catalog_source("beta", vec![shared_record("beta", "loser", shared_url)]),
    ])
    .expect("unique sources");
    let mut stream = Box::pin(catalog.search_stream(query(), limits()));

    assert!(stream.next().now_or_never().is_none());
    alpha_gate_tx.send(()).expect("alpha receiver alive");
    let first = block_on(stream.next()).expect("first record after alpha gate");
    assert_source(&first, "alpha");
    assert!(block_on(stream.collect::<Vec<_>>()).is_empty());
    let metrics = catalog.metrics();
    assert_eq!(metrics.sources["alpha"].records_emitted, 1);
    assert_eq!(metrics.sources["beta"].records_deduplicated, 1);
    serde_json::to_value(metrics).expect("metrics snapshot serializes");

    let (beta_gate_tx, beta_gate_rx) = oneshot::channel();
    let catalog = SourceCatalog::try_new(vec![
        catalog_source("alpha", vec![shared_record("alpha", "winner", shared_url)]),
        delayed_source(
            "beta",
            shared_record("beta", "loser", shared_url),
            beta_gate_rx,
        ),
    ])
    .expect("unique sources");
    let mut stream = Box::pin(catalog.search_stream(query(), limits()));
    let first = block_on(stream.next()).expect("first record before beta gate");
    assert_source(&first, "alpha");
    beta_gate_tx.send(()).expect("beta receiver alive");
    assert!(block_on(stream.collect::<Vec<_>>()).is_empty());
}

fn catalog_source(source: &str, records: Vec<Record>) -> CatalogSource {
    let id: SourceId = source.parse().expect("valid source id");
    let descriptor = comsat_source::SourceDescriptor::new(
        id.clone(),
        source,
        SourceProfile {
            search: true,
            fetch: true,
            follow: true,
        },
    );
    let runtime: Arc<dyn SourceRuntime> = Arc::new(StaticRuntime {
        source: id,
        records,
    });
    let mut cli = Cli::create(source);
    for command in source_commands(&descriptor, Arc::clone(&runtime)) {
        cli = cli.command(command.name.clone(), command);
    }
    CatalogSource::new(descriptor, cli.tool_catalog())
}

fn gated_source(source: &str, gate: oneshot::Receiver<()>) -> CatalogSource {
    let descriptor = descriptor(source);
    let runtime: Arc<dyn SourceRuntime> = Arc::new(GatedRuntime {
        first: record(source, "1"),
        second: record(source, "2"),
        gate: std::sync::Mutex::new(Some(gate)),
    });
    catalog_from_runtime(source, descriptor, &runtime)
}

fn failing_source(source: &str) -> CatalogSource {
    let descriptor = descriptor(source);
    let runtime: Arc<dyn SourceRuntime> = Arc::new(FailingRuntime {
        source: source.parse().expect("valid source id"),
    });
    catalog_from_runtime(source, descriptor, &runtime)
}

fn delayed_source(source: &str, record: Record, gate: oneshot::Receiver<()>) -> CatalogSource {
    let descriptor = descriptor(source);
    let runtime: Arc<dyn SourceRuntime> = Arc::new(DelayedRuntime {
        record,
        gate: std::sync::Mutex::new(Some(gate)),
    });
    catalog_from_runtime(source, descriptor, &runtime)
}

fn final_data_source(source: &str, records: Vec<Record>) -> CatalogSource {
    let descriptor = descriptor(source);
    let command = CommandDef::build(OperationKind::Search.as_str(), FinalDataHandler { records })
        .description("Return final record array")
        .mcp(McpCommandOptions {
            name: Some(descriptor.commands.search.clone()),
            description: Some("Return final record array".to_string()),
            ..McpCommandOptions::default()
        })
        .done();
    let cli = Cli::create(source).command(command.name.clone(), command);
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

fn catalog_from_runtime(
    source: &str,
    descriptor: comsat_source::SourceDescriptor,
    runtime: &Arc<dyn SourceRuntime>,
) -> CatalogSource {
    let mut cli = Cli::create(source);
    for command in source_commands(&descriptor, Arc::clone(runtime)) {
        cli = cli.command(command.name.clone(), command);
    }
    CatalogSource::new(descriptor, cli.tool_catalog())
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

fn shared_record(source: &str, id: &str, url: &str) -> Record {
    Record {
        url: url.to_string(),
        ..record(source, id)
    }
}

fn query() -> SearchRequest {
    SearchRequest::all(Query {
        text: "oauth".to_string(),
        limit: None,
        since: None,
        until: None,
    })
}

const fn limits() -> EngineLimits {
    EngineLimits {
        max_active_sources: 2,
        event_buffer: 1,
        max_records_per_source: 10,
        max_bytes_per_source: 64 * 1024,
    }
}

fn assert_source(event: &crate::EngineEvent, source: &str) {
    let crate::EngineEvent::Record(record) = event else {
        panic!("expected record event");
    };
    assert_eq!(record.source.as_str(), source);
}
