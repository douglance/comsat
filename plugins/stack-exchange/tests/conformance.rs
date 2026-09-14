use std::sync::{Arc, Mutex};

use comsat_source::{
    CancellationFixture, ConformanceFixtures, ConformanceSuite, HttpClient, SourceErrorFixture,
    SourceRunContext, SourceRuntime,
};
use comsat_stack_exchange::StackExchangeSource;
use comsat_types::{ErrorClass, Query, SourceId, Target};
use futures::StreamExt;
use http::{Request, Response};
use incurs::agent_plugin::loader::{AgentPluginLoadOptions, load_agent_plugin};

#[test]
fn stack_exchange_manifest_loads_through_incurs() {
    let report = load_agent_plugin(
        env!("CARGO_MANIFEST_DIR"),
        &AgentPluginLoadOptions::default(),
    );
    let plugin = report.plugin.unwrap();

    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert!(plugin.extensions.contains_key("io.comsat.source"));
}

#[derive(Default)]
struct MockHttp {
    responses: Mutex<Vec<MockResponse>>,
    requests: Mutex<Vec<String>>,
}

enum MockResponse {
    Response(Response<Vec<u8>>),
    Pending,
}

#[tokio::test]
async fn stack_exchange_backoff_blocks_next_same_operation_after_returning_records() {
    let http = Arc::new(MockHttp::new(vec![
        r#"{"backoff":60,"items":[{"question_id":481,"link":"https://stackoverflow.com/questions/481/example","title":"How do I use MCP?","body":"Question body","owner":{"display_name":"Dana","user_id":44},"creation_date":1767225600,"last_activity_date":1767229200,"score":5,"answer_count":1,"is_answered":true,"tags":["rust","mcp"]}]}"#,
    ]));
    let source: Arc<dyn SourceRuntime> = Arc::new(StackExchangeSource::new(http, None, None));
    let query = Query {
        text: "MCP".into(),
        limit: Some(1),
        since: None,
        until: None,
    };

    let first = source
        .search(query.clone(), SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap();
    assert!(first.is_ok());

    let second = source
        .search(query, SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap();
    let error = second.unwrap_err();
    assert_eq!(error.class, comsat_types::ErrorClass::RateLimit);
    assert!(error.retry_after_seconds.is_some());
}

#[tokio::test]
async fn stack_exchange_throttle_error_without_items_sets_backoff_and_latches() {
    let http = Arc::new(MockHttp::new(vec![
        r#"{"error_id":502,"error_name":"throttle_violation","error_message":"too many requests","backoff":30}"#,
    ]));
    let source: Arc<dyn SourceRuntime> = Arc::new(StackExchangeSource::new(http, None, None));
    let query = Query {
        text: "MCP".into(),
        limit: Some(1),
        since: None,
        until: None,
    };

    let first = source
        .search(query.clone(), SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(first.class, comsat_types::ErrorClass::RateLimit);
    assert_eq!(first.retry_after_seconds, Some(30));

    let second = source
        .search(query, SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(second.class, comsat_types::ErrorClass::RateLimit);
    assert!(second.retry_after_seconds.is_some());
}

impl MockHttp {
    fn new(responses: Vec<&'static str>) -> Self {
        Self::with_statuses(responses.into_iter().map(|body| (200, body)).collect())
    }

    fn with_statuses(responses: Vec<(u16, &'static str)>) -> Self {
        Self::with_statuses_and_pending(responses, false)
    }

    fn with_statuses_and_pending(responses: Vec<(u16, &'static str)>, pending: bool) -> Self {
        let pending = pending.then_some(MockResponse::Pending);
        let responses = responses
            .into_iter()
            .map(|body| {
                MockResponse::Response(
                    Response::builder()
                        .status(body.0)
                        .body(body.1.as_bytes().to_vec())
                        .unwrap(),
                )
            })
            .chain(pending)
            .rev()
            .collect();
        Self {
            responses: Mutex::new(responses),
            requests: Mutex::default(),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl HttpClient for MockHttp {
    async fn send(
        &self,
        request: Request<Vec<u8>>,
    ) -> comsat_source::SourceResult<Response<Vec<u8>>> {
        self.requests
            .lock()
            .unwrap()
            .push(request.uri().to_string());
        let response = self.responses.lock().unwrap().pop().ok_or_else(|| {
            comsat_types::SourceError::new(
                SourceId::new("stack-exchange").unwrap(),
                comsat_types::ErrorClass::Internal,
                "missing mock response",
            )
        })?;
        match response {
            MockResponse::Response(response) => Ok(response),
            MockResponse::Pending => std::future::pending().await,
        }
    }
}

#[tokio::test]
async fn stack_exchange_http_400_error_envelope_sets_backoff_and_latches() {
    let http = Arc::new(MockHttp::with_statuses(vec![(
        400,
        r#"{"error_id":400,"error_name":"bad_parameter","error_message":"site is required","backoff":20}"#,
    )]));
    let source: Arc<dyn SourceRuntime> = Arc::new(StackExchangeSource::new(http, None, None));
    let query = Query {
        text: "MCP".into(),
        limit: Some(1),
        since: None,
        until: None,
    };

    let first = source
        .search(query.clone(), SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(first.class, comsat_types::ErrorClass::InvalidQuery);
    assert_eq!(first.retry_after_seconds, Some(20));

    let second = source
        .search(query, SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(second.class, comsat_types::ErrorClass::RateLimit);
    assert!(second.retry_after_seconds.is_some());
}

#[tokio::test]
async fn stack_exchange_encodes_site_and_api_key_query_values() {
    let http = Arc::new(MockHttp::new(vec![
        r#"{"items":[{"question_id":481,"link":"https://stackoverflow.com/questions/481/example","title":"How do I use MCP?","body":"Question body"}]}"#,
    ]));
    let source: Arc<dyn SourceRuntime> = Arc::new(StackExchangeSource::new(
        http.clone(),
        Some("stack overflow".into()),
        Some("key+/= value".into()),
    ));
    let query = Query {
        text: "MCP".into(),
        limit: Some(1),
        since: None,
        until: None,
    };

    let record = source
        .search(query, SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap();
    assert!(record.is_ok());

    let request = http.requests().pop().unwrap();
    assert!(request.contains("site=stack%20overflow"));
    assert!(request.contains("key=key%2B%2F%3D%20value"));
}

#[tokio::test]
async fn stack_exchange_source_conforms_with_fixtures() {
    let http = Arc::new(MockHttp::with_statuses_and_pending(
        vec![
            (
                200,
                r#"{"items":[{"question_id":481,"link":"https://stackoverflow.com/questions/481/example","title":"How do I use MCP?","body":"Question body","owner":{"display_name":"Dana","user_id":44},"creation_date":1767225600,"last_activity_date":1767229200,"score":5,"answer_count":1,"is_answered":true,"tags":["rust","mcp"]}]}"#,
            ),
            (
                200,
                r#"{"items":[{"question_id":481,"link":"https://stackoverflow.com/questions/481/example","title":"How do I use MCP?","body":"Question body","owner":{"display_name":"Dana","user_id":44},"creation_date":1767225600,"last_activity_date":1767229200,"score":5,"answer_count":1,"is_answered":true,"tags":["rust","mcp"]}]}"#,
            ),
            (
                200,
                r#"{"items":[{"question_id":481,"link":"https://stackoverflow.com/questions/481/example","title":"How do I use MCP?","body":"Question body","owner":{"display_name":"Dana","user_id":44},"creation_date":1767225600,"last_activity_date":1767229200,"score":5,"answer_count":1,"is_answered":true,"tags":["rust","mcp"]}]}"#,
            ),
            (
                200,
                r#"{"items":[{"answer_id":482,"question_id":481,"link":"https://stackoverflow.com/a/482","body":"Answer body","owner":{"display_name":"Alex","user_id":45},"creation_date":1767230000,"last_activity_date":1767230000,"score":3,"is_accepted":true}]}"#,
            ),
            (
                200,
                r#"{"error_id":400,"error_name":"bad_parameter","error_message":"bad parameter"}"#,
            ),
        ],
        true,
    ));
    let source: Arc<dyn SourceRuntime> = Arc::new(StackExchangeSource::new(http, None, None));
    let query = Query {
        text: "MCP".into(),
        limit: Some(1),
        since: Some("2026-01-01T00:00:00Z".into()),
        until: Some("2026-01-31T00:00:00Z".into()),
    };
    let target = Target::Url {
        source: SourceId::new("stack-exchange").unwrap(),
        url: "https://stackoverflow.com/questions/481/example".into(),
    };

    let report = ConformanceSuite::new(StackExchangeSource::descriptor())
        .run_with_fixtures(
            source,
            ConformanceFixtures {
                query: query.clone(),
                target: Some(target),
                error: Some(SourceErrorFixture {
                    query: query.clone(),
                    expected_class: ErrorClass::InvalidQuery,
                }),
                cancellation: Some(CancellationFixture { query }),
            },
        )
        .await
        .unwrap();

    assert!(report.search_checked);
    assert!(report.fetch_checked);
    assert!(report.follow_checked);
    assert!(report.error_checked);
    assert!(report.cancellation_checked);
}
