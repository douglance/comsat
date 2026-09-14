use std::borrow::Borrow;
use std::sync::Arc;

use comsat_types::{ErrorClass, Query, SourceError, Target};
use futures::StreamExt;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, McpAnnotations, McpCommandOptions,
};
use incurs::output::{CommandResult, StreamRecord};
use incurs::schema::{FieldMeta, FieldType};
use schemars::JsonSchema;
use serde_json::Value;

use crate::{OperationKind, SourceDescriptor, SourceRunContext, SourceRuntime};

#[derive(Clone)]
pub struct SourceCommand {
    descriptor: SourceDescriptor,
    operation: OperationKind,
    runtime: Arc<dyn SourceRuntime>,
    input_schema: Value,
}

pub fn source_commands(
    descriptor: impl Borrow<SourceDescriptor>,
    runtime: Arc<dyn SourceRuntime>,
) -> Vec<CommandDef> {
    let descriptor = descriptor.borrow();
    let mut commands = Vec::new();
    commands.push(command_for(
        descriptor,
        OperationKind::Search,
        Arc::clone(&runtime),
    ));
    if descriptor.profile.fetch {
        commands.push(command_for(
            descriptor,
            OperationKind::Fetch,
            Arc::clone(&runtime),
        ));
    }
    if descriptor.profile.follow {
        commands.push(command_for(descriptor, OperationKind::Follow, runtime));
    }
    commands
}

fn command_for(
    descriptor: &SourceDescriptor,
    operation: OperationKind,
    runtime: Arc<dyn SourceRuntime>,
) -> CommandDef {
    let name = descriptor
        .tool_name(operation)
        .expect("command construction must match descriptor profile")
        .to_owned();
    let handler = SourceCommand {
        descriptor: descriptor.clone(),
        operation,
        runtime,
        input_schema: input_schema(operation),
    };
    let description = format!(
        "{} {} source records",
        descriptor.display_name,
        operation.as_str()
    );
    let mut command = CommandDef::build(operation.as_str(), handler)
        .description(description.clone())
        .mcp(McpCommandOptions {
            name: Some(name),
            description: Some(description),
            annotations: Some(McpAnnotations {
                title: Some(format!(
                    "{} {}",
                    descriptor.display_name,
                    operation.as_str()
                )),
                read_only_hint: Some(true),
                destructive_hint: Some(false),
                idempotent_hint: Some(true),
                open_world_hint: Some(true),
            }),
            ..McpCommandOptions::default()
        })
        .done();
    command.options_fields = input_fields(operation);
    command.output_schema = Some(output_schema(operation));
    command
}

#[async_trait::async_trait]
impl CommandHandler for SourceCommand {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let _ = ctx.request;
        let context = SourceRunContext::default();
        match self.operation {
            OperationKind::Search => self.search(ctx.options, context).await,
            OperationKind::Fetch => self.fetch(ctx.options, context).await,
            OperationKind::Follow => self.follow(ctx.options, context).await,
        }
    }

    fn mcp_input_schema(&self) -> Option<&Value> {
        Some(&self.input_schema)
    }
}

impl SourceCommand {
    async fn search(&self, options: Value, context: SourceRunContext) -> CommandResult {
        let query = match query_from_options(options) {
            Ok(query) => query,
            Err(error) => return command_error(self.local_error(error)),
        };
        let stream = self.runtime.search(query, context).await;
        CommandResult::RecordStream(Box::pin(stream.map(source_result_to_record)))
    }

    async fn fetch(&self, options: Value, context: SourceRunContext) -> CommandResult {
        let target = match target_from_options(options) {
            Ok(target) => target,
            Err(error) => return command_error(self.local_error(error)),
        };
        if let Err(error) = self.validate_target_source(&target) {
            return command_error(error);
        }
        match self.runtime.fetch(target, context).await {
            Ok(record) => chunk_result(record),
            Err(error) => command_error(error),
        }
    }

    async fn follow(&self, options: Value, context: SourceRunContext) -> CommandResult {
        let target = match target_from_options(options) {
            Ok(target) => target,
            Err(error) => return command_error(self.local_error(error)),
        };
        if let Err(error) = self.validate_target_source(&target) {
            return command_error(error);
        }
        let stream = self.runtime.follow(target, context).await;
        CommandResult::RecordStream(Box::pin(stream.map(source_result_to_record)))
    }
}

