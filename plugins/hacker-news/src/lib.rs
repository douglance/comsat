#![forbid(unsafe_code)]

use std::sync::Arc;

mod target;
mod timeutil;

use comsat_source::{
    HttpClient, RecordStream, SourceDescriptor, SourceProfile, SourceRunContext, SourceRuntime,
};
use comsat_types::{ErrorClass, Query, Record, SourceError, SourceId, Target};
use http::{Request, Response, StatusCode};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE_ID: &str = "hacker-news";
const MAX_LIMIT: u32 = 50;

#[derive(Clone)]
pub struct HackerNewsSource {
    client: Arc<dyn HttpClient>,
}

impl HackerNewsSource {
    pub fn new(client: Arc<dyn HttpClient>) -> Self {
        Self { client }
    }

    pub fn descriptor() -> SourceDescriptor {
        SourceDescriptor::new(
            source_id(),
            "Hacker News",
            SourceProfile {
                search: true,
                fetch: true,
                follow: true,
            },
        )
    }

    async fn search_records(&self, query: Query) -> Result<Vec<Record>, SourceError> {
        ensure_limit(query.limit)?;
        let encoded = utf8_percent_encode(&query.text, NON_ALPHANUMERIC);
        let filters = algolia_filters(&query)?;
        let limit = query.limit.unwrap_or(10);
        let tags = utf8_percent_encode("(story,comment)", NON_ALPHANUMERIC);
        let url = format!(
            "https://hn.algolia.com/api/v1/search?query={encoded}&tags={tags}&hitsPerPage={limit}&advancedSyntax=true{filters}"
        );
        let body = checked_body(self.client.send(get(url)?).await?)?;
        let response: AlgoliaResponse = parse_json(&body)?;
        response.hits.iter().map(normalize_hit).collect()
    }

    async fn fetch_item_record(&self, target: Target) -> Result<Record, SourceError> {
        normalize_item(
            &self
                .fetch_item(target::target_id(source_id(), &target)?)
                .await?,
        )
    }

    async fn fetch_item(&self, id: u64) -> Result<Item, SourceError> {
        let url = format!("https://hacker-news.firebaseio.com/v0/item/{id}.json");
        let body = checked_body(self.client.send(get(url)?).await?)?;
        parse_json(&body)
    }
}

#[async_trait::async_trait]
impl SourceRuntime for HackerNewsSource {
    async fn search(&self, query: Query, _context: SourceRunContext) -> RecordStream {
        let source = self.clone();
        Box::pin(async_stream::stream! {
            match source.search_records(query).await {
                Ok(records) => for record in records { yield Ok(record); },
                Err(error) => yield Err(error),
            }
        })
    }

    async fn fetch(
        &self,
        target: Target,
        _context: SourceRunContext,
    ) -> Result<Record, SourceError> {
        self.fetch_item_record(target).await
    }

