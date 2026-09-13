use comsat_types::{ErrorClass, Record, SourceError, SourceId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::query;

const SOURCE_ID: &str = "stack-exchange";

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct StackResponse<T> {
    #[serde(default)]
    pub items: Vec<T>,
    pub backoff: Option<u64>,
    error_id: Option<u64>,
    error_name: Option<String>,
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Question {
    #[serde(rename = "question_id")]
    pub id: u64,
    link: String,
    title: String,
    body: Option<String>,
    owner: Option<Owner>,
    creation_date: Option<u64>,
    last_activity_date: Option<u64>,
    score: Option<i64>,
    answer_count: Option<i64>,
    is_answered: Option<bool>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct Answer {
    #[serde(rename = "answer_id")]
    id: u64,
    question_id: Option<u64>,
    link: Option<String>,
    body: Option<String>,
    owner: Option<Owner>,
    creation_date: Option<u64>,
    last_activity_date: Option<u64>,
    score: Option<i64>,
    is_accepted: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct Owner {
    display_name: Option<String>,
    user_id: Option<u64>,
}

pub fn normalize_question(
    source: SourceId,
    question: &Question,
    site: &str,
) -> Result<Record, SourceError> {
    record(
        source,
        json!({
            "id": format!("stack-exchange:{site}:question:{}", question.id),
            "source": SOURCE_ID,
            "kind": "question",
            "url": question.link,
            "title": question.title,
            "text": question.body,
            "author": question.owner.as_ref().and_then(|owner| owner.display_name.as_deref()),
            "created_at": question.creation_date.and_then(query::unix_to_rfc3339),
            "updated_at": question.last_activity_date.and_then(query::unix_to_rfc3339),
            "metadata": {
                "site": site,
                "question_id": question.id,
                "score": question.score,
                "answer_count": question.answer_count,
                "is_answered": question.is_answered,
                "tags": question.tags,
                "owner_user_id": question.owner.as_ref().and_then(|owner| owner.user_id)
            }
        }),
    )
}

pub fn normalize_answer(
    source: SourceId,
    answer: &Answer,
    site: &str,
    fallback_question_id: u64,
) -> Result<Record, SourceError> {
    let question_id = answer.question_id.unwrap_or(fallback_question_id);
    let url = answer
        .link
        .clone()
        .unwrap_or_else(|| format!("https://stackoverflow.com/a/{}", answer.id));
    record(
        source,
        json!({
            "id": format!("stack-exchange:{site}:answer:{}", answer.id),
            "source": SOURCE_ID,
            "kind": "answer",
            "url": url,
            "title": null,
            "text": answer.body,
            "author": answer.owner.as_ref().and_then(|owner| owner.display_name.as_deref()),
            "created_at": answer.creation_date.and_then(query::unix_to_rfc3339),
            "updated_at": answer.last_activity_date.and_then(query::unix_to_rfc3339),
            "metadata": {
                "site": site,
                "answer_id": answer.id,
                "question_id": question_id,
                "score": answer.score,
                "is_accepted": answer.is_accepted,
                "owner_user_id": answer.owner.as_ref().and_then(|owner| owner.user_id)
            }
        }),
    )
}

pub fn parse_stack_response<T: for<'de> Deserialize<'de>>(
    source: SourceId,
    body: &[u8],
) -> Result<StackResponse<T>, SourceError> {
    let response: StackResponse<T> = serde_json::from_slice(body).map_err(|error| {
        SourceError::new(source.clone(), ErrorClass::Protocol, error.to_string())
    })?;
    if response.error_id.is_some() {
        let mut error = SourceError::new(
            source,
            stack_error_class(response.error_name.as_deref()),
            format!(
                "{}: {}",
                response
                    .error_name
                    .as_deref()
                    .unwrap_or("stack_exchange_error"),
                response.error_message.as_deref().unwrap_or("unknown error")
            ),
        );
        error.retry_after_seconds = response.backoff;
        return Err(error);
    }
    Ok(response)
}

fn stack_error_class(name: Option<&str>) -> ErrorClass {
    let name = name.unwrap_or_default();
    if name == "key_required" {
        return ErrorClass::Authentication;
    }
    [
        ("throttle", ErrorClass::RateLimit),
        ("access_token", ErrorClass::Authentication),
        ("auth", ErrorClass::Authentication),
        ("access_denied", ErrorClass::Authorization),
        ("bad_parameter", ErrorClass::InvalidQuery),
        ("invalid", ErrorClass::InvalidQuery),
        ("no_method", ErrorClass::InvalidQuery),
    ]
    .into_iter()
    .find_map(|(pattern, class)| name.contains(pattern).then_some(class))
    .unwrap_or(ErrorClass::Upstream)
}

fn record(source: SourceId, value: Value) -> Result<Record, SourceError> {
    serde_json::from_value(value)
        .map_err(|error| SourceError::new(source, ErrorClass::Protocol, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_official_error_taxonomy() {
        assert_eq!(
            stack_error_class(Some("bad_parameter")),
            ErrorClass::InvalidQuery
        );
        assert_eq!(
            stack_error_class(Some("key_required")),
            ErrorClass::Authentication
        );
        assert_eq!(
            stack_error_class(Some("access_denied")),
            ErrorClass::Authorization
        );
    }
}
