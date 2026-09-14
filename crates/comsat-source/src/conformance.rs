use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::task::Poll;

use comsat_types::{ErrorClass, Query, Record, RecordId, SourceError, Target};
use futures::FutureExt;
use incurs::cli::Cli;
use incurs::tool::{ToolCallControl, ToolCallOptions, ToolCallOutcome, ToolCatalog};
use thiserror::Error;

use crate::{OperationKind, SourceDescriptor, SourceRuntime, source_commands};

mod cancellation;
mod schema;

use cancellation::{RequestLedger, observe_runtime};
use schema::validate_schema_document;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "conformance reports expose independent check coverage flags"
)]
pub struct ConformanceReport {
    pub source_id: String,
    pub search_checked: bool,
    pub fetch_checked: bool,
    pub follow_checked: bool,
    pub error_checked: bool,
    pub cancellation_checked: bool,
}

pub struct ConformanceFixtures {
    pub query: Query,
    pub target: Option<Target>,
    pub error: Option<SourceErrorFixture>,
    pub cancellation: Option<CancellationFixture>,
}

pub struct SourceErrorFixture {
    pub query: Query,
    pub expected_class: ErrorClass,
}

pub struct CancellationFixture {
    pub query: Query,
}

#[derive(Debug, Error)]
pub enum ConformanceError {
    #[error("source profile must advertise search")]
    MissingSearch,
    #[error("missing tool: {0}")]
    MissingTool(String),
    #[error("invalid tool schema for {tool}: {reason}")]
    InvalidToolSchema { tool: String, reason: String },
    #[error("invalid fixture: {0}")]
    InvalidFixture(String),
    #[error("search fixture emitted no records")]
    EmptySearch,
    #[error("record ids changed between repeat search runs")]
    UnstableRecordIds,
    #[error("source error code is not a COMSAT error class: {0}")]
    UnstructuredSourceError(String),
    #[error("expected source error class {expected:?}, got {actual:?}")]
    UnexpectedErrorClass {
        expected: ErrorClass,
        actual: ErrorClass,
    },
    #[error("cancellation fixture completed before cancellation was observed")]
    CancellationFinishedEarly,
    #[error("cancellation was not propagated through the ToolCatalog")]
    CancellationNotPropagated,
    #[error("cancelled call never reached the source request")]
    CancellationNotObserved,
    #[error("cancelled call left {0} source request(s) pending")]
    CancellationLeakedRequest(usize),
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
        self.run_with_fixtures(
            runtime,
            ConformanceFixtures {
                query,
                target,
                error: None,
                cancellation: None,
            },
        )
        .await
    }

    pub async fn run_with_fixtures(
        &self,
        runtime: Arc<dyn SourceRuntime>,
        fixtures: ConformanceFixtures,
    ) -> Result<ConformanceReport, ConformanceError> {
        if !self.descriptor.profile.search {
            return Err(ConformanceError::MissingSearch);
        }
        validate_fixtures(&fixtures)?;
        let ledger = Arc::new(RequestLedger::default());
        let catalog = self.catalog(observe_runtime(runtime, Arc::clone(&ledger)))?;
        self.require_tool(&catalog, OperationKind::Search)?;
        let first = self.search_records(&catalog, &fixtures.query).await?;
        let second = self.search_records(&catalog, &fixtures.query).await?;
        if first.is_empty() {
            return Err(ConformanceError::EmptySearch);
        }
        if record_ids(&first) != record_ids(&second) {
            return Err(ConformanceError::UnstableRecordIds);
        }
        let (fetch_checked, follow_checked) = self
            .check_target_operations(&catalog, fixtures.target)
            .await?;
        let error_checked = self.check_error(&catalog, fixtures.error).await?;
        let cancellation_checked = self
            .check_cancellation(&catalog, fixtures.cancellation, &ledger)
            .await?;
        Ok(ConformanceReport {
            source_id: self.descriptor.id.as_str().to_owned(),
            search_checked: true,
            fetch_checked,
            follow_checked,
            error_checked,
            cancellation_checked,
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
        validate_schema_document(name, &definition.input_schema)?;
        if let Some(output_schema) = &definition.output_schema {
            validate_schema_document(name, output_schema)?;
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

    async fn check_error(
        &self,
        catalog: &ToolCatalog,
        fixture: Option<SourceErrorFixture>,
    ) -> Result<bool, ConformanceError> {
        let Some(fixture) = fixture else {
            return Ok(false);
        };
        call_expected_error(
            catalog,
            self.tool(OperationKind::Search)?,
            query_arguments(&fixture.query),
            fixture.expected_class,
        )
        .await?;
        Ok(true)
    }

    async fn check_cancellation(
        &self,
        catalog: &ToolCatalog,
        fixture: Option<CancellationFixture>,
        ledger: &RequestLedger,
    ) -> Result<bool, ConformanceError> {
        let Some(fixture) = fixture else {
            return Ok(false);
        };
        let opened = ledger.opened();
        let live = ledger.live();
        call_and_cancel(
            catalog,
            self.tool(OperationKind::Search)?,
            query_arguments(&fixture.query),
        )
        .await?;
        if ledger.opened() == opened {
            return Err(ConformanceError::CancellationNotObserved);
        }
        let leaked = ledger.live().saturating_sub(live);
        if leaked > 0 {
            return Err(ConformanceError::CancellationLeakedRequest(leaked));
        }
        Ok(true)
    }
}

fn validate_fixtures(fixtures: &ConformanceFixtures) -> Result<(), ConformanceError> {
    fixtures
        .query
        .validate()
        .map_err(|error| ConformanceError::InvalidFixture(error.to_string()))?;
    if let Some(target) = &fixtures.target {
        target
            .validate()
            .map_err(|error| ConformanceError::InvalidFixture(error.to_string()))?;
    }
    if let Some(error) = &fixtures.error {
        error
            .query
            .validate()
            .map_err(|error| ConformanceError::InvalidFixture(error.to_string()))?;
    }
    if let Some(cancellation) = &fixtures.cancellation {
        cancellation
            .query
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

async fn call_expected_error(
    catalog: &ToolCatalog,
    tool: &str,
    arguments: BTreeMap<String, serde_json::Value>,
    expected_class: ErrorClass,
) -> Result<(), ConformanceError> {
    let outcome = catalog
        .call(tool, arguments, ToolCallOptions::isolated())
        .await;
    match outcome {
        ToolCallOutcome::Error { code, .. } => {
            let actual = error_class_from_code(&code)?;
            if actual == expected_class {
                return Ok(());
            }
            Err(ConformanceError::UnexpectedErrorClass {
                expected: expected_class,
                actual,
            })
        }
        ToolCallOutcome::Ok { .. } => Err(ConformanceError::Tool(
            "expected source error fixture to fail".to_string(),
        )),
    }
}

async fn call_and_cancel(
    catalog: &ToolCatalog,
    tool: &str,
    arguments: BTreeMap<String, serde_json::Value>,
) -> Result<(), ConformanceError> {
    let control = ToolCallControl::default();
    let cancellation = control.cancellation.clone();
    let options = ToolCallOptions {
        control,
        ..ToolCallOptions::isolated()
    };
    let call = catalog.call(tool, arguments, options);
    futures::pin_mut!(call);
    wait_until_pending(&mut call).await?;
    cancellation.cancel();
    match call.await {
        ToolCallOutcome::Error { code, .. } if code == "CANCELLED" => Ok(()),
        ToolCallOutcome::Error { .. } | ToolCallOutcome::Ok { .. } => {
            Err(ConformanceError::CancellationNotPropagated)
        }
    }
}

async fn wait_until_pending<F>(future: &mut std::pin::Pin<&mut F>) -> Result<(), ConformanceError>
where
    F: Future<Output = ToolCallOutcome>,
{
    futures::future::poll_fn(|context| match future.poll_unpin(context) {
        Poll::Ready(_) => Poll::Ready(Err(ConformanceError::CancellationFinishedEarly)),
        Poll::Pending => Poll::Ready(Ok(())),
    })
    .await
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

fn error_class_from_code(code: &str) -> Result<ErrorClass, ConformanceError> {
    serde_json::from_value(serde_json::Value::String(code.to_owned()))
        .map_err(|_| ConformanceError::UnstructuredSourceError(code.to_owned()))
}

#[cfg(test)]
mod tests;
