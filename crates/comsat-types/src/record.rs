use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{RecordId, SourceId, ValidationError, validate_timestamp};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Record {
    pub id: RecordId,
    pub source: SourceId,
    pub kind: String,
    pub url: String,
    pub title: Option<String>,
    pub text: Option<String>,
    pub author: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    #[schemars(with = "serde_json::Map<String, Value>")]
    pub metadata: Value,
}

pub fn validate_url(value: &str) -> Result<(), ValidationError> {
    let url = url::Url::parse(value).map_err(|_| ValidationError("invalid URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ValidationError(
            "URL must be absolute HTTP(S), without credentials".into(),
        ));
    }
    Ok(())
}

impl Record {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.kind.trim().is_empty() || self.kind.len() > 128 {
            return Err(ValidationError(
                "record kind must contain 1 to 128 bytes".into(),
            ));
        }
        validate_url(&self.url)?;
        for value in [&self.created_at, &self.updated_at].into_iter().flatten() {
            validate_timestamp(value)?;
        }
        if !self.metadata.is_object() {
            return Err(ValidationError("metadata must be a JSON object".into()));
        }
        Ok(())
    }
}
