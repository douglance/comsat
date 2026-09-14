mod errors;
mod schema;
mod search;
mod types;
mod watch;

use std::sync::Arc;

use comsat_source::{ConformanceSuite, OperationKind};
use comsat_store::{DeleteWatch, HistoryRequest};
use comsat_types::SourceId;
use incurs::{
    cli::Cli,
    command::{CommandContext, CommandDef, CommandHandler, TypedResult},
    output::{CommandResult, Format, StreamRecord},
};

use crate::{AppError, ComsatApp};
use schema::read_only_mcp;
use search::{fetch_command, follow_command, search_command};
use types::{
    HistoryOptions, SourceArgs, SourceCommandSummary, SourceDoctorOutput, SourceHealth,
    SourceInspectOutput, SourceListOutput, SourceTestOutput, WatchAddOptions, WatchDeleteOptions,
    WatchDeleteOutput, WatchListOutput, source_summary,
};

pub fn build_cli(app: Arc<ComsatApp>) -> Cli {
    Cli::create("comsat")
        .description("Composable search for humans and agents")
        .version(env!("CARGO_PKG_VERSION"))
        .command("search", search_command(Arc::clone(&app)))
        .command("fetch", fetch_command(Arc::clone(&app)))
        .command("follow", follow_command(Arc::clone(&app)))
        .group(source_group(Arc::clone(&app)))
        .group(watch_group(Arc::clone(&app)))
        .command("history", history_command(app))
}

fn source_group(app: Arc<ComsatApp>) -> Cli {
    Cli::create("source")
        .description("Inspect COMSAT sources")
        .command("list", source_list_command(Arc::clone(&app)))
        .command("inspect", source_inspect_command(Arc::clone(&app)))
        .command("doctor", source_doctor_command(Arc::clone(&app)))
        .command("test", source_test_command(app))
}

fn watch_group(app: Arc<ComsatApp>) -> Cli {
    Cli::create("watch")
        .description("Manage COMSAT watches")
        .command("add", watch_add_command(Arc::clone(&app)))
        .command("list", watch_list_command(Arc::clone(&app)))
        .command("delete", watch_delete_command(app))
}

fn source_list_command(app: Arc<ComsatApp>) -> CommandDef {
    CommandDef::typed::<(), (), (), SourceListOutput, _, _>("list", move |_| {
        let app = Arc::clone(&app);
        async move {
            TypedResult::ok(SourceListOutput {
                sources: app.catalog.sources().iter().map(source_summary).collect(),
            })
        }
    })
    .description("List enabled COMSAT sources")
    .mcp(read_only_mcp("source_list", "List enabled COMSAT sources"))
    .done()
}

fn source_inspect_command(app: Arc<ComsatApp>) -> CommandDef {
    CommandDef::typed::<SourceArgs, (), (), SourceInspectOutput, _, _>("inspect", move |ctx| {
        let app = Arc::clone(&app);
        async move {
            let Ok(source_id) = SourceId::new(ctx.args.source) else {
                return TypedResult::error("invalid_query", "invalid source id");
            };
            let Some(source) = app.catalog.source(&source_id) else {
                return TypedResult::error("not_found", "source was not found");
            };
            TypedResult::ok(SourceInspectOutput {
                source: source_summary(source),
                commands: SourceCommandSummary {
                    search: source
                        .descriptor
                        .tool_name(OperationKind::Search)
                        .map(ToOwned::to_owned),
                    fetch: source
                        .descriptor
                        .tool_name(OperationKind::Fetch)
                        .map(ToOwned::to_owned),
                    follow: source
                        .descriptor
                        .tool_name(OperationKind::Follow)
                        .map(ToOwned::to_owned),
                },
            })
        }
    })
    .description("Inspect one COMSAT source")
    .mcp(read_only_mcp("source_inspect", "Inspect one COMSAT source"))
    .done()
}

