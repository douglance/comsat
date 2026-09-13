use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::ValidationError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Query {
    pub text: String,
    pub limit: Option<u32>,
    pub since: Option<String>,
    pub until: Option<String>,
}

pub fn validate_timestamp(value: &str) -> Result<OffsetDateTime, ValidationError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| ValidationError("timestamp must be RFC 3339".into()))
}

impl Query {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.text.trim().is_empty() || self.text.len() > 8192 {
            return Err(ValidationError(
                "query text must contain 1 to 8192 bytes".into(),
            ));
        }
        if self.limit.is_some_and(|n| n == 0 || n > 1000) {
            return Err(ValidationError(
                "query limit must be between 1 and 1000".into(),
            ));
        }
        let since = self.since.as_deref().map(validate_timestamp).transpose()?;
        let until = self.until.as_deref().map(validate_timestamp).transpose()?;
        if since.zip(until).is_some_and(|(start, end)| start > end) {
            return Err(ValidationError("since must not be later than until".into()));
        }
        Ok(())
    }
}
