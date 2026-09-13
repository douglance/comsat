use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Record, RecordId, SourceId, ValidationError, record::validate_url};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Target {
    Record { record: Box<Record> },
    Url { source: SourceId, url: String },
    Native { source: SourceId, id: String },
}

impl Target {
    pub fn source(&self) -> &SourceId {
        match self {
            Self::Record { record } => &record.source,
            Self::Url { source, .. } | Self::Native { source, .. } => source,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Record { record } => record.validate(),
            Self::Url { url, .. } => validate_url(url),
            Self::Native { id, .. } => RecordId::new(id.clone()).map(|_| ()),
        }
    }
}
