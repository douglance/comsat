use comsat_store::Watch;
use comsat_types::{Record, SourceId, Target};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchArgs {
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchOptions {
    #[serde(default)]
    pub text: Option<String>,
    pub limit: Option<u32>,
    pub since: Option<String>,
    pub until: Option<String>,
    #[serde(default)]
    pub source: Vec<String>,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TargetOptions {
    #[serde(rename = "type", alias = "target_kind")]
    pub target_type: String,
    pub source: Option<SourceId>,
    pub url: Option<String>,
    pub id: Option<String>,
    pub record: Option<Record>,
    #[serde(default)]
    pub strict: bool,
}

impl TargetOptions {
    pub(crate) fn into_target(self) -> Result<Target, String> {
        let target = match self.target_type.as_str() {
            "record" => self
                .record
                .map(|record| Target::Record {
                    record: Box::new(record),
                })
                .ok_or_else(|| "record target requires record".to_string())?,
            "url" => Target::Url {
                source: self
                    .source
                    .ok_or_else(|| "url target requires source".to_string())?,
                url: self
                    .url
                    .ok_or_else(|| "url target requires url".to_string())?,
            },
            "native" => Target::Native {
                source: self
                    .source
                    .ok_or_else(|| "native target requires source".to_string())?,
                id: self
                    .id
                    .ok_or_else(|| "native target requires id".to_string())?,
            },
            other => return Err(format!("unsupported target type: {other}")),
        };
        target.validate().map_err(|error| error.to_string())?;
        Ok(target)
    }
}

#[derive(Debug, Deserialize, incurs::Args)]
pub struct SourceArgs {
    pub source: String,
}

#[derive(Debug, Deserialize, incurs::Options)]
pub struct WatchAddOptions {
    pub watch_id: Option<String>,
    pub query: String,
    pub interval_seconds: Option<u64>,
    #[serde(default)]
    pub source: Vec<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, incurs::Options)]
pub struct WatchDeleteOptions {
    pub watch: String,
}

#[derive(Debug, Deserialize, incurs::Options)]
pub struct HistoryOptions {
    pub watch: Option<String>,
    pub since: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceListOutput {
    pub sources: Vec<SourceSummary>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceInspectOutput {
    pub source: SourceSummary,
    pub commands: SourceCommandSummary,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceDoctorOutput {
    pub sources: Vec<SourceHealth>,
    pub store_configured: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceTestOutput {
    pub sources_checked: usize,
    pub failures: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceSummary {
    pub id: String,
    pub display_name: String,
    pub supports_search: bool,
    pub supports_fetch: bool,
    pub supports_follow: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceCommandSummary {
    pub search: Option<String>,
    pub fetch: Option<String>,
    pub follow: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceHealth {
    pub id: String,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WatchListOutput {
    pub watches: Vec<Watch>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WatchDeleteOutput {
    pub deleted: bool,
}

pub fn source_summary(source: &comsat_engine::CatalogSource) -> SourceSummary {
    SourceSummary {
        id: source.descriptor.id.to_string(),
        display_name: source.descriptor.display_name.clone(),
        supports_search: source.descriptor.profile.search,
        supports_fetch: source.descriptor.profile.fetch,
        supports_follow: source.descriptor.profile.follow,
    }
}
