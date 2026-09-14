//! GitHub API payloads and their mapping onto canonical COMSAT records.

use comsat_types::{ErrorClass, Record, SourceError, SourceId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::target::RepositoryRef;

pub const SOURCE_ID: &str = "github";

#[derive(Debug, Deserialize)]
pub struct SearchResponse<T> {
    pub items: Vec<T>,
}

#[derive(Debug, Deserialize)]
pub struct Issue {
    pub id: u64,
    pub node_id: Option<String>,
    pub html_url: String,
    pub title: String,
    pub body: Option<String>,
    pub user: Option<User>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub number: u64,
    pub state: Option<String>,
    pub comments: Option<u64>,
    pub repository_url: Option<String>,
    pub pull_request: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub node_id: Option<String>,
    pub html_url: String,
    pub body: Option<String>,
    pub user: Option<User>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub reactions: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct Repository {
    pub id: u64,
    pub node_id: Option<String>,
    pub html_url: String,
    pub full_name: String,
    pub description: Option<String>,
    pub owner: Option<User>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub stargazers_count: Option<u64>,
    pub forks_count: Option<u64>,
    pub open_issues_count: Option<u64>,
    pub language: Option<String>,
    pub default_branch: Option<String>,
    pub topics: Option<Vec<String>>,
    pub archived: Option<bool>,
}

/// A pull-request review comment: one entry in a code review discussion.
#[derive(Debug, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub node_id: Option<String>,
    pub html_url: String,
    pub body: Option<String>,
    pub user: Option<User>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub in_reply_to_id: Option<u64>,
    pub pull_request_review_id: Option<u64>,
}

/// A submitted review: the verdict that heads a review discussion.
#[derive(Debug, Deserialize)]
pub struct Review {
    pub id: u64,
    pub node_id: Option<String>,
    pub html_url: String,
    pub body: Option<String>,
    pub user: Option<User>,
    pub state: Option<String>,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
}

pub fn issue(issue: &Issue) -> Result<Record, SourceError> {
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

pub fn issue_comment(
    comment: &IssueComment,
    repository: &RepositoryRef,
    number: u64,
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
            "repository": repository.name_with_owner(),
            "number": number,
            "reactions": comment.reactions
        }
    }))
}

pub fn repository(repository: &Repository) -> Result<Record, SourceError> {
    record(json!({
        "id": repository.node_id.clone().unwrap_or_else(|| format!("github:repository:{}", repository.id)),
        "source": SOURCE_ID,
        "kind": "repository",
        "url": repository.html_url,
        "title": repository.full_name,
        "text": repository.description,
        "author": repository.owner.as_ref().map(|owner| owner.login.as_str()),
        "created_at": repository.created_at,
        "updated_at": repository.updated_at,
        "metadata": {
            "github_id": repository.id,
            "node_id": repository.node_id,
            "repository": repository.full_name,
            "stars": repository.stargazers_count,
            "forks": repository.forks_count,
            "open_issues": repository.open_issues_count,
            "language": repository.language,
            "default_branch": repository.default_branch,
            "topics": repository.topics,
            "archived": repository.archived
        }
    }))
}

pub fn review_comment(
    comment: &ReviewComment,
    repository: &RepositoryRef,
    number: u64,
) -> Result<Record, SourceError> {
    record(json!({
        "id": comment.node_id.clone().unwrap_or_else(|| format!("github:review-comment:{}", comment.id)),
        "source": SOURCE_ID,
        "kind": "review-comment",
        "url": comment.html_url,
        "title": null,
        "text": comment.body,
        "author": comment.user.as_ref().map(|user| user.login.as_str()),
        "created_at": comment.created_at,
        "updated_at": comment.updated_at,
        "metadata": {
            "github_id": comment.id,
            "node_id": comment.node_id,
            "repository": repository.name_with_owner(),
            "number": number,
            "path": comment.path,
            "line": comment.line,
            "in_reply_to": comment.in_reply_to_id,
            "review_id": comment.pull_request_review_id
        }
    }))
}

pub fn review(
    review: &Review,
    repository: &RepositoryRef,
    number: u64,
) -> Result<Record, SourceError> {
    record(json!({
        "id": review.node_id.clone().unwrap_or_else(|| format!("github:review:{}", review.id)),
        "source": SOURCE_ID,
        "kind": "review",
        "url": review.html_url,
        "title": review.state.clone(),
        "text": review.body,
        "author": review.user.as_ref().map(|user| user.login.as_str()),
        "created_at": review.submitted_at,
        "updated_at": review.submitted_at,
        "metadata": {
            "github_id": review.id,
            "node_id": review.node_id,
            "repository": repository.name_with_owner(),
            "number": number,
            "state": review.state
        }
    }))
}

pub fn record(value: Value) -> Result<Record, SourceError> {
    serde_json::from_value(value).map_err(protocol)
}

pub fn source_id() -> SourceId {
    SourceId::new(SOURCE_ID).expect("static source id is valid")
}

pub fn error(class: ErrorClass, message: impl Into<String>) -> SourceError {
    SourceError::new(source_id(), class, message)
}

pub fn protocol(message: impl core::fmt::Display) -> SourceError {
    error(ErrorClass::Protocol, message.to_string())
}
