use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use comsat_types::{ErrorClass, Query, Record, SourceError, SourceId, Target};
use futures::executor::block_on;
use futures::stream;

use super::{
    CancellationFixture, ConformanceError, ConformanceFixtures, ConformanceSuite,
    SourceErrorFixture, validate_schema_document,
};
use crate::{RecordStream, SourceDescriptor, SourceProfile, SourceRunContext, SourceRuntime};

#[test]
fn checks_source_error_and_active_cancellation_fixtures() {
    let runtime: Arc<dyn SourceRuntime> = Arc::new(ScriptedRuntime::new(vec![
        SearchStep::Records(vec![record("item-1", "https://example.com/item-1")]),
        SearchStep::Records(vec![record("item-1", "https://example.com/item-1")]),
        SearchStep::Error(ErrorClass::Authentication),
        SearchStep::Pending,
    ]));

    let report = block_on(suite().run_with_fixtures(runtime, full_fixtures())).unwrap();

    assert!(report.search_checked);
    assert!(report.error_checked);
    assert!(report.cancellation_checked);
}

#[test]
fn rejects_unstable_record_ids() {
    let runtime: Arc<dyn SourceRuntime> = Arc::new(ScriptedRuntime::new(vec![
        SearchStep::Records(vec![record("first", "https://example.com/first")]),
        SearchStep::Records(vec![record("second", "https://example.com/second")]),
    ]));

    let error = block_on(suite().run(runtime, query(), None)).unwrap_err();

    assert!(matches!(error, ConformanceError::UnstableRecordIds));
}

#[test]
fn rejects_malformed_record_streams() {
    let runtime: Arc<dyn SourceRuntime> = Arc::new(ScriptedRuntime::new(vec![
        SearchStep::Records(vec![invalid_record()]),
    ]));

    let error = block_on(suite().run(runtime, query(), None)).unwrap_err();

    assert!(matches!(error, ConformanceError::Tool(message) if message.contains("URL")));
}

#[test]
fn rejects_unstructured_error_codes() {
    let fixtures = ConformanceFixtures {
        cancellation: None,
        ..full_fixtures()
    };
    let runtime: Arc<dyn SourceRuntime> = Arc::new(ScriptedRuntime::new(vec![
        SearchStep::Records(vec![record("item-1", "https://example.com/item-1")]),
        SearchStep::Records(vec![record("item-1", "https://example.com/item-1")]),
        SearchStep::Records(vec![invalid_record()]),
    ]));

    let error = block_on(suite().run_with_fixtures(runtime, fixtures)).unwrap_err();

    assert!(matches!(
        error,
        ConformanceError::UnstructuredSourceError(code) if code == "serialization"
    ));
}

#[test]
fn rejects_completed_cancellation_fixtures() {
    let fixtures = ConformanceFixtures {
        error: None,
        ..full_fixtures()
    };
    let runtime: Arc<dyn SourceRuntime> = Arc::new(ScriptedRuntime::new(vec![
        SearchStep::Records(vec![record("item-1", "https://example.com/item-1")]),
        SearchStep::Records(vec![record("item-1", "https://example.com/item-1")]),
        SearchStep::Records(vec![record("item-1", "https://example.com/item-1")]),
    ]));

    let error = block_on(suite().run_with_fixtures(runtime, fixtures)).unwrap_err();

    assert!(matches!(error, ConformanceError::CancellationFinishedEarly));
}

#[test]
fn rejects_schema_with_nested_schema_keyword() {
    let schema = serde_json::json!({
        "type": "array",
        "items": {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": "#/$defs/Record"
        },
        "$defs": {
            "Record": { "type": "object" }
        }
    });

    let error = validate_schema_document("search", &schema).unwrap_err();

    assert!(matches!(
        error,
        ConformanceError::InvalidToolSchema { reason, .. }
            if reason.contains("nested $schema")
    ));
}

#[test]
fn rejects_schema_with_dangling_local_ref() {
    let schema = serde_json::json!({
        "type": "array",
        "items": { "$ref": "#/$defs/Record" }
    });

    let error = validate_schema_document("search", &schema).unwrap_err();

    assert!(matches!(
        error,
        ConformanceError::InvalidToolSchema { reason, .. }
            if reason.contains("unresolved local ref")
    ));
}

struct ScriptedRuntime {
    steps: Vec<SearchStep>,
    calls: AtomicUsize,
}

impl ScriptedRuntime {
    fn new(steps: Vec<SearchStep>) -> Self {
        Self {
            steps,
            calls: AtomicUsize::new(0),
        }
    }
}

enum SearchStep {
    Records(Vec<Record>),
    Error(ErrorClass),
    Pending,
}

#[async_trait]
impl SourceRuntime for ScriptedRuntime {
    async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        match self.steps.get(index).expect("script step must exist") {
            SearchStep::Records(records) => Box::pin(stream::iter(
                records.clone().into_iter().map(Ok::<Record, SourceError>),
            )),
            SearchStep::Error(class) => {
                let error = SourceError::new(source_id(), *class, "fixture error");
                Box::pin(stream::iter([Err(error)]))
            }
            SearchStep::Pending => Box::pin(stream::pending::<Result<Record, SourceError>>()),
        }
    }

    async fn fetch(
        &self,
        _target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, SourceError> {
        Ok(record("fetch", "https://example.com/fetch"))
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        Box::pin(stream::empty())
    }
}

fn suite() -> ConformanceSuite {
    ConformanceSuite::new(SourceDescriptor::new(
        source_id(),
        "Test",
        SourceProfile {
            search: true,
            fetch: false,
            follow: false,
        },
    ))
}

fn full_fixtures() -> ConformanceFixtures {
    ConformanceFixtures {
        query: query(),
        target: None,
        error: Some(SourceErrorFixture {
            query: query(),
            expected_class: ErrorClass::Authentication,
        }),
        cancellation: Some(CancellationFixture { query: query() }),
    }
}

fn query() -> Query {
    Query {
        text: "COMSAT".to_string(),
        limit: Some(1),
        since: None,
        until: None,
    }
}

fn record(id: &str, url: &str) -> Record {
    Record {
        id: id.parse().expect("valid record id"),
        source: source_id(),
        kind: "fixture".to_string(),
        url: url.to_string(),
        title: Some("Fixture".to_string()),
        text: Some("Fixture text".to_string()),
        author: None,
        created_at: None,
        updated_at: None,
        metadata: serde_json::json!({}),
    }
}

fn invalid_record() -> Record {
    record("invalid", "not-a-url")
}

fn source_id() -> SourceId {
    "test".parse().expect("valid source id")
}
