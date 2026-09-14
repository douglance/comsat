use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use comsat_github::GitHubSource;
use comsat_source::{
    CancellationFixture, ConformanceFixtures, ConformanceSuite, HttpClient, SourceErrorFixture,
    SourceRunContext, SourceRuntime,
};
use comsat_types::{ErrorClass, Query, SourceId, Target};
use futures::StreamExt;
use http::{Request, Response};
use incurs::agent_plugin::loader::{AgentPluginLoadOptions, load_agent_plugin};

#[test]
fn github_manifest_loads_through_incurs() {
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
    responses: Mutex<VecDeque<MockResponse>>,
}

enum MockResponse {
    Response(Response<Vec<u8>>),
    Pending,
}

impl MockHttp {
    fn with_statuses(responses: Vec<(u16, &'static str)>) -> Self {
        Self::with_statuses_and_pending(responses, false)
    }

    fn with_statuses_and_pending(responses: Vec<(u16, &'static str)>, pending: bool) -> Self {
        let mut responses: VecDeque<_> = responses
            .into_iter()
            .map(|(status, body)| {
                MockResponse::Response(
                    Response::builder()
                        .status(status)
                        .header("retry-after", "60")
                        .body(body.as_bytes().to_vec())
                        .unwrap(),
                )
            })
            .collect();
        if pending {
            responses.push_back(MockResponse::Pending);
        }
        Self {
            responses: Mutex::new(responses),
        }
    }
}

#[tokio::test]
async fn github_rate_limit_blocks_next_same_operation() {
    let http = Arc::new(MockHttp::with_statuses(vec![(403, r"{}")]));
    let source: Arc<dyn SourceRuntime> = Arc::new(GitHubSource::new(http, None));
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
    assert!(first.is_err());

    let second = source
        .search(query, SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap();
    let error = second.unwrap_err();
    assert_eq!(error.class, comsat_types::ErrorClass::RateLimit);
    assert!(matches!(error.retry_after_seconds, Some(1..=60)));
}

#[async_trait::async_trait]
impl HttpClient for MockHttp {
    async fn send(
        &self,
        _request: Request<Vec<u8>>,
    ) -> comsat_source::SourceResult<Response<Vec<u8>>> {
        let response = self.responses.lock().unwrap().pop_front().ok_or_else(|| {
            comsat_types::SourceError::new(
                SourceId::new("github").unwrap(),
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
async fn github_source_conforms_with_fixtures() {
    let http = Arc::new(MockHttp::with_statuses_and_pending(
        vec![
            (
                200,
                r#"{"items":[{"id":10,"node_id":"I_kwDO","html_url":"https://github.com/owner/repo/issues/3","title":"OAuth trouble","body":"token refresh fails","user":{"login":"alice"},"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","number":3,"state":"open","comments":1,"repository_url":"https://api.github.com/repos/owner/repo"}]}"#,
            ),
            (
                200,
                r#"{"items":[{"id":10,"node_id":"I_kwDO","html_url":"https://github.com/owner/repo/issues/3","title":"OAuth trouble","body":"token refresh fails","user":{"login":"alice"},"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","number":3,"state":"open","comments":1,"repository_url":"https://api.github.com/repos/owner/repo"}]}"#,
            ),
            (
                200,
                r#"{"id":10,"node_id":"I_kwDO","html_url":"https://github.com/owner/repo/issues/3","title":"OAuth trouble","body":"token refresh fails","user":{"login":"alice"},"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","number":3,"state":"open","comments":1,"repository_url":"https://api.github.com/repos/owner/repo"}"#,
            ),
            (
                200,
                r#"[{"id":11,"node_id":"IC_kwDO","html_url":"https://github.com/owner/repo/issues/3#issuecomment-11","body":"same here","user":{"login":"bob"},"created_at":"2026-01-03T00:00:00Z","updated_at":"2026-01-03T00:00:00Z","reactions":{"total_count":1}}]"#,
            ),
            (401, r"{}"),
        ],
        true,
    ));
    let source: Arc<dyn SourceRuntime> = Arc::new(GitHubSource::new(http, None));
    let query = Query {
        text: "MCP OAuth".into(),
        limit: Some(1),
        since: Some("2026-01-01T00:00:00Z".into()),
        until: Some("2026-01-31T00:00:00Z".into()),
    };
    let target = Target::Url {
        source: SourceId::new("github").unwrap(),
        url: "https://github.com/owner/repo/issues/3".into(),
    };

    let report = ConformanceSuite::new(GitHubSource::descriptor())
        .run_with_fixtures(
            source,
            ConformanceFixtures {
                query: query.clone(),
                target: Some(target),
                error: Some(SourceErrorFixture {
                    query: query.clone(),
                    expected_class: ErrorClass::Authentication,
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
