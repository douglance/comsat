#![forbid(unsafe_code)]

use std::sync::Arc;

mod guard;

use comsat_source::{
    HttpClient, RecordStream, SourceDescriptor, SourceProfile, SourceRunContext, SourceRuntime,
};
use comsat_types::{ErrorClass, Query, Record, SourceError, SourceId, Target};
use http::{Request, Response, StatusCode};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

const SOURCE_ID: &str = "web";
const MAX_LIMIT: u32 = 20;
const MAX_FETCH_BYTES: usize = 256_000;

#[derive(Clone)]
pub struct WebSource {
    client: Arc<dyn HttpClient>,
    brave_api_key: String,
}

impl WebSource {
    pub fn new(client: Arc<dyn HttpClient>, brave_api_key: String) -> Self {
        Self {
            client,
            brave_api_key,
        }
    }

    pub fn descriptor() -> SourceDescriptor {
        SourceDescriptor::new(
            source_id(),
            "Web Search",
            SourceProfile {
                search: true,
                fetch: true,
                follow: false,
            },
        )
    }

    async fn search_records(&self, query: Query) -> Result<Vec<Record>, SourceError> {
        reject_time_bounds(&query)?;
        ensure_limit(query.limit)?;
        if self.brave_api_key.is_empty() {
            return Err(error(
                ErrorClass::Authentication,
                "web search API key is not configured",
            ));
        }
        let encoded = utf8_percent_encode(&query.text, NON_ALPHANUMERIC);
        let limit = query.limit.unwrap_or(10);
        let url =
            format!("https://api.search.brave.com/res/v1/web/search?q={encoded}&count={limit}");
        let body = checked_body(
            self.client.send(self.brave_get(url)?).await?,
            "Brave web search",
        )?;
        let response: BraveResponse = parse_json(&body)?;
        response
            .web
            .map(|web| web.results)
            .unwrap_or_default()
            .iter()
            .map(normalize_result)
            .collect()
    }

    async fn fetch_record(&self, target: Target) -> Result<Record, SourceError> {
        let url = guard::target_url(source_id(), &target)?;
        guard::validate_fetch_url(source_id(), &url)?;
        let body = checked_body(
            self.client
                .send(guard::public_get(source_id(), &url)?)
                .await?,
            "web fetch",
        )?;
        let text = String::from_utf8_lossy(&body[..body.len().min(MAX_FETCH_BYTES)]).to_string();
        record(json!({
            "id": format!("web:{}", canonical_url(&url)),
            "source": SOURCE_ID,
            "kind": "page",
            "url": canonical_url(&url),
            "title": null,
            "text": text,
            "author": null,
            "created_at": null,
            "updated_at": null,
            "metadata": {
                "fetched_bytes": body.len().min(MAX_FETCH_BYTES),
                "truncated": body.len() > MAX_FETCH_BYTES
            }
        }))
    }

    fn brave_get(&self, url: String) -> Result<Request<Vec<u8>>, SourceError> {
        Request::builder()
            .method("GET")
            .uri(url)
            .header("accept", "application/json")
            .header("user-agent", "comsat")
            .header("x-subscription-token", self.brave_api_key.as_str())
            .body(Vec::new())
            .map_err(protocol)
    }
}

#[async_trait::async_trait]
impl SourceRuntime for WebSource {
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
        self.fetch_record(target).await
    }

    async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
        Box::pin(async_stream::stream! {
            yield Err(error(ErrorClass::Unsupported, "web source does not advertise follow"));
        })
    }
}

#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    url: String,
    title: Option<String>,
    description: Option<String>,
    age: Option<String>,
    language: Option<String>,
    family_friendly: Option<bool>,
}

fn normalize_result(result: &BraveResult) -> Result<Record, SourceError> {
    let url = Url::parse(&result.url).map_err(protocol)?;
    let created_at = result.age.as_deref().and_then(valid_rfc3339);
    record(json!({
        "id": format!("web:{}", canonical_url(&url)),
        "source": SOURCE_ID,
        "kind": "search-result",
        "url": canonical_url(&url),
        "title": result.title,
        "text": result.description,
        "author": null,
        "created_at": created_at,
        "updated_at": null,
        "metadata": {
            "provider": "brave",
            "provider_age": result.age,
            "language": result.language,
            "family_friendly": result.family_friendly
        }
    }))
}

fn valid_rfc3339(value: &str) -> Option<&str> {
    OffsetDateTime::parse(value, &Rfc3339).ok()?;
    Some(value)
}

fn checked_body(response: Response<Vec<u8>>, source: &str) -> Result<Vec<u8>, SourceError> {
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let body = response.into_body();
    if status.is_success() {
        return Ok(body);
    }
    match status {
        StatusCode::UNAUTHORIZED => Err(error(
            ErrorClass::Authentication,
            format!("{source} authentication failed"),
        )),
        StatusCode::FORBIDDEN => Err(error(
            ErrorClass::Authorization,
            format!("{source} access denied"),
        )),
        StatusCode::TOO_MANY_REQUESTS => {
            let mut err = error(ErrorClass::RateLimit, format!("{source} was rate limited"));
            err.retry_after_seconds = retry_after;
            Err(err)
        }
        StatusCode::NOT_FOUND => Err(error(
            ErrorClass::NotFound,
            format!("{source} target was not found"),
        )),
        status => Err(error(
            ErrorClass::Upstream,
            format!("{source} returned HTTP {status}"),
        )),
    }
}

fn reject_time_bounds(query: &Query) -> Result<(), SourceError> {
    if query.since.is_some() || query.until.is_some() {
        return Err(error(
            ErrorClass::Unsupported,
            "web source does not support since or until bounds",
        ));
    }
    Ok(())
}

fn ensure_limit(limit: Option<u32>) -> Result<(), SourceError> {
    if limit.unwrap_or(10) > MAX_LIMIT {
        return Err(error(
            ErrorClass::InvalidQuery,
            format!("web source supports at most {MAX_LIMIT} results per search"),
        ));
    }
    Ok(())
}

fn canonical_url(url: &Url) -> String {
    let mut canonical = url.clone();
    canonical.set_fragment(None);
    canonical.to_string()
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