fn source_result_to_record(result: Result<comsat_types::Record, SourceError>) -> StreamRecord {
    match result {
        Ok(record) => match validate_and_serialize(record) {
            Ok(value) => StreamRecord::Chunk(value),
            Err(error) => StreamRecord::Error {
                code: "serialization".to_string(),
                message: error,
                retryable: false,
                exit_code: Some(1),
                cta: None,
            },
        },
        Err(error) => source_error_record(error),
    }
}

fn source_error_record(error: SourceError) -> StreamRecord {
    StreamRecord::Error {
        code: error_code(error.class),
        message: error.message,
        retryable: matches!(error.class, ErrorClass::RateLimit | ErrorClass::Timeout),
        exit_code: Some(1),
        cta: None,
    }
}

fn command_error(error: SourceError) -> CommandResult {
    CommandResult::Error {
        code: error_code(error.class),
        message: error.message,
        retryable: matches!(error.class, ErrorClass::RateLimit | ErrorClass::Timeout),
        exit_code: Some(1),
        cta: None,
    }
}

fn chunk_result(record: comsat_types::Record) -> CommandResult {
    match validate_and_serialize(record) {
        Ok(data) => CommandResult::Ok {
            data,
            cta: None,
            exit_code: None,
        },
        Err(error) => CommandResult::Error {
            code: "serialization".to_string(),
            message: error,
            retryable: false,
            exit_code: Some(1),
            cta: None,
        },
    }
}

fn validate_and_serialize(record: comsat_types::Record) -> Result<Value, String> {
    record
        .validate()
        .map_err(|error| error.to_string())
        .and_then(|()| serde_json::to_value(record).map_err(|error| error.to_string()))
}

fn query_from_options(options: Value) -> Result<Query, String> {
    let query: Query = serde_json::from_value(options).map_err(|error| error.to_string())?;
    query.validate().map_err(|error| error.to_string())?;
    Ok(query)
}

fn target_from_options(options: Value) -> Result<Target, String> {
    let target: Target = serde_json::from_value(options).map_err(|error| error.to_string())?;
    target.validate().map_err(|error| error.to_string())?;
    Ok(target)
}

impl SourceCommand {
    fn local_error(&self, error: impl std::fmt::Display) -> SourceError {
        SourceError::new(
            self.descriptor.id.clone(),
            ErrorClass::InvalidQuery,
            error.to_string(),
        )
    }

    fn validate_target_source(&self, target: &Target) -> Result<(), SourceError> {
        if target.source() == &self.descriptor.id {
            return Ok(());
        }
        Err(SourceError::new(
            self.descriptor.id.clone(),
            ErrorClass::InvalidQuery,
            "target source does not match command source",
        ))
    }
}

fn error_code(class: ErrorClass) -> String {
    serde_json::to_value(class)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{class:?}").to_lowercase())
}

fn input_schema(operation: OperationKind) -> Value {
    match operation {
        OperationKind::Search => schema_for::<Query>(),
        OperationKind::Fetch | OperationKind::Follow => schema_for::<Target>(),
    }
}

fn output_schema(operation: OperationKind) -> Value {
    match operation {
        OperationKind::Search | OperationKind::Follow => schema_for::<Vec<comsat_types::Record>>(),
        OperationKind::Fetch => schema_for::<comsat_types::Record>(),
    }
}

fn schema_for<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema must serialize")
}

fn input_fields(operation: OperationKind) -> Vec<FieldMeta> {
    match operation {
        OperationKind::Search => search_fields(),
        OperationKind::Fetch | OperationKind::Follow => target_fields(),
    }
}

fn search_fields() -> Vec<FieldMeta> {
    vec![
        field("text", "text", FieldType::String, true),
        field("limit", "limit", FieldType::Number, false),
        field("since", "since", FieldType::String, false),
        field("until", "until", FieldType::String, false),
    ]
}

fn target_fields() -> Vec<FieldMeta> {
    vec![
        field("type", "type", FieldType::String, true),
        field("source", "source", FieldType::String, false),
        field("url", "url", FieldType::String, false),
        field("id", "id", FieldType::String, false),
        field("record", "record", FieldType::Value, false),
    ]
}

