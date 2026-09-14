//! Search across the GitHub object classes COMSAT exposes: issues and pull
//! requests, repositories, and discussions.

use comsat_types::{Query, Record, SourceError};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use crate::GitHubSource;
use crate::discussions::{self, Discussion};
use crate::normalize::{self, Issue, Repository, SearchResponse, source_id};
use crate::query::{DEFAULT_LIMIT, SearchKind, ensure_limit, github_search};
use crate::target::RepositoryRef;

impl GitHubSource {
    pub(crate) async fn search_records(&self, query: Query) -> Result<Vec<Record>, SourceError> {
        ensure_limit(source_id(), query.limit)?;
        self.cooldowns.check(source_id(), "search")?;
        let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
        let (kind, text) = github_search(&query);
        match kind {
            SearchKind::Issues => self.search_issues(&text, limit).await,
            SearchKind::Repositories => self.search_repositories(&text, limit).await,
            SearchKind::Discussions => self.search_discussions(&text, limit).await,
        }
    }

    async fn search_issues(&self, text: &str, limit: u32) -> Result<Vec<Record>, SourceError> {
        let url = search_url("issues", text, limit);
        let response: SearchResponse<Issue> = self.rest("search", url).await?;
        response.items.iter().map(normalize::issue).collect()
    }

    async fn search_repositories(
        &self,
        text: &str,
        limit: u32,
    ) -> Result<Vec<Record>, SourceError> {
        let url = search_url("repositories", text, limit);
        let response: SearchResponse<Repository> = self.rest("search", url).await?;
        response.items.iter().map(normalize::repository).collect()
    }

    async fn search_discussions(&self, text: &str, limit: u32) -> Result<Vec<Record>, SourceError> {
        let data = self
            .graphql("search", &discussions::search_request(text, limit))
            .await?;
        let nodes: Vec<Discussion> = discussions::nodes(&data, "/search/nodes")?;
        nodes
            .iter()
            .map(|discussion| {
                discussions::discussion_record(discussion, &repository_of(discussion))
            })
            .collect()
    }
}

/// Discussion search spans repositories, so each record carries the repository
/// GitHub reported rather than a queried one.
fn repository_of(discussion: &Discussion) -> RepositoryRef {
    let name_with_owner = discussion
        .repository
        .as_ref()
        .map_or("", |value| value.name_with_owner.as_str());
    let (owner, repo) = name_with_owner.split_once('/').unwrap_or(("", ""));
    RepositoryRef {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
    }
}

fn search_url(resource: &str, text: &str, limit: u32) -> String {
    let encoded = utf8_percent_encode(text, NON_ALPHANUMERIC);
    format!(
        "https://api.github.com/search/{resource}?q={encoded}&per_page={limit}&sort=updated&order=desc"
    )
}
