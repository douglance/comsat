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
    requests: Mutex<Vec<(String, String)>>,
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
            requests: Mutex::default(),
        }
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().unwrap().clone()
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
        request: Request<Vec<u8>>,
    ) -> comsat_source::SourceResult<Response<Vec<u8>>> {
        self.requests.lock().unwrap().push((
            request.uri().to_string(),
            String::from_utf8_lossy(request.body()).into_owned(),
        ));
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

const REPOSITORY_BODY: &str = include_str!("fixtures/repository.json");

fn query(text: &str) -> Query {
    Query {
        text: text.into(),
        limit: Some(2),
        since: None,
        until: None,
    }
}

fn github_source(http: Arc<MockHttp>, token: Option<&str>) -> Arc<dyn SourceRuntime> {
    Arc::new(GitHubSource::new(http, token.map(ToOwned::to_owned)))
}

async fn collect(stream: comsat_source::RecordStream) -> Vec<comsat_types::Record> {
    stream
        .map(|record| record.expect("record must be produced"))
        .collect()
        .await
}

fn url_target(url: &str) -> Target {
    Target::Url {
        source: SourceId::new("github").unwrap(),
        url: url.into(),
    }
}

fn native_target(id: &str) -> Target {
    Target::Native {
        source: SourceId::new("github").unwrap(),
        id: id.into(),
    }
}

#[tokio::test]
async fn repository_search_reads_the_repository_endpoint() {
    let http = Arc::new(MockHttp::with_statuses(vec![(
        200,
        concat!(
            r#"{"items":["#,
            include_str!("fixtures/repository.json"),
            "]}"
        ),
    )]));
    let source = github_source(Arc::clone(&http), None);

    let records = collect(
        source
            .search(
                query("type:repo rust compiler"),
                SourceRunContext::default(),
            )
            .await,
    )
    .await;

    let (uri, _) = http.requests().pop().unwrap();
    assert!(
        uri.starts_with("https://api.github.com/search/repositories?q="),
        "{uri}"
    );
    assert!(
        !uri.contains("type%3Arepo"),
        "selector must not reach GitHub: {uri}"
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "repository");
    assert_eq!(records[0].title.as_deref(), Some("rust-lang/rust"));
    assert_eq!(records[0].metadata["stars"], 104_000);
}

#[tokio::test]
async fn discussion_search_requires_a_token_before_any_request() {
    let http = Arc::new(MockHttp::with_statuses(vec![]));
    let source = github_source(Arc::clone(&http), None);

    let error = source
        .search(
            query("type:discussion turbopack"),
            SourceRunContext::default(),
        )
        .await
        .next()
        .await
        .unwrap()
        .unwrap_err();

    assert_eq!(error.class, ErrorClass::Authentication);
    assert!(
        http.requests().is_empty(),
        "no upstream call without a token"
    );
}

#[tokio::test]
async fn discussion_search_uses_graphql_and_normalizes_the_category() {
    let http = Arc::new(MockHttp::with_statuses(vec![(
        200,
        concat!(
            r#"{"data":{"search":{"nodes":["#,
            include_str!("fixtures/discussion.json"),
            "]}}}"
        ),
    )]));
    let source = github_source(Arc::clone(&http), Some("token"));

    let records = collect(
        source
            .search(
                query("type:discussion repo:vercel/next.js turbopack"),
                SourceRunContext::default(),
            )
            .await,
    )
    .await;

    let (uri, body) = http.requests().pop().unwrap();
    assert_eq!(uri, "https://api.github.com/graphql");
    assert!(body.contains("type: DISCUSSION"), "{body}");
    assert!(body.contains("repo:vercel/next.js turbopack"), "{body}");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "discussion");
    assert_eq!(records[0].metadata["category"], "Turbopack Error Report");
    assert_eq!(records[0].metadata["repository"], "vercel/next.js");
}