fn source_doctor_command(app: Arc<ComsatApp>) -> CommandDef {
    CommandDef::typed::<(), (), (), SourceDoctorOutput, _, _>("doctor", move |_| {
        let app = Arc::clone(&app);
        async move {
            TypedResult::ok(SourceDoctorOutput {
                sources: app.catalog.sources().iter().map(source_health).collect(),
                store_configured: app.store.is_some(),
            })
        }
    })
    .description("Validate COMSAT source registration")
    .mcp(read_only_mcp(
        "source_doctor",
        "Validate COMSAT source registration",
    ))
    .done()
}

fn source_health(source: &comsat_engine::CatalogSource) -> SourceHealth {
    SourceHealth {
        id: source.descriptor.id.to_string(),
        ok: source.catalog.definitions().iter().any(|definition| {
            source.descriptor.tool_name(OperationKind::Search) == Some(definition.name.as_str())
        }),
        message: "source graph is registered".to_string(),
    }
}

fn source_test_command(app: Arc<ComsatApp>) -> CommandDef {
    CommandDef::typed::<(), (), (), SourceTestOutput, _, _>("test", move |_| {
        let app = Arc::clone(&app);
        async move {
            let failures = source_registration_failures(&app);
            let sources_checked = app.catalog.sources().len();
            if failures.is_empty() {
                TypedResult::ok(SourceTestOutput {
                    sources_checked,
                    failures,
                })
            } else {
                TypedResult::ok_with_exit_code(
                    SourceTestOutput {
                        sources_checked,
                        failures,
                    },
                    1,
                )
            }
        }
    })
    .description("Check source command graph consistency")
    .mcp(read_only_mcp(
        "source_test",
        "Check source command graph consistency",
    ))
    .done()
}

/// Every registered source must advertise the operations its profile declares
/// and expose internally consistent tool schemas for them. Fixture-driven
/// conformance stays in the test suite; this is the part that can run against a
/// live catalog without calling any upstream.
fn source_registration_failures(app: &ComsatApp) -> Vec<String> {
    let mut failures = Vec::new();
    for source in app.catalog.sources() {
        for operation in [
            OperationKind::Search,
            OperationKind::Fetch,
            OperationKind::Follow,
        ] {
            if let Some(tool) = source.descriptor.tool_name(operation)
                && source.catalog.get(tool).is_none()
            {
                failures.push(format!("{} missing {tool}", source.descriptor.id));
            }
        }
        if let Err(error) =
            ConformanceSuite::new(source.descriptor.clone()).check_contracts(&source.catalog)
        {
            failures.push(format!("{}: {error}", source.descriptor.id));
        }
    }
    failures
}

fn watch_add_command(app: Arc<ComsatApp>) -> CommandDef {
    CommandDef::typed::<(), WatchAddOptions, (), comsat_store::Watch, _, _>("add", move |ctx| {
        let app = Arc::clone(&app);
        async move {
            match watch::watch_add(&app, ctx.options).await {
                Ok(watch) => TypedResult::ok(watch),
                Err(error) => typed_app_error(error),
            }
        }
    })
    .description("Add a COMSAT watch")
    .done()
}

fn watch_list_command(app: Arc<ComsatApp>) -> CommandDef {
    CommandDef::typed::<(), (), (), WatchListOutput, _, _>("list", move |_| {
        let app = Arc::clone(&app);
        async move {
            match app.store() {
                Ok(store) => match store.list_watches(&app.tenant_id).await {
                    Ok(watches) => TypedResult::ok(WatchListOutput { watches }),
                    Err(error) => typed_store_error(&error),
                },
                Err(error) => typed_app_error(error),
            }
        }
    })
    .description("List COMSAT watches")
    .mcp(read_only_mcp("watch_list", "List COMSAT watches"))
    .done()
}

