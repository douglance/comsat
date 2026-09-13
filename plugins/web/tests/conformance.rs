use std::sync::{Arc, Mutex};

use comsat_source::{ConformanceSuite, HttpClient, SourceRunContext, SourceRuntime};
use comsat_types::{Query, Record, SourceId, Target};
use comsat_web::WebSource;
use futures::StreamExt;
use http::{Request, Response};
use incurs::agent_plugin::loader::{AgentPluginLoadOptions, load_agent_plugin};

#[test]
fn web_manifest_loads_through_incurs() {
    let report = load_agent_plugin(
        env!("CARGO_MANIFEST_DIR"),
        &AgentPluginLoadOptions::default(),
    );
    let plugin = report.plugin.unwrap();

    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert!(plugin.extensions.contains_key("io.comsat.source"));
}

#[tokio::test]
async fn web_search_preserves_relative_age_only_in_metadata() {
    let http = Arc::new(MockHttp::new(vec![
        r#"{"web":{"results":[{"url":"https://example.com/comsat","title":"COMSAT","description":"Composable search","age":"2 days ago","language":"en","family_friendly":true}]}}"#,
    ]));
    let source: Arc<dyn SourceRuntime> = Arc::new(WebSource::new(http, "test-key".into()));
    let query = Query {
        text: "COMSAT".into(),
        limit: Some(1),
        since: None,
        until: None,
    };

    let record = source
        .search(query, SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap()
        .unwrap();

    assert_eq!(record.created_at, None);
    assert_eq!(
        record
            .metadata
            .get("provider_age")
            .and_then(|value| value.as_str()),
        Some("2 days ago")
    );
}

#[derive(Default)]
struct MockHttp {
    responses: Mutex<Vec<Response<Vec<u8>>>>,
}

impl MockHttp {
    fn new(responses: Vec<&'static str>) -> Self {
        Self {
            responses: Mutex::new(
                responses
                    .into_iter()
                    .rev()
                    .map(|body| {
                        Response::builder()
                            .status(200)
                            .body(body.as_bytes().to_vec())
                            .unwrap()
                    })
                    .collect(),
            ),
        }
    }
}

#[async_trait::async_trait]
impl HttpClient for MockHttp {
    async fn send(
        &self,
        _request: Request<Vec<u8>>,
    ) -> comsat_source::SourceResult<Response<Vec<u8>>> {
        self.responses.lock().unwrap().pop().ok_or_else(|| {
            comsat_types::SourceError::new(
                SourceId::new("web").unwrap(),
                comsat_types::ErrorClass::Internal,
                "missing mock response",
            )
        })
    }
}

#[tokio::test]
async fn web_source_conforms_with_fixtures() {
    let http = Arc::new(MockHttp::new(vec![
        r#"{"web":{"results":[{"url":"https://example.com/comsat#top","title":"COMSAT","description":"Composable search","age":"2026-01-01T00:00:00Z","language":"en","family_friendly":true}]}}"#,
        r#"{"web":{"results":[{"url":"https://example.com/comsat#top","title":"COMSAT","description":"Composable search","age":"2026-01-01T00:00:00Z","language":"en","family_friendly":true}]}}"#,
        r"<html><title>COMSAT</title><body>Composable search</body></html>",
    ]));
    let source: Arc<dyn SourceRuntime> = Arc::new(WebSource::new(http, "test-key".into()));
    let query = Query {
        text: "COMSAT".into(),
        limit: Some(1),
        since: None,
        until: None,
    };
    let target = Target::Record {
        record: Box::new(
            serde_json::from_value::<Record>(serde_json::json!({
                "id": "web:https://example.com/comsat",
                "source": "web",
                "kind": "search-result",
                "url": "https://example.com/comsat",
                "title": "COMSAT",
                "text": "Composable search",
                "author": null,
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": null,
                "metadata": { "provider": "brave" }
            }))
            .unwrap(),
        ),
    };

    let report = ConformanceSuite::new(WebSource::descriptor())
        .run(source, query, Some(target))
        .await
        .unwrap();

    assert!(report.search_checked);
    assert!(report.fetch_checked);
    assert!(!report.follow_checked);
}
