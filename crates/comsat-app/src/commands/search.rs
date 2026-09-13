use std::{collections::BTreeSet, pin::Pin, sync::Arc};

use comsat_engine::{EngineDiagnostic, EngineEvent, SearchRequest};
use comsat_types::{Query, SourceId};
use futures::{Stream, StreamExt};
use incurs::{
    command::{CommandContext, CommandDef, CommandHandler},
    output::{CommandResult, Format, StreamRecord},
};
use serde_json::Value;

use crate::{ComsatApp, TargetStream};

use super::{
    errors::{
        command_error, engine_error_record, engine_error_result, record_result, record_value,
        source_error_value, source_failures_stream_error, stream_error,
    },
    schema::{
        read_only_mcp, record_or_source_error_array_schema, record_schema, search_args_fields,
        search_fields, target_fields, target_input_schema,
    },
    types::{SearchArgs, SearchOptions, TargetOptions},
};

pub(super) fn search_command(app: Arc<ComsatApp>) -> CommandDef {
    let mut command = CommandDef::build("search", SearchCommand { app })
        .description("Search enabled information sources")
        .mcp(read_only_mcp(
            "comsat_search",
            "Search enabled COMSAT sources",
        ))
        .done();
    command.format = Some(Format::Jsonl);
    command.args_fields = search_args_fields();
    command.options_fields = search_fields();
    command.output_schema = Some(record_or_source_error_array_schema());
    command
}

pub(super) fn fetch_command(app: Arc<ComsatApp>) -> CommandDef {
    let mut command = CommandDef::build(
        "fetch",
        FetchCommand {
            app,
            mcp_input_schema: target_input_schema(),
        },
    )
    .description("Fetch one source record")
    .mcp(read_only_mcp(
        "comsat_fetch",
        "Fetch one COMSAT source record",
    ))
    .done();
    command.format = Some(Format::Jsonl);
    command.options_fields = target_fields();
    command.output_schema = Some(record_schema());
    command
}

pub(super) fn follow_command(app: Arc<ComsatApp>) -> CommandDef {
    let mut command = CommandDef::build(
        "follow",
        FollowCommand {
            app,
            mcp_input_schema: target_input_schema(),
        },
    )
    .description("Follow one source record for related records")
    .mcp(read_only_mcp(
        "comsat_follow",
        "Follow one COMSAT source record for related records",
    ))
    .done();
    command.format = Some(Format::Jsonl);
    command.options_fields = target_fields();
    command.output_schema = Some(record_or_source_error_array_schema());
    command
}

struct SearchCommand {
    app: Arc<ComsatApp>,
}

#[async_trait::async_trait]
impl CommandHandler for SearchCommand {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let args = match search_args(ctx.args) {
            Ok(args) => args,
            Err(message) => return command_error("invalid_query", message, false, 2),
        };
        let input = match search_options(ctx.options) {
            Ok(input) => input,
            Err(message) => return command_error("invalid_query", message, false, 2),
        };
        let sources = match parse_source_ids(&input.source) {
            Ok(sources) => sources,
            Err(message) => return command_error("invalid_query", message, false, 2),
        };
        let expected_sources = selected_source_count(&self.app, &sources);
        let query = Query {
            text: args.text,
            limit: input.limit,
            since: input.since,
            until: input.until,
        };
        if let Err(error) = query.validate() {
            return command_error("invalid_query", error.to_string(), false, 2);
        }
        let stream = self.app.catalog.search_stream(
            SearchRequest {
                query,
                sources,
                strict: input.strict,
            },
            self.app.limits,
        );
        CommandResult::RecordStream(search_record_stream(stream, input.strict, expected_sources))
    }
}

struct FetchCommand {
    app: Arc<ComsatApp>,
    mcp_input_schema: Value,
}

#[async_trait::async_trait]
impl CommandHandler for FetchCommand {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let options = ctx.options.clone();
        let target = match target_options(options.clone()).and_then(TargetOptions::into_target) {
            Ok(target) => target,
            Err(message) => match native_targets(&self.app, &ctx, &options) {
                Some(targets) => {
                    return CommandResult::RecordStream(fetch_targets_stream(
                        Arc::clone(&self.app),
                        targets,
                    ));
                }
                None => return command_error("invalid_query", message, false, 2),
            },
        };
        match self.app.catalog.fetch(target, self.app.limits).await {
            Ok(record) => record_result(record),
            Err(error) => engine_error_result(error),
        }
    }

    fn mcp_input_schema(&self) -> Option<&Value> {
        Some(&self.mcp_input_schema)
    }
}

struct FollowCommand {
    app: Arc<ComsatApp>,
    mcp_input_schema: Value,
}

#[async_trait::async_trait]
impl CommandHandler for FollowCommand {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let options = ctx.options.clone();
        let input = match target_options(options.clone()) {
            Ok(input) => input,
            Err(message) => match native_targets(&self.app, &ctx, &options) {
                Some(targets) => {
                    return CommandResult::RecordStream(follow_targets_stream(
                        Arc::clone(&self.app),
                        targets,
                    ));
                }
                None => return command_error("invalid_query", message, false, 2),
            },
        };
        let strict = input.strict;
        let target = match input.into_target() {
            Ok(target) => target,
            Err(message) => return command_error("invalid_query", message, false, 2),
        };
        let stream = self.app.catalog.follow_stream(target, self.app.limits);
        CommandResult::RecordStream(search_record_stream(stream, strict, 1))
    }

    fn mcp_input_schema(&self) -> Option<&Value> {
        Some(&self.mcp_input_schema)
    }
}

