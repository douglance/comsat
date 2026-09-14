#![forbid(unsafe_code)]

use comsat_types::{Query, Record, RecordId, SourceId};
use incurs::{
    cli::Cli,
    command::{CommandContext, CommandDef, CommandHandler, TypedContext, TypedResult},
    errors::{FieldError, ValidationError},
    output::CommandResult,
    schema::{FieldMeta, FieldType, IncurSchema},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::OnceLock;

const SOURCE_ID: &str = "fixture-source";

/// Search text that makes this fixture hang until it is cancelled.
const HANG_QUERY: &str = "hang-until-cancelled";

#[derive(Debug, Deserialize, incurs::Args)]
struct SearchArgs {
    text: String,
}

#[derive(Debug, Deserialize, incurs::Options)]
struct SearchOptions {
    limit: Option<u32>,
    since: Option<String>,
    until: Option<String>,
}

#[derive(Debug)]
struct TargetOptions {
    target_type: String,
    source: Option<String>,
    url: Option<String>,
    id: Option<String>,
    record: Option<Record>,
}

impl IncurSchema for TargetOptions {
    fn fields() -> Vec<FieldMeta> {
        vec![
            field("type", FieldType::String, true),
            field("source", FieldType::String, false),
            field("url", FieldType::String, false),
            field("id", FieldType::String, false),
            field("record", FieldType::Value, false),
        ]
    }

    fn from_raw(
        raw: &std::collections::BTreeMap<String, serde_json::Value>,
    ) -> Result<Self, ValidationError> {
        Ok(Self {
            target_type: string_field(raw, "type")?,
            source: optional_string_field(raw, "source")?,
            url: optional_string_field(raw, "url")?,
            id: optional_string_field(raw, "id")?,
            record: optional_record_field(raw, "record")?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Cli::create("comsat-fixture-source")
        .description("Fixture COMSAT source")
        .command("search", search_command())
        .command("fetch", fetch_command())
        .command("follow", follow_command())
        .serve()
        .await?;
    Ok(())
}

fn search_command() -> CommandDef {
    CommandDef::typed::<SearchArgs, SearchOptions, (), Vec<Record>, _, _>(
        "search",
        |ctx: TypedContext<SearchArgs, SearchOptions, ()>| async move {
            let query = Query {
                text: ctx.args.text,
                limit: ctx.options.limit,
                since: ctx.options.since,
                until: ctx.options.until,
            };
            // Never answers, so a caller can prove that cancelling a search
            // tears down this plugin process instead of leaking it.
            if query.text == HANG_QUERY {
                std::future::pending::<()>().await;
            }
            TypedResult::ok(vec![record("search-result", "search", Some(query.text))])
        },
    )
    .description("Search fixture records")
    .done()
}

fn fetch_command() -> CommandDef {
    let mut command = CommandDef::build(
        "fetch",
        TargetCommand {
            output: TargetOutput::Fetch,
        },
    )
    .description("Fetch one fixture record")
    .options::<TargetOptions>()
    .done();
    // A source must advertise what it returns: COMSAT's conformance check reads
    // this schema, and a consumer has no other way to know the result shape.
    command.output_schema = Some(schema_for::<Record>());
    command
}

fn follow_command() -> CommandDef {
    let mut command = CommandDef::build(
        "follow",
        TargetCommand {
            output: TargetOutput::Follow,
        },
    )
    .description("Follow fixture relationships")
    .options::<TargetOptions>()
    .done();
    command.output_schema = Some(schema_for::<Vec<Record>>());
    command
}

fn schema_for<T: schemars::JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema must serialize")
}

enum TargetOutput {
    Fetch,
    Follow,
}

struct TargetCommand {
    output: TargetOutput,
}

#[async_trait::async_trait]
impl CommandHandler for TargetCommand {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let options = match target_options(&ctx.options) {
            Ok(options) => options,
            Err(error) => return validation_result(error),
        };
        let label = target_label(&options);
        match self.output {
            TargetOutput::Fetch => json_result(record("document", "fetch", Some(label))),
            TargetOutput::Follow => json_result(vec![record("comment", "follow", Some(label))]),
        }
    }

    fn mcp_input_schema(&self) -> Option<&Value> {
        Some(target_mcp_schema())
    }
}

fn target_options(value: &Value) -> Result<TargetOptions, ValidationError> {
    let raw: BTreeMap<String, Value> = value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    TargetOptions::from_raw(&raw)
}

fn validation_result(error: ValidationError) -> CommandResult {
    CommandResult::Error {
        code: "VALIDATION_ERROR".to_string(),
        message: error.to_string(),
        retryable: false,
        exit_code: Some(1),
        cta: None,
    }
}

fn json_result(value: impl serde::Serialize) -> CommandResult {
    match serde_json::to_value(value) {
        Ok(data) => CommandResult::Ok {
            data,
            cta: None,
            exit_code: None,
        },
        Err(error) => CommandResult::Error {
            code: "SERIALIZATION_ERROR".to_string(),
            message: error.to_string(),
            retryable: false,
            exit_code: Some(1),
            cta: None,
        },
    }
}

fn target_mcp_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        json!({
            "type": "object",
            "properties": {
                "type": { "type": "string" },
                "source": { "type": "string" },
                "url": { "type": "string" },
                "id": { "type": "string" },
                "record": { "type": "object" }
            },
            "required": ["type"]
        })
    })
}

fn field(name: &'static str, field_type: FieldType, required: bool) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: name.to_string(),
        description: None,
        field_type,
        required,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

fn string_field(
    raw: &std::collections::BTreeMap<String, serde_json::Value>,
    name: &str,
) -> Result<String, ValidationError> {
    optional_string_field(raw, name)?
        .ok_or_else(|| validation_error(name, "required field is missing"))
}

fn optional_string_field(
    raw: &std::collections::BTreeMap<String, serde_json::Value>,
    name: &str,
) -> Result<Option<String>, ValidationError> {
    raw.get(name)
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| validation_error(name, "field must be a string"))
        })
        .transpose()
}

fn optional_record_field(
    raw: &std::collections::BTreeMap<String, serde_json::Value>,
    name: &str,
) -> Result<Option<Record>, ValidationError> {
    raw.get(name)
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|error| validation_error(name, error.to_string()))
        })
        .transpose()
}

fn validation_error(path: &str, message: impl Into<String>) -> ValidationError {
    let message = message.into();
    ValidationError {
        message: message.clone(),
        field_errors: vec![FieldError {
            path: path.to_string(),
            expected: String::new(),
            received: String::new(),
            message,
        }],
        cause: None,
    }
}

fn target_label(options: &TargetOptions) -> String {
    options
        .record
        .as_ref()
        .map(|record| record.id.to_string())
        .or_else(|| options.url.clone())
        .or_else(|| options.id.clone())
        .or_else(|| options.source.clone())
        .unwrap_or_else(|| options.target_type.clone())
}

fn record(kind: &str, suffix: &str, text: Option<String>) -> Record {
    Record {
        id: RecordId::new(format!("fixture:{suffix}")).expect("fixture id is valid"),
        source: SourceId::new(SOURCE_ID).expect("fixture source id is valid"),
        kind: kind.to_string(),
        url: format!("https://example.com/fixture/{suffix}"),
        title: Some(format!("Fixture {kind}")),
        text,
        author: Some("fixture".to_string()),
        created_at: None,
        updated_at: None,
        metadata: json!({
            "native_id": suffix,
            "source": SOURCE_ID
        }),
    }
}