    async fn follow(&self, target: Target, _context: SourceRunContext) -> RecordStream {
        let source = self.clone();
        Box::pin(async_stream::stream! {
            let item = match target::target_id(source_id(), &target) {
                Ok(id) => source.fetch_item(id).await,
                Err(error) => Err(error),
            };
            match item {
                Ok(item) => {
                    for id in item.kids.iter().flatten().take(MAX_LIMIT as usize) {
                        yield source.fetch_item(*id).await.and_then(|item| normalize_item(&item));
                    }
                }
                Err(error) => yield Err(error),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct AlgoliaResponse {
    hits: Vec<AlgoliaHit>,
}

#[derive(Debug, Deserialize)]
struct AlgoliaHit {
    #[serde(rename = "objectID")]
    object_id: String,
    title: Option<String>,
    story_title: Option<String>,
    url: Option<String>,
    story_url: Option<String>,
    author: Option<String>,
    comment_text: Option<String>,
    story_text: Option<String>,
    created_at: Option<String>,
    points: Option<i64>,
    num_comments: Option<i64>,
    story_id: Option<u64>,
    parent_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Item {
    id: u64,
    #[serde(rename = "type")]
    kind: String,
    by: Option<String>,
    time: Option<u64>,
    title: Option<String>,
    text: Option<String>,
    url: Option<String>,
    score: Option<i64>,
    descendants: Option<i64>,
    parent: Option<u64>,
    kids: Option<Vec<u64>>,
}

fn normalize_hit(hit: &AlgoliaHit) -> Result<Record, SourceError> {
    let hn_url = format!("https://news.ycombinator.com/item?id={}", hit.object_id);
    let outbound_url = hit.url.as_ref().or(hit.story_url.as_ref()).cloned();
    let kind = if hit.comment_text.is_some() {
        "comment"
    } else {
        "story"
    };
    record(json!({
        "id": format!("hacker-news:{}", hit.object_id),
        "source": SOURCE_ID,
        "kind": kind,
        "url": hn_url,
        "title": hit.title.as_ref().or(hit.story_title.as_ref()),
        "text": hit.comment_text.as_ref().or(hit.story_text.as_ref()),
        "author": hit.author,
        "created_at": hit.created_at,
        "updated_at": null,
        "metadata": {
            "object_id": hit.object_id,
            "story_id": hit.story_id,
            "parent_id": hit.parent_id,
            "points": hit.points,
            "num_comments": hit.num_comments,
            "outbound_url": outbound_url,
            "search_provider": "hn.algolia.com"
        }
    }))
}

fn normalize_item(item: &Item) -> Result<Record, SourceError> {
    let url = format!("https://news.ycombinator.com/item?id={}", item.id);
    record(json!({
        "id": format!("hacker-news:{}", item.id),
        "source": SOURCE_ID,
        "kind": item.kind,
        "url": url,
        "title": item.title,
        "text": item.text,
        "author": item.by,
        "created_at": item.time.and_then(timeutil::unix_to_rfc3339),
        "updated_at": null,
        "metadata": {
            "story_id": if item.kind == "story" { Some(item.id) } else { None },
            "parent_id": item.parent,
            "points": item.score,
            "descendants": item.descendants,
            "kid_count": item.kids.as_ref().map(Vec::len),
            "outbound_url": item.url
        }
    }))
}

fn algolia_filters(query: &Query) -> Result<String, SourceError> {
    let mut filters = Vec::new();
    if let Some(since) = query.since.as_deref() {
        filters.push(format!(
            "created_at_i>={}",
            timeutil::rfc3339_to_unix(source_id(), since)?
        ));
    }
    if let Some(until) = query.until.as_deref() {
        filters.push(format!(
            "created_at_i<={}",
            timeutil::rfc3339_to_unix(source_id(), until)?
        ));
    }
    if filters.is_empty() {
        return Ok(String::new());
    }
    Ok(format!(
        "&numericFilters={}",
        utf8_percent_encode(&filters.join(","), NON_ALPHANUMERIC)
    ))
}

fn ensure_limit(limit: Option<u32>) -> Result<(), SourceError> {
    if limit.unwrap_or(10) > MAX_LIMIT {
        return Err(SourceError::new(
            source_id(),
            ErrorClass::InvalidQuery,
            "Hacker News source supports at most {MAX_LIMIT} results per search",
        ));
    }
    Ok(())
}

fn get(url: String) -> Result<Request<Vec<u8>>, SourceError> {
    Request::builder()
        .method("GET")
        .uri(url)
        .header("user-agent", "comsat")
        .body(Vec::new())
        .map_err(protocol)
}

fn checked_body(response: Response<Vec<u8>>) -> Result<Vec<u8>, SourceError> {
    let status = response.status();
    let body = response.into_body();
    if status.is_success() {
        return Ok(body);
    }
    match status {
        StatusCode::NOT_FOUND => Err(error(
            ErrorClass::NotFound,
            "Hacker News item was not found",
        )),
        status => Err(error(
            ErrorClass::Upstream,
            format!("Hacker News returned HTTP {status}"),
        )),
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, SourceError> {
    serde_json::from_slice(body).map_err(protocol)
}

fn record(value: Value) -> Result<Record, SourceError> {
    serde_json::from_value(value).map_err(protocol)
}

fn source_id() -> SourceId {
    SourceId::new(SOURCE_ID).expect("static source id is valid")
}

fn error(class: ErrorClass, message: impl Into<String>) -> SourceError {
    SourceError::new(source_id(), class, message)
}

fn protocol(message: impl std::fmt::Display) -> SourceError {
    error(ErrorClass::Protocol, message.to_string())
}
