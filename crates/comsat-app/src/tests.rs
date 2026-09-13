#![allow(
    clippy::cast_possible_wrap,
    clippy::future_not_send,
    clippy::significant_drop_tightening,
    clippy::suspicious_operation_groupings
)]

use std::{collections::BTreeMap, sync::Arc};

mod fixtures;

use comsat_store::HistoryRequest;
use incurs::{
    cli::Cli,
    tool::{ToolCallOptions, ToolCallOutcome},
};
use incurs_codemode::{CodeMode, ExecutionStatus, IncurConnector, MemoryStore as CodeMemoryStore};
use incurs_codemode_local::LocalExecutor;
use serde_json::json;

use crate::{ComsatApp, build_cli};
use fixtures::{
    FixtureStore, NoApprovalPolicy, class_failure_catalog, create_fixture_watch, fixture_catalog,
    strict_fixture_catalog,
};

#[tokio::test]
async fn fixture_source_supports_search_fetch_follow_and_watch_history() {
    let store = Arc::new(FixtureStore::default());
    let app = Arc::new(
        ComsatApp::new(fixture_catalog(), "default", Arc::new(|| 100)).with_store(store.clone()),
    );
    let cli = build_cli(Arc::clone(&app));

    assert_fixture_search_cli(&cli).await;
    assert_fixture_fetch_cli(&cli).await;

    create_fixture_watch(&app).await;
    assert_fixture_watch_history(&app).await;
    assert_fixture_history_cli(&cli).await;
}

async fn assert_fixture_search_cli(cli: &Cli) {
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec!["search".into(), "MCP OAuth".into()],
            &mut output,
            false,
        )
        .await
        .unwrap();
    assert_eq!(exit, None, "{}", String::from_utf8_lossy(&output));
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("\"fixture:search\"")
    );
}

async fn assert_fixture_fetch_cli(cli: &Cli) {
    let mut fetch_output = Vec::new();
    let exit = cli
        .serve_to(
            vec![
                "fetch".into(),
                "--type".into(),
                "native".into(),
                "--source".into(),
                "fixture".into(),
                "--id".into(),
                "item-1".into(),
                "--format".into(),
                "json".into(),
            ],
            &mut fetch_output,
            false,
        )
        .await
        .unwrap();
    assert_eq!(exit, None);
    assert!(
        String::from_utf8(fetch_output)
            .unwrap()
            .contains("fixture:fetch:item-1")
    );
}

async fn assert_fixture_watch_history(app: &ComsatApp) {
    let outcome = app.run_due_watch_once().await.unwrap().unwrap();
    assert_eq!(outcome.records_seen, 1);
    let history = app
        .store()
        .unwrap()
        .history(HistoryRequest {
            tenant_id: "default".into(),
            watch_id: Some("fixture-watch".into()),
            since_epoch_seconds: None,
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].record.id.as_str(), "fixture:search");
}

async fn assert_fixture_history_cli(cli: &Cli) {
    let mut output = Vec::new();
    let exit = cli
        .serve_to(vec!["history".into()], &mut output, false)
        .await
        .unwrap();
    assert_eq!(exit, None, "{}", String::from_utf8_lossy(&output));
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("\"source\":\"fixture\""), "{output}");
    assert!(!output.contains("\"record\":"), "{output}");
}

#[tokio::test]
async fn search_rejects_invalid_source_ids() {
    let cli = build_cli(Arc::new(ComsatApp::new(
        fixture_catalog(),
        "default",
        Arc::new(|| 100),
    )));
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec![
                "search".into(),
                "MCP OAuth".into(),
                "--source".into(),
                "Bad_Source".into(),
            ],
            &mut output,
            false,
        )
        .await
        .unwrap();
    assert_eq!(exit, Some(2), "{}", String::from_utf8_lossy(&output));
    assert!(String::from_utf8(output).unwrap().contains("invalid_query"));
}

