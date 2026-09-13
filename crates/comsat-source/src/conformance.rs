use std::collections::BTreeMap;
use std::sync::Arc;

use comsat_types::{Query, Record, RecordId, SourceError, Target};
use incurs::cli::Cli;
use incurs::tool::{ToolCallOptions, ToolCallOutcome, ToolCatalog};
use thiserror::Error;

use crate::{OperationKind, SourceDescriptor, SourceRuntime, source_commands};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceReport {
    pub source_id: String,
    pub search_checked: bool,
    pub fetch_checked: bool,
    pub follow_checked: bool,
}

#[derive(Debug, Error)]
pub enum ConformanceError {
    #[error("source profile must advertise search")]
    MissingSearch,
    #[error("missing tool: {0}")]
    MissingTool(String),
    #[error("invalid fixture: {0}")]
    InvalidFixture(String),
    #[error("search fixture emitted no records")]
    EmptySearch,
    #[error("record ids changed between repeat search runs")]
    UnstableRecordIds,
    #[error("source returned an error: {0}")]
    Source(#[from] SourceError),
    #[error("tool call failed: {0}")]
    Tool(String),
}

pub struct ConformanceSuite {
    descriptor: SourceDescriptor,
}

impl ConformanceSuite {
    #[must_use]
    pub const fn new(descriptor: SourceDescriptor) -> Self {
        Self { descriptor }
    }

    pub async fn run(
        &self,
        runtime: Arc<dyn SourceRuntime>,
        query: Query,
        target: Option<Target>,
    ) -> Result<ConformanceReport, ConformanceError> {
        if !self.descriptor.profile.search {
            return Err(ConformanceError::MissingSearch);
        }
        validate_fixtures(&query, target.as_ref())?;
        let catalog = self.catalog(runtime)?;
        self.require_tool(&catalog, OperationKind::Search)?;
        let first = self.search_records(&catalog, &query).await?;
        let second = self.search_records(&catalog, &query).await?;
        if first.is_empty() {
            return Err(ConformanceError::EmptySearch);
        }
        if record_ids(&first) != record_ids(&second) {
            return Err(ConformanceError::UnstableRecordIds);
        }
        let (fetch_checked, follow_checked) =
            self.check_target_operations(&catalog, target).await?;
        Ok(ConformanceReport {
            source_id: self.descriptor.id.as_str().to_owned(),
            search_checked: true,
            fetch_checked,
            follow_checked,
        })
    }

    fn catalog(&self, runtime: Arc<dyn SourceRuntime>) -> Result<ToolCatalog, ConformanceError> {
        let mut cli = Cli::create(format!("comsat-source-{}", self.descriptor.id));
        for command in source_commands(&self.descriptor, runtime) {
            cli = cli.command(command.name.clone(), command);
        }
        cli.try_tool_catalog()
            .map_err(|error| ConformanceError::Tool(error.to_string()))
    }

    fn require_tool(
        &self,
        catalog: &ToolCatalog,
        operation: OperationKind,
    ) -> Result<(), ConformanceError> {
        let name = self.tool(operation)?;
        let definition = catalog
            .get(name)
            .ok_or_else(|| ConformanceError::MissingTool(name.to_owned()))?;
        if definition.input_schema.is_null() || definition.output_schema.is_none() {
            return Err(ConformanceError::MissingTool(name.to_owned()));
        }
        Ok(())
    }

    async fn search_records(
        &self,
        catalog: &ToolCatalog,
        query: &Query,
    ) -> Result<Vec<Record>, ConformanceError> {
        call_records(
            catalog,
            self.tool(OperationKind::Search)?,
            query_arguments(query),
        )
        .await
    }

    fn tool(&self, operation: OperationKind) -> Result<&str, ConformanceError> {
        self.descriptor
            .tool_name(operation)
            .ok_or_else(|| ConformanceError::MissingTool(operation.as_str().to_string()))
    }

    async fn check_target_operations(
        &self,
        catalog: &ToolCatalog,
        target: Option<Target>,
    ) -> Result<(bool, bool), ConformanceError> {
        let Some(target) = target else {
            return Ok((false, false));
        };
        let fetch_checked = self.check_fetch(catalog, &target).await?;
        let follow_checked = self.check_follow(catalog, &target).await?;
        Ok((fetch_checked, follow_checked))
    }

    async fn check_fetch(
        &self,
        catalog: &ToolCatalog,
        target: &Target,
    ) -> Result<bool, ConformanceError> {
        if !self.descriptor.profile.fetch {
            return Ok(false);
        }
        self.require_tool(catalog, OperationKind::Fetch)?;
        validate_record(
            call_record(
                catalog,
                self.tool(OperationKind::Fetch)?,
                target_arguments(target),
            )
            .await?,
        )?;
        Ok(true)
    }

    async fn check_follow(
        &self,
        catalog: &ToolCatalog,
        target: &Target,
    ) -> Result<bool, ConformanceError> {
        if !self.descriptor.profile.follow {
            return Ok(false);
        }
        self.require_tool(catalog, OperationKind::Follow)?;
        for record in call_records(
            catalog,
            self.tool(OperationKind::Follow)?,
            target_arguments(target),
        )
        .await?
        {
            validate_record(record)?;
        }
        Ok(true)
    }
}

fn validate_fixtures(query: &Query, target: Option<&Target>) -> Result<(), ConformanceError> {
    query
        .validate()
        .map_err(|error| ConformanceError::InvalidFixture(error.to_string()))?;
    if let Some(target) = target {
        target
            .validate()
            .map_err(|error| ConformanceError::InvalidFixture(error.to_string()))?;
    }
    Ok(())
}

async fn call_records(
    catalog: &ToolCatalog,
    tool: &str,
    arguments: BTreeMap<String, serde_json::Value>,
) -> Result<Vec<Record>, ConformanceError> {
    let outcome = catalog
        .call(tool, arguments, ToolCallOptions::isolated())
        .await;
    match outcome {
        ToolCallOutcome::Ok { data, .. } => serde_json::from_value::<Vec<Record>>(data)
            .map_err(|error| ConformanceError::Tool(error.to_string()))?
            .into_iter()
            .map(validate_record)
            .collect(),
        ToolCallOutcome::Error { message, .. } => Err(ConformanceError::Tool(message)),
    }
}

async fn call_record(
    catalog: &ToolCatalog,
    tool: &str,
    arguments: BTreeMap<String, serde_json::Value>,
) -> Result<Record, ConformanceError> {
    let outcome = catalog
        .call(tool, arguments, ToolCallOptions::isolated())
        .await;
    match outcome {
        ToolCallOutcome::Ok { data, .. } => serde_json::from_value::<Record>(data)
            .map_err(|error| ConformanceError::Tool(error.to_string())),
        ToolCallOutcome::Error { message, .. } => Err(ConformanceError::Tool(message)),
    }
}

fn query_arguments(query: &Query) -> BTreeMap<String, serde_json::Value> {
    object_arguments(&serde_json::to_value(query).expect("query must serialize"))
}

fn target_arguments(target: &Target) -> BTreeMap<String, serde_json::Value> {
    object_arguments(&serde_json::to_value(target).expect("target must serialize"))
}

fn object_arguments(value: &serde_json::Value) -> BTreeMap<String, serde_json::Value> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn validate_record(record: Record) -> Result<Record, ConformanceError> {
    record
        .validate()
        .map_err(|error| ConformanceError::InvalidFixture(error.to_string()))?;
    Ok(record)
}

fn record_ids(records: &[Record]) -> Vec<RecordId> {
    records.iter().map(|record| record.id.clone()).collect()
}
