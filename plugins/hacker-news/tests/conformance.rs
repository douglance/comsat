use std::sync::{Arc, Mutex};

use comsat_hacker_news::HackerNewsSource;
use comsat_source::{ConformanceSuite, HttpClient, SourceRunContext, SourceRuntime};
use comsat_types::{Query, SourceId, Target};
use futures::StreamExt;
use http::{Request, Response};
use incurs::agent_plugin::loader::{AgentPluginLoadOptions, load_agent_plugin};

#[test]
fn hacker_news_manifest_loads_through_incurs() {
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
    responses: Mutex<Vec<Response<Vec<u8>>>>,
    requests: Mutex<Vec<String>>,
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
        self.responses.lock().unwrap().pop().ok_or_else(|| {
            comsat_types::SourceError::new(
                SourceId::new("hacker-news").unwrap(),
                comsat_types::ErrorClass::Internal,
                "missing mock response",
            )
        })
    }
}

#[tokio::test]
async fn hacker_news_search_uses_algolia_or_tags_and_hn_item_url() {
    let http = Arc::new(MockHttp::new(vec![
        r#"{"hits":[{"_tags":["story","author_Sikul","story_22238335"],"author":"Sikul","created_at":"2020-02-04T17:30:40Z","num_comments":642,"objectID":"22238335","points":1582,"story_id":22238335,"title":"Why Discord is switching from Go to Rust","url":"https://blog.discordapp.com/why-discord-is-switching-from-go-to-rust-a190bbca2b1f"}],"hitsPerPage":1,"nbHits":620489,"page":0}"#,
    ]));
    let source: Arc<dyn SourceRuntime> = Arc::new(HackerNewsSource::new(http.clone()));
    let query = Query {
        text: "Rust".into(),
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

    let request = http.requests().pop().unwrap();
    assert!(request.contains("tags=%28story%2Ccomment%29"));
    assert!(request.contains("advancedSyntax=true"));
    assert_eq!(record.url, "https://news.ycombinator.com/item?id=22238335");
    assert_eq!(
        record
            .metadata
            .get("outbound_url")
            .and_then(|value| value.as_str()),
        Some("https://blog.discordapp.com/why-discord-is-switching-from-go-to-rust-a190bbca2b1f")
    );
}

#[tokio::test]
async fn hacker_news_source_conforms_with_fixtures() {
    let http = Arc::new(MockHttp::new(vec![
        r#"{"hits":[{"_tags":["story","author_pg","story_43192810"],"objectID":"43192810","title":"COMSAT","url":"https://example.com/comsat","author":"pg","created_at":"2026-01-01T00:00:00Z","points":37,"num_comments":1,"story_id":43192810}]}"#,
        r#"{"hits":[{"_tags":["story","author_pg","story_43192810"],"objectID":"43192810","title":"COMSAT","url":"https://example.com/comsat","author":"pg","created_at":"2026-01-01T00:00:00Z","points":37,"num_comments":1,"story_id":43192810}]}"#,
        r#"{"id":43192810,"type":"story","by":"pg","time":1767225600,"title":"COMSAT","url":"https://example.com/comsat","score":37,"descendants":1,"kids":[43192811]}"#,
        r#"{"id":43192810,"type":"story","by":"pg","time":1767225600,"title":"COMSAT","url":"https://example.com/comsat","score":37,"descendants":1,"kids":[43192811]}"#,
        r#"{"id":43192811,"type":"comment","by":"alice","time":1767229200,"text":"nice","parent":43192810}"#,
    ]));
    let source: Arc<dyn SourceRuntime> = Arc::new(HackerNewsSource::new(http));
    let query = Query {
        text: "COMSAT".into(),
        limit: Some(1),
        since: Some("2026-01-01T00:00:00Z".into()),
        until: Some("2026-01-31T00:00:00Z".into()),
    };
    let target = Target::Url {
        source: SourceId::new("hacker-news").unwrap(),
        url: "https://news.ycombinator.com/item?id=43192810".into(),
    };

    let report = ConformanceSuite::new(HackerNewsSource::descriptor())
        .run(source, query, Some(target))
        .await
        .unwrap();

    assert!(report.search_checked);
    assert!(report.fetch_checked);
    assert!(report.follow_checked);
}