#[tokio::test]
async fn strict_search_partial_failure_exits_three_with_diagnostic() {
    let cli = build_cli(Arc::new(ComsatApp::new(
        strict_fixture_catalog(),
        "default",
        Arc::new(|| 100),
    )));
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec!["search".into(), "MCP OAuth".into(), "--strict".into()],
            &mut output,
            false,
        )
        .await
        .unwrap();

    assert_eq!(exit, Some(3), "{}", String::from_utf8_lossy(&output));
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("source_error"));
    assert!(output.contains("partial_failure"));
}

#[tokio::test]
async fn nonstrict_total_source_failure_exits_one_with_diagnostic() {
    let cli = build_cli(Arc::new(ComsatApp::new(
        strict_fixture_catalog(),
        "default",
        Arc::new(|| 100),
    )));
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec![
                "search".into(),
                "MCP OAuth".into(),
                "--source".into(),
                "failing".into(),
            ],
            &mut output,
            false,
        )
        .await
        .unwrap();

    assert_eq!(exit, Some(1), "{}", String::from_utf8_lossy(&output));
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("source_error"));
    assert!(output.contains("all selected sources failed"));
}

#[tokio::test]
async fn nonstrict_partial_source_failure_exits_zero_with_diagnostic() {
    let cli = build_cli(Arc::new(ComsatApp::new(
        strict_fixture_catalog(),
        "default",
        Arc::new(|| 100),
    )));
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec!["search".into(), "MCP OAuth".into()],
            &mut output,
            false,
        )
        .await
        .unwrap();

    assert_eq!(exit, None, "{}", String::from_utf8_lossy(&output));
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("fixture:search"));
    assert!(output.contains("source_error"));
}

#[tokio::test]
async fn follow_total_failure_exits_one_with_diagnostic() {
    let cli = build_cli(Arc::new(ComsatApp::new(
        strict_fixture_catalog(),
        "default",
        Arc::new(|| 100),
    )));
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec![
                "follow".into(),
                "--type".into(),
                "native".into(),
                "--source".into(),
                "failing".into(),
                "--id".into(),
                "item-1".into(),
            ],
            &mut output,
            false,
        )
        .await
        .unwrap();

    assert_eq!(exit, Some(1), "{}", String::from_utf8_lossy(&output));
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("source_error"));
    assert!(output.contains("all selected sources failed"));
}

#[tokio::test]
async fn tool_catalog_all_failure_preserves_source_error_classes() {
    let cli = build_cli(Arc::new(ComsatApp::new(
        class_failure_catalog(),
        "default",
        Arc::new(|| 100),
    )));
    let catalog = cli.tool_catalog();

    let rate_limited = catalog
        .call(
            "comsat_search",
            search_arguments(&["rate-limited"]),
            ToolCallOptions::isolated(),
        )
        .await;
    assert_tool_error(
        &rate_limited,
        "rate_limit",
        Some(true),
        &["rate-limited", "rate_limit"],
    );

    let multi_source = catalog
        .call(
            "comsat_search",
            search_arguments(&["rate-limited", "auth-failing"]),
            ToolCallOptions::isolated(),
        )
        .await;
    assert_tool_error(
        &multi_source,
        "source_failure",
        Some(true),
        &[
            "rate-limited",
            "rate_limit",
            "auth-failing",
            "authentication",
        ],
    );
}

#[tokio::test]
async fn tool_catalog_stream_output_schemas_match_aggregated_results() {
    let cli = build_cli(Arc::new(ComsatApp::new(
        fixture_catalog(),
        "default",
        Arc::new(|| 100),
    )));
    let definitions = cli.tool_catalog().definitions();

    assert_schema_type(&definitions, "comsat_search", "array");
    assert_schema_contains(&definitions, "comsat_search", "source_error");
    assert_schema_type(&definitions, "comsat_fetch", "object");
    assert_schema_record_property_is_object(&definitions, "comsat_fetch");
    assert_schema_type(&definitions, "comsat_follow", "array");
    assert_schema_record_property_is_object(&definitions, "comsat_follow");
    assert_schema_contains(&definitions, "comsat_follow", "source_error");
    assert_schema_type(&definitions, "history", "array");
    assert!(!schema_text(&definitions, "history").contains("source_error"));
}