#[tokio::test]
async fn pull_request_follow_adds_the_review_discussion() {
    let http = Arc::new(MockHttp::with_statuses(vec![
        (
            200,
            r#"[{"id":11,"node_id":"IC_1","html_url":"https://github.com/owner/repo/pull/3#issuecomment-11","body":"ping","user":{"login":"bob"},"created_at":"2026-01-03T00:00:00Z","updated_at":"2026-01-03T00:00:00Z"}]"#,
        ),
        (
            200,
            r#"[{"id":21,"node_id":"PRR_1","html_url":"https://github.com/owner/repo/pull/3#pullrequestreview-21","body":"looks good","user":{"login":"carol"},"state":"APPROVED","submitted_at":"2026-01-04T00:00:00Z"}]"#,
        ),
        (
            200,
            r#"[{"id":31,"node_id":"PRRC_1","html_url":"https://github.com/owner/repo/pull/3#discussion_r31","body":"rename this","user":{"login":"carol"},"created_at":"2026-01-04T00:00:00Z","updated_at":"2026-01-04T00:00:00Z","path":"src/lib.rs","line":42,"pull_request_review_id":21}]"#,
        ),
    ]));
    let source = github_source(Arc::clone(&http), None);

    let records = collect(
        source
            .follow(
                url_target("https://github.com/owner/repo/pull/3"),
                SourceRunContext::default(),
            )
            .await,
    )
    .await;

    let paths: Vec<String> = http.requests().into_iter().map(|(uri, _)| uri).collect();
    assert!(paths[0].contains("/issues/3/comments"), "{paths:?}");
    assert!(paths[1].contains("/pulls/3/reviews"), "{paths:?}");
    assert!(paths[2].contains("/pulls/3/comments"), "{paths:?}");
    let kinds: Vec<&str> = records.iter().map(|record| record.kind.as_str()).collect();
    assert_eq!(kinds, ["issue-comment", "review", "review-comment"]);
    assert_eq!(records[2].metadata["path"], "src/lib.rs");
    assert_eq!(records[2].metadata["review_id"], 21);
}

#[tokio::test]
async fn plain_issue_follow_resolves_the_kind_once_and_skips_review_lookups() {
    let http = Arc::new(MockHttp::with_statuses(vec![
        (
            200,
            r#"{"id":10,"node_id":"I_1","html_url":"https://github.com/owner/repo/issues/3","title":"bug","body":null,"user":{"login":"alice"},"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","number":3,"state":"open","comments":1,"repository_url":"https://api.github.com/repos/owner/repo"}"#,
        ),
        (
            200,
            r#"[{"id":11,"node_id":"IC_1","html_url":"https://github.com/owner/repo/issues/3#issuecomment-11","body":"same here","user":{"login":"bob"},"created_at":"2026-01-03T00:00:00Z","updated_at":"2026-01-03T00:00:00Z"}]"#,
        ),
    ]));
    let source = github_source(Arc::clone(&http), None);

    let records = collect(
        source
            .follow(native_target("owner/repo#3"), SourceRunContext::default())
            .await,
    )
    .await;

    assert_eq!(http.requests().len(), 2, "no review lookups for an issue");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "issue-comment");
}

#[tokio::test]
async fn repository_follow_returns_repository_discussions() {
    let http = Arc::new(MockHttp::with_statuses(vec![(
        200,
        concat!(
            r#"{"data":{"repository":{"discussions":{"nodes":["#,
            include_str!("fixtures/discussion.json"),
            "]}}}}"
        ),
    )]));
    let source = github_source(Arc::clone(&http), Some("token"));

    let records = collect(
        source
            .follow(native_target("vercel/next.js"), SourceRunContext::default())
            .await,
    )
    .await;

    let (_, body) = http.requests().pop().unwrap();
    assert!(body.contains("discussions(first:"), "{body}");
    assert!(body.contains("UPDATED_AT"), "{body}");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "discussion");
}

#[tokio::test]
async fn repository_fetch_normalizes_repository_metadata() {
    let http = Arc::new(MockHttp::with_statuses(vec![(200, REPOSITORY_BODY)]));
    let source = github_source(Arc::clone(&http), None);

    let record = source
        .fetch(
            url_target("https://github.com/rust-lang/rust"),
            SourceRunContext::default(),
        )
        .await
        .unwrap();

    let (uri, _) = http.requests().pop().unwrap();
    // Path segments are percent-encoded; GitHub resolves the escaped hyphen.
    assert_eq!(uri, "https://api.github.com/repos/rust%2Dlang/rust");
    assert_eq!(record.kind, "repository");
    assert_eq!(record.metadata["language"], "Rust");
    assert_eq!(record.metadata["default_branch"], "main");
}

#[tokio::test]
async fn graphql_errors_become_structured_source_errors() {
    let http = Arc::new(MockHttp::with_statuses(vec![(
        200,
        r#"{"errors":[{"message":"Could not resolve to a Repository with the name 'nope/nope'."}]}"#,
    )]));
    let source = github_source(http, Some("token"));

    let error = source
        .follow(native_target("nope/nope"), SourceRunContext::default())
        .await
        .next()
        .await
        .unwrap()
        .unwrap_err();

    assert_eq!(error.class, ErrorClass::NotFound);
    assert!(
        error.message.contains("Could not resolve"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn missing_discussion_is_not_found_rather_than_a_parse_error() {
    let http = Arc::new(MockHttp::with_statuses(vec![(
        200,
        r#"{"data":{"repository":{"discussion":null}}}"#,
    )]));
    let source = github_source(http, Some("token"));

    let error = source
        .fetch(
            native_target("vercel/next.js/discussions/1"),
            SourceRunContext::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(error.class, ErrorClass::NotFound);
}
