use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SourceId;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Authentication,
    Authorization,
    RateLimit,
    InvalidQuery,
    NotFound,
    Unsupported,
    Upstream,
    Timeout,
    Cancelled,
    Protocol,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceError {
    pub source: SourceId,
    pub class: ErrorClass,
    pub message: String,
    pub retry_after_seconds: Option<u64>,
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.source, self.message)
    }
}

impl std::error::Error for SourceError {}

impl SourceError {
    pub fn new(source: SourceId, class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            source,
            class,
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    pub fn authentication(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::Authentication, message)
    }

    pub fn authorization(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::Authorization, message)
    }

    pub fn rate_limit(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::RateLimit, message)
    }

    pub fn invalid_query(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::InvalidQuery, message)
    }

    pub fn not_found(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::NotFound, message)
    }

    pub fn unsupported(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::Unsupported, message)
    }

    pub fn upstream(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::Upstream, message)
    }

    pub fn timeout(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::Timeout, message)
    }

    pub fn cancelled(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::Cancelled, message)
    }

    pub fn protocol(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::Protocol, message)
    }

    pub fn internal(source: SourceId, message: impl Into<String>) -> Self {
        Self::new(source, ErrorClass::Internal, message)
    }
}
