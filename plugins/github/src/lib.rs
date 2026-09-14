#![forbid(unsafe_code)]

use std::sync::Arc;

mod cooldown;
mod discussions;
mod follow;
mod normalize;
mod query;
mod search;
mod target;

use comsat_source::{
    HttpClient, RecordStream, SourceDescriptor, SourceProfile, SourceRunContext, SourceRuntime,
};
use comsat_types::{ErrorClass, Query, Record, SourceError, Target};
use http::{Request, Response, StatusCode};
use serde::Deserialize;
use serde_json::Value;

use normalize::{error, protocol, source_id};
use target::GitHubTarget;

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

    async fn fetch_record(&self, target: Target) -> Result<Record, SourceError> {
        self.cooldowns.check(source_id(), "fetch")?;
        match GitHubTarget::from_target(source_id(), &target)? {
            GitHubTarget::Repository(repository) => {
                let url = format!(
                    "https://api.github.com/repos/{}/{}",
                    repository.encoded_owner(),
                    repository.encoded_repo()
                );
                normalize::repository(&self.rest("fetch", url).await?)
            }
            GitHubTarget::Issue {
                repository, number, ..
            } => {
                let url = format!(
                    "https://api.github.com/repos/{}/{}/issues/{number}",
                    repository.encoded_owner(),
                    repository.encoded_repo()
                );
                normalize::issue(&self.rest("fetch", url).await?)
            }
            GitHubTarget::Discussion { repository, number } => {
                let data = self
                    .graphql("fetch", &discussions::fetch_request(&repository, number))
                    .await?;
                let discussion = discussions::node(&data, "/repository/discussion")?;
                discussions::discussion_record(&discussion, &repository)
            }
        }
    }

    /// Sends one GET to the REST API and deserializes its body.
    async fn rest<T: for<'de> Deserialize<'de>>(
        &self,
        operation: &'static str,
        url: String,
    ) -> Result<T, SourceError> {
        let body = self.checked_body(operation, self.client.send(self.get(url)?).await?)?;
        serde_json::from_slice(&body).map_err(protocol)
    }

    /// Sends one GraphQL request. Discussions are only reachable this way, and
    /// the GraphQL API rejects unauthenticated callers.
    async fn graphql(
        &self,
        operation: &'static str,
        request: &Value,
    ) -> Result<Value, SourceError> {
        if self.token.is_none() {
            return Err(error(
                ErrorClass::Authentication,
                "GitHub Discussions require COMSAT_GITHUB_TOKEN or GITHUB_TOKEN",
            ));
        }
        let body = serde_json::to_vec(request).map_err(protocol)?;
        let request = self
            .request("POST", discussions::GRAPHQL_URL.to_owned())
            .body(body)
            .map_err(protocol)?;
        let response = self.checked_body(operation, self.client.send(request).await?)?;
        discussions::payload(&response)
    }

    fn get(&self, url: String) -> Result<Request<Vec<u8>>, SourceError> {
        self.request("GET", url).body(Vec::new()).map_err(protocol)
    }

    fn request(&self, method: &str, url: String) -> http::request::Builder {
        let mut builder = Request::builder()
            .method(method)
            .uri(url)
            .header("accept", "application/vnd.github+json")
            .header("content-type", "application/json")
            .header("user-agent", "comsat")
            .header("x-github-api-version", "2022-11-28");
        if let Some(token) = &self.token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        builder
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