fn native_targets(app: &ComsatApp, ctx: &CommandContext, options: &Value) -> Option<TargetStream> {
    if ctx.request.is_some() || !options.as_object().is_some_and(serde_json::Map::is_empty) {
        return None;
    }
    let provider = app.target_stream_provider.as_ref()?;
    Some(provider.read_targets())
}

fn fetch_targets_stream(
    app: Arc<ComsatApp>,
    mut targets: TargetStream,
) -> Pin<Box<dyn Stream<Item = StreamRecord> + Send>> {
    Box::pin(async_stream::stream! {
        while let Some(target) = targets.next().await {
            let target = match target {
                Ok(target) => target,
                Err(message) => {
                    yield stream_error("invalid_query", message, false, 2);
                    break;
                }
            };
            match app.catalog.fetch(target, app.limits).await {
                Ok(record) => match record_value(record) {
                    Ok(value) => yield StreamRecord::Chunk(value),
                    Err(message) => {
                        yield stream_error("protocol", message, false, 1);
                        break;
                    }
                },
                Err(error) => {
                    yield engine_error_record(error);
                    break;
                }
            }
        }
    })
}

fn follow_targets_stream(
    app: Arc<ComsatApp>,
    mut targets: TargetStream,
) -> Pin<Box<dyn Stream<Item = StreamRecord> + Send>> {
    Box::pin(async_stream::stream! {
        while let Some(target) = targets.next().await {
            let target = match target {
                Ok(target) => target,
                Err(message) => {
                    yield stream_error("invalid_query", message, false, 2);
                    break;
                }
            };
            let stream = app.catalog.follow_stream(target, app.limits);
            let mut state = RecordStreamState::default();
            futures::pin_mut!(stream);
            while let Some(event) = stream.next().await {
                state.observe(&event);
                let terminal = state.terminal(false, 1);
                match event {
                    EngineEvent::Record(record) => match record_value(record) {
                        Ok(value) => yield StreamRecord::Chunk(value),
                        Err(message) => {
                            yield stream_error("protocol", message, false, 1);
                            return;
                        }
                    },
                    EngineEvent::SourceFailed(diagnostic) => {
                        yield StreamRecord::Chunk(source_error_value(&diagnostic));
                    }
                }
                if terminal {
                    yield source_failures_stream_error(&state.diagnostics, false);
                    return;
                }
            }
        }
    })
}

fn search_record_stream(
    stream: impl Stream<Item = EngineEvent> + Send + 'static,
    strict: bool,
    expected_sources: usize,
) -> Pin<Box<dyn Stream<Item = StreamRecord> + Send>> {
    Box::pin(async_stream::stream! {
        let mut state = RecordStreamState::default();
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            state.observe(&event);
            let terminal = state.terminal(strict, expected_sources);
            match event {
                EngineEvent::Record(record) => match record_value(record) {
                    Ok(value) => yield StreamRecord::Chunk(value),
                    Err(message) => {
                        yield stream_error("protocol", message, false, 1);
                        break;
                    }
                },
                EngineEvent::SourceFailed(diagnostic) => {
                    yield StreamRecord::Chunk(source_error_value(&diagnostic));
                }
            }
            if terminal {
                yield source_failures_stream_error(&state.diagnostics, strict);
                break;
            }
        }
    })
}

#[derive(Default)]
struct RecordStreamState {
    saw_record: bool,
    failed_sources: BTreeSet<SourceId>,
    diagnostics: Vec<EngineDiagnostic>,
}

impl RecordStreamState {
    fn observe(&mut self, event: &EngineEvent) {
        match event {
            EngineEvent::Record(_) => self.saw_record = true,
            EngineEvent::SourceFailed(diagnostic) => {
                self.failed_sources.insert(diagnostic.source.clone());
                self.diagnostics.push(diagnostic.clone());
            }
        }
    }

    fn terminal(&self, strict: bool, expected_sources: usize) -> bool {
        strict
            || (!self.saw_record
                && expected_sources > 0
                && self.failed_sources.len() >= expected_sources)
    }
}

fn selected_source_count(app: &ComsatApp, sources: &[SourceId]) -> usize {
    if sources.is_empty() {
        app.catalog.sources().len()
    } else {
        sources.iter().collect::<BTreeSet<_>>().len()
    }
}

fn search_options(value: Value) -> Result<SearchOptions, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn search_args(value: Value) -> Result<SearchArgs, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn target_options(value: Value) -> Result<TargetOptions, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

pub(super) fn parse_source_ids(raw: &[String]) -> Result<Vec<SourceId>, String> {
    raw.iter()
        .map(|value| SourceId::new(value.clone()).map_err(|error| error.to_string()))
        .collect()
}
