#![forbid(unsafe_code)]

use std::sync::Arc;

mod cooldown;
mod query;
mod target;

use comsat_source::{
    HttpClient, RecordStream, SourceDescriptor, SourceProfile, SourceRunContext, SourceRuntime,
};
use comsat_types::{ErrorClass, Query, Record, SourceError, SourceId, Target};
use http::{Request, Response, StatusCode};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE_ID: &str = "github";

#[derive(Clone)]
pub struct GitHubSource {
    client: Arc<dyn HttpClient>,
    token: Option<String>,
    cooldowns: Arc<cooldown::Cooldowns>,
}

impl GitHubSource {
    pub fn new(client: Arc<dyn HttpClient>, token: Option<String>) -> Self {
        Self {
            client,
            token,
            cooldowns: Arc::default(),
        }
    }

    pub fn descriptor() -> SourceDescriptor {
        SourceDescriptor::new(
            source_id(),
            "GitHub",
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
        let query_text = query::github_query_text(&query);
        let encoded = utf8_percent_encode(&query_text, NON_ALPHANUMERIC);
        let limit = query.limit.unwrap_or(10);
        let url = format!(
            "https://api.github.com/search/issues?q={encoded}&per_page={limit}&sort=updated&order=desc"
        );
        let body = self.checked_body("search", self.client.send(self.get(url)?).await?)?;
        let search: SearchResponse = parse_json(&body)?;
        search.items.iter().map(normalize_issue).collect()
    }

    async fn fetch_record(&self, target: Target) -> Result<Record, SourceError> {
        self.cooldowns.check(source_id(), "fetch")?;
        let target = target::IssueTarget::from_target(source_id(), &target)?;
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/{}",
            target.encoded_owner(),
            target.encoded_repo(),
            target.number
        );
        let body = self.checked_body("fetch", self.client.send(self.get(url)?).await?)?;
        normalize_issue(&parse_json(&body)?)
    }

    async fn follow_records(&self, target: Target) -> Result<Vec<Record>, SourceError> {
        self.cooldowns.check(source_id(), "follow")?;
        let target = target::IssueTarget::from_target(source_id(), &target)?;
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/{}/comments?per_page={}",
            target.encoded_owner(),
            target.encoded_repo(),
            target.number,
            query::MAX_LIMIT
        );
        let body = self.checked_body("follow", self.client.send(self.get(url)?).await?)?;
        let comments: Vec<IssueComment> = parse_json(&body)?;
        comments
            .iter()
            .map(|comment| normalize_comment(comment, &target))
            .collect()
    }

    fn get(&self, url: String) -> Result<Request<Vec<u8>>, SourceError> {
        let mut builder = Request::builder()
            .method("GET")
            .uri(url)
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "comsat")
            .header("x-github-api-version", "2022-11-28");
        if let Some(token) = &self.token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        builder.body(Vec::new()).map_err(protocol)
    }

    fn checked_body(
        &self,
        operation: &'static str,
        response: Response<Vec<u8>>,
    ) -> Result<Vec<u8>, SourceError> {
        match checked_body(response) {
            Ok(body) => Ok(body),
            Err(error) => {
                self.cooldowns.observe_error(operation, &error);
                Err(error)
            }
        }
    }
}

#[async_trait::async_trait]
impl SourceRuntime for GitHubSource {
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

#[derive(Debug, Deserialize)]
struct SearchResponse {
    items: Vec<Issue>,
}

#[derive(Debug, Deserialize)]
struct Issue {
    id: u64,
    node_id: Option<String>,
    html_url: String,
    title: String,
    body: Option<String>,
    user: Option<User>,
    created_at: Option<String>,
    updated_at: Option<String>,
    number: u64,
    state: Option<String>,
    comments: Option<u64>,
    repository_url: Option<String>,
    pull_request: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct IssueComment {
    id: u64,
    node_id: Option<String>,
    html_url: String,
    body: Option<String>,
    user: Option<User>,
    created_at: Option<String>,
    updated_at: Option<String>,
    reactions: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct User {
    login: String,
}

fn normalize_issue(issue: &Issue) -> Result<Record, SourceError> {
    let repository = issue
        .repository_url
        .as_deref()
        .and_then(|url| url.strip_prefix("https://api.github.com/repos/"));
    let kind = if issue.pull_request.is_some() {
        "pull-request"
    } else {
        "issue"
    };
    record(json!({
        "id": issue.node_id.clone().unwrap_or_else(|| format!("github:issue:{}", issue.id)),
        "source": SOURCE_ID,
        "kind": kind,
        "url": issue.html_url,
        "title": issue.title,
        "text": issue.body,
        "author": issue.user.as_ref().map(|user| user.login.as_str()),
        "created_at": issue.created_at,
        "updated_at": issue.updated_at,
        "metadata": {
            "github_id": issue.id,
            "node_id": issue.node_id,
            "repository": repository,
            "number": issue.number,
            "state": issue.state,
            "comments": issue.comments
        }
    }))
}

fn normalize_comment(
    comment: &IssueComment,
    issue: &target::IssueTarget,
) -> Result<Record, SourceError> {
    record(json!({
        "id": comment.node_id.clone().unwrap_or_else(|| format!("github:comment:{}", comment.id)),
        "source": SOURCE_ID,
        "kind": "issue-comment",
        "url": comment.html_url,
        "title": null,
        "text": comment.body,
        "author": comment.user.as_ref().map(|user| user.login.as_str()),
        "created_at": comment.created_at,
        "updated_at": comment.updated_at,
        "metadata": {
            "github_id": comment.id,
            "node_id": comment.node_id,
            "repository": format!("{}/{}", issue.owner, issue.repo),
            "number": issue.number,
            "reactions": comment.reactions
        }
    }))
}

fn checked_body(response: Response<Vec<u8>>) -> Result<Vec<u8>, SourceError> {
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
            "GitHub authentication failed",
        )),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
            let mut err = error(ErrorClass::RateLimit, "GitHub request was rate limited");
            err.retry_after_seconds = retry_after;
            Err(err)
        }
        StatusCode::NOT_FOUND => Err(error(ErrorClass::NotFound, "GitHub target was not found")),
        status => Err(error(
            ErrorClass::Upstream,
            format!("GitHub returned HTTP {status}"),
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
