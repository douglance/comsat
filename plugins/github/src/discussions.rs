//! GitHub Discussions. The REST API does not expose repository discussions, so
//! these operations use the GraphQL API, which requires a configured token.

use comsat_types::{ErrorClass, Record, SourceError};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::normalize::{SOURCE_ID, error, protocol, record};
use crate::target::RepositoryRef;

pub const GRAPHQL_URL: &str = "https://api.github.com/graphql";

const DISCUSSION_FIELDS: &str = "id number title url createdAt updatedAt bodyText \
     author { login } category { name } comments { totalCount } \
     repository { nameWithOwner }";

pub fn search_request(text: &str, limit: u32) -> Value {
    json!({
        "query": format!(
            "query($q: String!, $n: Int!) {{ search(query: $q, type: DISCUSSION, first: $n) \
             {{ nodes {{ ... on Discussion {{ {DISCUSSION_FIELDS} }} }} }} }}"
        ),
        "variables": {"q": text, "n": limit},
    })
}

pub fn fetch_request(repository: &RepositoryRef, number: u64) -> Value {
    json!({
        "query": format!(
            "query($owner: String!, $name: String!, $number: Int!) \
             {{ repository(owner: $owner, name: $name) \
             {{ discussion(number: $number) {{ {fields} }} }} }}",
            fields = DISCUSSION_FIELDS
        ),
        "variables": {
            "owner": repository.owner,
            "name": repository.repo,
            "number": number,
        },
    })
}

pub fn repository_request(repository: &RepositoryRef, limit: u32) -> Value {
    json!({
        "query": format!(
            "query($owner: String!, $name: String!, $n: Int!) \
             {{ repository(owner: $owner, name: $name) \
             {{ discussions(first: $n, orderBy: {{field: UPDATED_AT, direction: DESC}}) \
             {{ nodes {{ {DISCUSSION_FIELDS} }} }} }} }}"
        ),
        "variables": {
            "owner": repository.owner,
            "name": repository.repo,
            "n": limit,
        },
    })
}

const COMMENTS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $n: Int!) \
     { repository(owner: $owner, name: $name) { discussion(number: $number) \
     { comments(first: $n) { nodes { id url bodyText createdAt updatedAt \
     author { login } } } } } }";

pub fn comments_request(repository: &RepositoryRef, number: u64, limit: u32) -> Value {
    json!({
        "query": COMMENTS_QUERY,
        "variables": {
            "owner": repository.owner,
            "name": repository.repo,
            "number": number,
            "n": limit,
        },
    })
}

/// Reads a GraphQL envelope, turning declared errors into source errors before
/// any data is trusted.
pub fn payload(body: &[u8]) -> Result<Value, SourceError> {
    let envelope: Value = serde_json::from_slice(body).map_err(protocol)?;
    if let Some(message) = first_error(&envelope) {
        return Err(graphql_error(&message));
    }
    envelope
        .get("data")
        .cloned()
        .ok_or_else(|| protocol("GitHub GraphQL response has no data"))
}

fn first_error(envelope: &Value) -> Option<String> {
    let errors = envelope.get("errors")?.as_array()?;
    let first = errors.first()?;
    Some(
        first
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("GitHub GraphQL request failed")
            .to_owned(),
    )
}

fn graphql_error(message: &str) -> SourceError {
    let lowered = message.to_ascii_lowercase();
    let class = if lowered.contains("could not resolve") || lowered.contains("not exist") {
        ErrorClass::NotFound
    } else if lowered.contains("rate limit") {
        ErrorClass::RateLimit
    } else if lowered.contains("credential") || lowered.contains("permission") {
        ErrorClass::Authentication
    } else {
        ErrorClass::Upstream
    };
    error(class, format!("GitHub GraphQL: {message}"))
}

#[derive(Debug, Deserialize)]
pub struct Discussion {
    pub id: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    #[serde(rename = "bodyText")]
    pub body_text: Option<String>,
    pub author: Option<Author>,
    pub category: Option<Category>,
    pub comments: Option<Count>,
    pub repository: Option<NameWithOwner>,
}

#[derive(Debug, Deserialize)]
pub struct DiscussionComment {
    pub id: String,
    pub url: String,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    #[serde(rename = "bodyText")]
    pub body_text: Option<String>,
    pub author: Option<Author>,
}

#[derive(Debug, Deserialize)]
pub struct Author {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct Category {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Count {
    #[serde(rename = "totalCount")]
    pub total_count: u64,
}

#[derive(Debug, Deserialize)]
pub struct NameWithOwner {
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
}

pub fn nodes<T: for<'de> Deserialize<'de>>(
    data: &Value,
    pointer: &str,
) -> Result<Vec<T>, SourceError> {
    let nodes = data
        .pointer(pointer)
        .ok_or_else(|| protocol(format!("GitHub GraphQL response is missing {pointer}")))?;
    serde_json::from_value(nodes.clone()).map_err(protocol)
}

pub fn node<T: for<'de> Deserialize<'de>>(data: &Value, pointer: &str) -> Result<T, SourceError> {
    let node = data.pointer(pointer).filter(|value| !value.is_null());
    let node =
        node.ok_or_else(|| error(ErrorClass::NotFound, "GitHub discussion was not found"))?;
    serde_json::from_value(node.clone()).map_err(protocol)
}

pub fn discussion_record(
    discussion: &Discussion,
    repository: &RepositoryRef,
) -> Result<Record, SourceError> {
    let name_with_owner = discussion.repository.as_ref().map_or_else(
        || repository.name_with_owner(),
        |value| value.name_with_owner.clone(),
    );
    record(json!({
        "id": discussion.id,
        "source": SOURCE_ID,
        "kind": "discussion",
        "url": discussion.url,
        "title": discussion.title,
        "text": discussion.body_text,
        "author": discussion.author.as_ref().map(|author| author.login.as_str()),
        "created_at": discussion.created_at,
        "updated_at": discussion.updated_at,
        "metadata": {
            "node_id": discussion.id,
            "repository": name_with_owner,
            "number": discussion.number,
            "category": discussion.category.as_ref().map(|category| category.name.as_str()),
            "comments": discussion.comments.as_ref().map(|comments| comments.total_count)
        }
    }))
}

pub fn comment_record(
    comment: &DiscussionComment,
    repository: &RepositoryRef,
    number: u64,
) -> Result<Record, SourceError> {
    record(json!({
        "id": comment.id,
        "source": SOURCE_ID,
        "kind": "discussion-comment",
        "url": comment.url,
        "title": null,
        "text": comment.body_text,
        "author": comment.author.as_ref().map(|author| author.login.as_str()),
        "created_at": comment.created_at,
        "updated_at": comment.updated_at,
        "metadata": {
            "node_id": comment.id,
            "repository": repository.name_with_owner(),
            "number": number
        }
    }))
}