fn field(
    name: &'static str,
    cli_name: &'static str,
    field_type: FieldType,
    required: bool,
) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: cli_name.to_string(),
        description: None,
        field_type,
        required,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use futures::executor::block_on;
    use futures::stream;
    use incurs::cli::Cli;
    use incurs::tool::{ToolCallOptions, ToolCallOutcome};

    use super::*;
    use crate::{RecordStream, SourceProfile};

    struct EmptyRuntime;
    struct CountingRuntime {
        fetches: Arc<AtomicUsize>,
        follows: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SourceRuntime for EmptyRuntime {
        async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
            Box::pin(stream::empty())
        }

        async fn fetch(
            &self,
            _target: Target,
            _context: SourceRunContext,
        ) -> Result<comsat_types::Record, SourceError> {
            Err(SourceError::new(
                "test".parse().expect("valid source id"),
                ErrorClass::Unsupported,
                "not implemented",
            ))
        }

        async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
            Box::pin(stream::empty())
        }
    }

    #[async_trait]
    impl SourceRuntime for CountingRuntime {
        async fn search(&self, _query: Query, _context: SourceRunContext) -> RecordStream {
            Box::pin(stream::empty())
        }

        async fn fetch(
            &self,
            _target: Target,
            _context: SourceRunContext,
        ) -> Result<comsat_types::Record, SourceError> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            Err(SourceError::new(
                "test".parse().expect("valid source id"),
                ErrorClass::Internal,
                "fetch should not run",
            ))
        }

        async fn follow(&self, _target: Target, _context: SourceRunContext) -> RecordStream {
            self.follows.fetch_add(1, Ordering::SeqCst);
            Box::pin(stream::empty())
        }
    }

    #[test]
    fn source_command_schemas_match_tool_contract() {
        let source_id = "test".parse().expect("valid source id");
        let descriptor = SourceDescriptor::new(
            source_id,
            "Test",
            SourceProfile {
                search: true,
                fetch: true,
                follow: true,
            },
        );
        let mut cli = Cli::create("test");
        for command in source_commands(&descriptor, Arc::new(EmptyRuntime)) {
            cli = cli.command(command.name.clone(), command);
        }
        let catalog = cli.tool_catalog();
        let search = catalog
            .get(&descriptor.commands.search)
            .expect("search tool exists");
        assert_eq!(
            search
                .output_schema
                .as_ref()
                .and_then(|schema| schema.get("type")),
            Some(&serde_json::Value::String("array".to_string()))
        );
        let fetch = catalog
            .get(&descriptor.commands.fetch)
            .expect("fetch tool exists");
        assert_eq!(fetch.input_schema, schema_for::<Target>());
        assert!(
            target_from_options(serde_json::json!({
                "type": "url",
                "source": "test",
                "url": "https://example.com/item"
            }))
            .is_ok()
        );
    }

    #[test]
    fn fetch_rejects_mismatched_target_source_before_runtime_dispatch() {
        let descriptor = SourceDescriptor::new(
            "test".parse().expect("valid source id"),
            "Test",
            SourceProfile {
                search: true,
                fetch: true,
                follow: true,
            },
        );
        let fetches = Arc::new(AtomicUsize::new(0));
        let follows = Arc::new(AtomicUsize::new(0));
        let runtime = Arc::new(CountingRuntime {
            fetches: Arc::clone(&fetches),
            follows: Arc::clone(&follows),
        });
        let mut cli = Cli::create("test");
        for command in source_commands(&descriptor, runtime) {
            cli = cli.command(command.name.clone(), command);
        }
        let catalog = cli.tool_catalog();
        let outcome = block_on(catalog.call(
            &descriptor.commands.fetch,
            object_arguments(&serde_json::json!({
                "type": "url",
                "source": "other",
                "url": "https://example.com/item"
            })),
            ToolCallOptions::isolated(),
        ));

        assert!(matches!(
            outcome,
            ToolCallOutcome::Error { code, .. } if code == "invalid_query"
        ));
        assert_eq!(fetches.load(Ordering::SeqCst), 0);
        let outcome = block_on(catalog.call(
            &descriptor.commands.follow,
            object_arguments(&serde_json::json!({
                "type": "native",
                "source": "other",
                "id": "item-1"
            })),
            ToolCallOptions::isolated(),
        ));

        assert!(matches!(
            outcome,
            ToolCallOutcome::Error { code, .. } if code == "invalid_query"
        ));
        assert_eq!(follows.load(Ordering::SeqCst), 0);
    }

    fn object_arguments(value: &Value) -> BTreeMap<String, Value> {
        value
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}