#[tokio::test]
async fn codemode_uses_the_same_incur_graph_for_search_fetch_follow() {
    let cli = build_cli(Arc::new(ComsatApp::new(
        fixture_catalog(),
        "default",
        Arc::new(|| 100),
    )));
    let codemode = CodeMode::new(
        Arc::new(CodeMemoryStore::default()),
        LocalExecutor::default(),
        vec![Arc::new(
            IncurConnector::new(cli.tool_catalog())
                .with_policy_resolver(Arc::new(NoApprovalPolicy)),
        )],
    );

    let execution = codemode
        .execute(
            "const found = await comsat.comsat_search({ text: 'MCP OAuth' }); \
             const fetched = await comsat.comsat_fetch({ type: 'record', record: found[0] }); \
             const followed = await comsat.comsat_follow({ type: 'record', record: found[0] }); \
             return { found: found[0].id, fetched: fetched.id, followed: followed[0].id };",
        )
        .await
        .unwrap();

    assert_eq!(
        execution.status,
        ExecutionStatus::Completed,
        "{execution:?}"
    );
    assert_eq!(
        execution.result,
        Some(json!({
            "found": "fixture:search",
            "fetched": "fixture:fetch:record",
            "followed": "fixture:follow"
        }))
    );
}

fn search_arguments(sources: &[&str]) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("text".to_string(), json!("MCP OAuth")),
        ("source".to_string(), json!(sources)),
    ])
}

fn assert_tool_error(
    outcome: &ToolCallOutcome,
    expected_code: &str,
    expected_retryable: Option<bool>,
    message_needles: &[&str],
) {
    let ToolCallOutcome::Error {
        code,
        message,
        retryable,
        exit_code,
        ..
    } = outcome
    else {
        panic!("expected tool error: {outcome:?}");
    };
    assert_eq!(code, expected_code, "{outcome:?}");
    assert_eq!(*retryable, expected_retryable, "{outcome:?}");
    assert_eq!(*exit_code, Some(1), "{outcome:?}");
    assert!(message.contains("all selected sources failed"), "{message}");
    for needle in message_needles {
        assert!(message.contains(needle), "{message}");
    }
}

fn assert_schema_type(
    definitions: &[incurs::tool::ToolDefinition],
    name: &str,
    expected_type: &str,
) {
    let schema = schema(definitions, name);
    assert_eq!(schema["type"], expected_type, "{schema}");
}

fn assert_schema_contains(definitions: &[incurs::tool::ToolDefinition], name: &str, needle: &str) {
    let text = schema_text(definitions, name);
    assert!(text.contains(needle), "{text}");
}

fn assert_schema_record_property_is_object(
    definitions: &[incurs::tool::ToolDefinition],
    name: &str,
) {
    let schema = input_schema(definitions, name);
    assert!(schema.to_string().contains("\"record\""), "{schema}");
    assert!(schema.to_string().contains("\"object\""), "{schema}");
}

fn schema_text(definitions: &[incurs::tool::ToolDefinition], name: &str) -> String {
    schema(definitions, name).to_string()
}

fn schema<'a>(
    definitions: &'a [incurs::tool::ToolDefinition],
    name: &str,
) -> &'a serde_json::Value {
    definitions
        .iter()
        .find(|definition| definition.name == name)
        .and_then(|definition| definition.output_schema.as_ref())
        .unwrap_or_else(|| panic!("missing output schema for {name}"))
}

fn input_schema<'a>(
    definitions: &'a [incurs::tool::ToolDefinition],
    name: &str,
) -> &'a serde_json::Value {
    &definitions
        .iter()
        .find(|definition| definition.name == name)
        .unwrap_or_else(|| panic!("missing tool definition for {name}"))
        .input_schema
}
