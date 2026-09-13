#![forbid(unsafe_code)]

use std::sync::Arc;

mod cooldown;
mod normalize;
mod query;
mod target;

use comsat_source::{
    HttpClient, RecordStream, SourceDescriptor, SourceProfile, SourceRunContext, SourceRuntime,
};
use comsat_types::{ErrorClass, Query, Record, SourceError, SourceId, Target};
use http::{Request, Response, StatusCode};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use normalize::{
    Answer, Question, StackResponse, normalize_answer, normalize_question, parse_stack_response,
};

const SOURCE_ID: &str = "stack-exchange";
const DEFAULT_SITE: &str = "stackoverflow";

#[derive(Clone)]
pub struct StackExchangeSource {
    client: Arc<dyn HttpClient>,
    site: String,
    api_key: Option<String>,
    cooldowns: Arc<cooldown::Cooldowns>,
}

impl StackExchangeSource {
    pub fn new(client: Arc<dyn HttpClient>, site: Option<String>, api_key: Option<String>) -> Self {
        Self {
            client,
            site: site.unwrap_or_else(|| DEFAULT_SITE.into()),
            api_key,
            cooldowns: Arc::default(),
        }
    }

    pub fn descriptor() -> SourceDescriptor {
        SourceDescriptor::new(
            source_id(),
            "Stack Exchange",
            SourceProfile {
                search: true,
                fetch: true,
                follow: true,
            },
        )
    }

    async fn search_records(&self, query: Query) -> Result<Vec<Record>, SourceError> {
        query::ensure_limit(source_id(), query.limit)?;
        self.cooldowns.check(source_id(), "search")?;
        let encoded = utf8_percent_encode(&query.text, NON_ALPHANUMERIC);
        let date_query = query::date_query(source_id(), &query)?;
        let limit = query.limit.unwrap_or(10);
        let site = self.encoded_site();
        let url = self.api_url(&format!(
            "/2.3/search/advanced?order=desc&sort=relevance&q={encoded}&site={site}&pagesize={limit}{date_query}",
        ));
        let response: StackResponse<Question> =
            self.parse_http_response("search", &read_body(self.client.send(get(url)?).await?))?;
        self.cooldowns.observe_backoff("search", response.backoff);
        response
            .items
            .iter()
            .map(|question| normalize_question(source_id(), question, &self.site))
            .collect()
    }

    async fn fetch_record(&self, target: Target) -> Result<Record, SourceError> {
        self.cooldowns.check(source_id(), "fetch")?;
        let id = target::question_id(source_id(), &self.site, &target)?;
        let site = self.encoded_site();
        let url = self.api_url(&format!(
            "/2.3/questions/{id}?order=desc&sort=activity&site={site}&filter=withbody",
        ));
        let response: StackResponse<Question> =
            self.parse_http_response("fetch", &read_body(self.client.send(get(url)?).await?))?;
        self.cooldowns.observe_backoff("fetch", response.backoff);
        response
            .items
            .first()
            .ok_or_else(|| {
                error(
                    ErrorClass::NotFound,
                    "Stack Exchange question was not found",
                )
            })
            .and_then(|question| normalize_question(source_id(), question, &self.site))
    }

    async fn follow_records(&self, target: Target) -> Result<Vec<Record>, SourceError> {
        self.cooldowns.check(source_id(), "follow")?;
        let id = target::question_id(source_id(), &self.site, &target)?;
        let site = self.encoded_site();
        let url = self.api_url(&format!(
            "/2.3/questions/{id}/answers?order=desc&sort=activity&site={}&filter=withbody&pagesize={}",
            site,
            query::MAX_LIMIT
        ));
        let response: StackResponse<Answer> =
            self.parse_http_response("follow", &read_body(self.client.send(get(url)?).await?))?;
        self.cooldowns.observe_backoff("follow", response.backoff);
        response
            .items
            .iter()
            .map(|answer| normalize_answer(source_id(), answer, &self.site, id))
            .collect()
    }

    fn api_url(&self, path_and_query: &str) -> String {
        let key = self
            .api_key
            .as_ref()
            .map(|key| format!("&key={}", query_value(key)))
            .unwrap_or_default();
        format!("https://api.stackexchange.com{path_and_query}{key}")
    }

    fn encoded_site(&self) -> String {
        query_value(&self.site)
    }

    fn parse_http_response<T: for<'de> serde::Deserialize<'de>>(
        &self,
        operation: &'static str,
        response: &ResponseBody,
    ) -> Result<StackResponse<T>, SourceError> {
        if response.status.is_success() {
            return self.parse_response(operation, &response.body);
        }
        match self.parse_response::<T>(operation, &response.body) {
            Err(error) if error.class != ErrorClass::Protocol => Err(error),
            _ => {
                let error = http_error(response.status, response.retry_after_seconds);
                self.cooldowns
                    .observe_backoff(operation, error.retry_after_seconds);
                Err(error)
            }
        }
    }

    fn parse_response<T: for<'de> serde::Deserialize<'de>>(
        &self,
        operation: &'static str,
        body: &[u8],
    ) -> Result<StackResponse<T>, SourceError> {
        parse_stack_response(source_id(), body).inspect_err(|error| {
            self.cooldowns
                .observe_backoff(operation, error.retry_after_seconds);
        })
    }
}

#[async_trait::async_trait]
impl SourceRuntime for StackExchangeSource {
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

    async fn follow(&self, target: Target, _context: SourceRunContext) -> RecordStream {
        let source = self.clone();
        Box::pin(async_stream::stream! {
            match source.follow_records(target).await {
                Ok(records) => for record in records { yield Ok(record); },
                Err(error) => yield Err(error),
            }
        })
    }
}

fn get(url: String) -> Result<Request<Vec<u8>>, SourceError> {
    Request::builder()
        .method("GET")
        .uri(url)
        .header("accept", "application/json")
        .header("user-agent", "comsat")
        .body(Vec::new())
        .map_err(protocol)
}

struct ResponseBody {
    status: StatusCode,
    retry_after_seconds: Option<u64>,
    body: Vec<u8>,
}

fn read_body(response: Response<Vec<u8>>) -> ResponseBody {
    let status = response.status();
    let retry_after_seconds = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let body = response.into_body();
    ResponseBody {
        status,
        retry_after_seconds,
        body,
    }
}

fn http_error(status: StatusCode, retry_after_seconds: Option<u64>) -> SourceError {
    let mut error = match status {
        StatusCode::UNAUTHORIZED => error(
            ErrorClass::Authentication,
            "Stack Exchange authentication failed",
        ),
        StatusCode::FORBIDDEN => error(ErrorClass::Authorization, "Stack Exchange access denied"),
        StatusCode::TOO_MANY_REQUESTS => error(
            ErrorClass::RateLimit,
            "Stack Exchange request was rate limited",
        ),
        StatusCode::NOT_FOUND => error(ErrorClass::NotFound, "Stack Exchange target was not found"),
        status => error(
            ErrorClass::Upstream,
            format!("Stack Exchange returned HTTP {status}"),
        ),
    };
    if matches!(error.class, ErrorClass::RateLimit) {
        error.retry_after_seconds = retry_after_seconds;
    }
    error
}

fn query_value(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
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
