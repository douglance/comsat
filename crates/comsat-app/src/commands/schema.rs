use comsat_types::{Record, Target};
use incurs::{
    command::{McpAnnotations, McpCommandOptions},
    schema::{FieldMeta, FieldType},
};
use schemars::JsonSchema;
use serde_json::{Value, json};

pub(super) fn read_only_mcp(name: &'static str, description: &'static str) -> McpCommandOptions {
    McpCommandOptions {
        name: Some(name.to_string()),
        description: Some(description.to_string()),
        annotations: Some(McpAnnotations {
            title: Some(description.to_string()),
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: Some(true),
            open_world_hint: Some(true),
        }),
        ..McpCommandOptions::default()
    }
}

pub(super) fn record_schema() -> Value {
    schema_for::<Record>()
}

pub(super) fn target_input_schema() -> Value {
    schema_for::<Target>()
}

pub(super) fn record_array_schema() -> Value {
    schema_for::<Vec<Record>>()
}

pub(super) fn record_or_source_error_array_schema() -> Value {
    let mut schema = record_array_schema();
    let record_item = std::mem::take(&mut schema["items"]);
    schema["items"] = json!({"anyOf": [record_item, source_error_schema()]});
    schema
}

fn source_error_schema() -> Value {
    json!({
        "type": "object",
        "required": ["type", "diagnostic"],
        "properties": {
            "type": {
                "const": "source_error"
            },
            "diagnostic": {
                "type": "object",
                "required": ["source", "class", "message"],
                "properties": {
                    "source": { "type": "string" },
                    "class": {
                        "type": "string",
                        "enum": [
                            "authentication",
                            "authorization",
                            "rate_limit",
                            "invalid_query",
                            "not_found",
                            "unsupported",
                            "upstream",
                            "timeout",
                            "cancelled",
                            "protocol",
                            "internal"
                        ]
                    },
                    "message": { "type": "string" },
                    "retry_after_seconds": {
                        "type": ["integer", "null"],
                        "minimum": 0
                    }
                }
            }
        }
    })
}

pub(super) fn search_args_fields() -> Vec<FieldMeta> {
    vec![field("text", "text", FieldType::String, false)]
}

pub(super) fn search_fields() -> Vec<FieldMeta> {
    vec![
        field("text", "text", FieldType::String, false),
        field("limit", "limit", FieldType::Number, false),
        field("since", "since", FieldType::String, false),
        field("until", "until", FieldType::String, false),
        field(
            "source",
            "source",
            FieldType::Array(Box::new(FieldType::String)),
            false,
        ),
        field("strict", "strict", FieldType::Boolean, false),
    ]
}

pub(super) fn target_fields() -> Vec<FieldMeta> {
    vec![
        field("type", "type", FieldType::String, false),
        field("source", "source", FieldType::String, false),
        field("url", "url", FieldType::String, false),
        field("id", "id", FieldType::String, false),
        field("record", "record", FieldType::Value, false),
        field("strict", "strict", FieldType::Boolean, false),
    ]
}

fn schema_for<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema must serialize")
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
    use super::*;

    #[test]
    fn generated_output_schemas_keep_references_in_their_document() {
        for schema in [
            record_schema(),
            record_array_schema(),
            record_or_source_error_array_schema(),
        ] {
            assert_schema_locations(&schema, &schema, true);
        }
    }

    fn assert_schema_locations(root: &Value, value: &Value, is_root: bool) {
        match value {
            Value::Object(object) => {
                assert!(
                    is_root || !object.contains_key("$schema") || object.contains_key("$id"),
                    "nested schema dialect without a resource ID: {value}"
                );
                if let Some(reference) = object
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|reference| reference.strip_prefix('#'))
                {
                    assert!(
                        root.pointer(reference).is_some(),
                        "unresolved schema reference #{reference}"
                    );
                }
                for child in object.values() {
                    assert_schema_locations(root, child, false);
                }
            }
            Value::Array(items) => {
                for item in items {
                    assert_schema_locations(root, item, false);
                }
            }
            _ => {}
        }
    }
}