fn watch_delete_command(app: Arc<ComsatApp>) -> CommandDef {
    CommandDef::typed::<(), WatchDeleteOptions, (), WatchDeleteOutput, _, _>("delete", move |ctx| {
        let app = Arc::clone(&app);
        async move {
            match app.store() {
                Ok(store) => match store
                    .delete_watch(DeleteWatch {
                        tenant_id: app.tenant_id.clone(),
                        watch_id: ctx.options.watch,
                    })
                    .await
                {
                    Ok(deleted) => TypedResult::ok(WatchDeleteOutput { deleted }),
                    Err(error) => typed_store_error(&error),
                },
                Err(error) => typed_app_error(error),
            }
        }
    })
    .description("Delete a COMSAT watch")
    .done()
}

fn history_command(app: Arc<ComsatApp>) -> CommandDef {
    let mut command = CommandDef::build("history", HistoryCommand { app })
        .description("List observed COMSAT records")
        .options::<HistoryOptions>()
        .mcp(read_only_mcp("history", "List observed COMSAT records"))
        .done();
    command.format = Some(Format::Jsonl);
    command.output_schema = Some(schema::record_array_schema());
    command
}

struct HistoryCommand {
    app: Arc<ComsatApp>,
}

#[async_trait::async_trait]
impl CommandHandler for HistoryCommand {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let options = match history_options(ctx.options) {
            Ok(options) => options,
            Err(result) => return result,
        };
        match self.history_records(options).await {
            Ok(records) => history_record_stream(records),
            Err(result) => result,
        }
    }
}

impl HistoryCommand {
    async fn history_records(
        &self,
        options: HistoryOptions,
    ) -> Result<Vec<comsat_store::RecordObservation>, CommandResult> {
        let since_epoch_seconds = parse_history_since(options.since.as_deref(), self.app.now())?;
        let store = self
            .app
            .store()
            .map_err(|error| history_error("operational", error, false, 1))?;
        store
            .history(HistoryRequest {
                tenant_id: self.app.tenant_id.clone(),
                watch_id: options.watch,
                since_epoch_seconds,
                limit: options.limit.unwrap_or(100),
            })
            .await
            .map_err(|error| history_error("operational", error, false, 1))
    }
}

fn history_options(value: serde_json::Value) -> Result<HistoryOptions, CommandResult> {
    serde_json::from_value(value).map_err(|error| history_error("invalid_query", error, false, 2))
}

fn parse_history_since(value: Option<&str>, now: i64) -> Result<Option<i64>, CommandResult> {
    value
        .map(|value| {
            watch::parse_since(value, now)
                .map_err(|message| history_error("invalid_query", message, false, 2))
        })
        .transpose()
}

fn history_record_stream(records: Vec<comsat_store::RecordObservation>) -> CommandResult {
    CommandResult::RecordStream(Box::pin(futures::stream::iter(
        records.into_iter().map(history_record_chunk),
    )))
}

fn history_record_chunk(observation: comsat_store::RecordObservation) -> StreamRecord {
    serde_json::to_value(observation.record).map_or_else(
        |error| StreamRecord::Error {
            code: "protocol".to_string(),
            message: error.to_string(),
            retryable: false,
            exit_code: Some(1),
            cta: None,
        },
        StreamRecord::Chunk,
    )
}

fn typed_app_error<T>(error: AppError) -> TypedResult<T> {
    match error {
        AppError::StoreUnavailable => TypedResult::error("operational", error.to_string()),
        AppError::Store(error) => typed_store_error(&error),
    }
}

fn typed_store_error<T>(error: &comsat_store::StoreError) -> TypedResult<T> {
    TypedResult::error("operational", error.to_string())
}

fn history_error(
    code: impl Into<String>,
    error: impl std::fmt::Display,
    retryable: bool,
    exit_code: i32,
) -> CommandResult {
    CommandResult::Error {
        code: code.into(),
        message: error.to_string(),
        retryable,
        exit_code: Some(exit_code),
        cta: None,
    }
}
