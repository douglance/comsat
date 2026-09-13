use std::path::Path;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::Result;
use crate::cloud_test_http::{HttpClient, HttpResponse};
use crate::cloud_test_project::{
    ALLOWED_ORIGIN, CloudTestProject, init_d1, query_d1, start_worker,
};
use crate::cloud_test_support::{TempDir, free_port, now_nanos};

const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn run(root: &Path) -> Result<()> {
    let temp = TempDir::new("comsat-cloud-test")?;
    let tokens = TestTokens::new();
    let project = CloudTestProject::write(root, temp.path(), &tokens)?;
    project.ensure_worker_build()?;
    init_d1(root, &project)?;

    let port = free_port()?;
    let worker = start_worker(root, &project, port)?;
    let client = HttpClient::new(port);

    assert_auth_and_origin(&client, &tokens.tenant_a)?;
    assert_mcp_tools(&client, &tokens.tenant_a)?;
    assert_history_and_input_contract(&client, &tokens.tenant_a)?;
    assert_watch_tenant_isolation(&project, &client, &tokens)?;
    assert_scheduled_failure_bookkeeping(root, &project, &client, &tokens.tenant_a)?;
    assert_body_contains(&project.log_tail(), "storage_snapshot")?;
    assert_body_lacks(&project.log_tail(), "\"event\":\"d1_failure\"")?;

    drop(worker);
    Ok(())
}

fn assert_history_and_input_contract(client: &HttpClient, token: &str) -> Result<()> {
    let headers = auth_origin(token, ALLOWED_ORIGIN);
    let history = client.get("/history?since=30d&limit=10", &headers)?;
    assert_status("relative history filter", &history, 200)?;
    if serde_json::from_str::<Value>(&history.body)? != json!([]) {
        return Err("new tenant history must be an empty canonical record array".into());
    }
    for path in ["/history?since=invalid", "/history?limit=invalid"] {
        assert_status(
            "invalid history arguments",
            &client.get(path, &headers)?,
            400,
        )?;
    }
    let malformed = client.post_raw("/search", &headers, "{")?;
    assert_status("malformed search JSON", &malformed, 400)?;
    assert_body_contains(&malformed.body, "invalid_query")
}

fn assert_auth_and_origin(client: &HttpClient, token: &str) -> Result<()> {
    let unauthenticated = client.get("/health", &[])?;
    assert_status("unauthenticated health", &unauthenticated, 401)?;

    let bad_origin = client.get("/health", &auth_origin(token, "http://evil.invalid"))?;
    assert_status("disallowed origin", &bad_origin, 403)?;

    let allowed = client.get("/health", &auth_origin(token, ALLOWED_ORIGIN))?;
    assert_status("authenticated health", &allowed, 200)?;
    assert_json_bool(&allowed.body, &["ok"], true)
}

fn assert_mcp_tools(client: &HttpClient, token: &str) -> Result<()> {
    let init = client.post_json("/mcp", &mcp_headers(token, None), &mcp_initialize())?;
    assert_success("mcp initialize", &init)?;
    let session = init.headers.get("mcp-session-id").cloned();

    let tools = client.post_json(
        "/mcp",
        &mcp_headers(token, session.as_deref()),
        &mcp_tools_list(),
    )?;
    assert_success("mcp tools/list", &tools)?;
    assert_body_contains(&tools.body, "comsat_search")?;
    assert_any_body_contains(
        &tools.body,
        &["comsat_watch_create", "comsat_watch_add", "watch_add"],
    )
}

fn assert_watch_tenant_isolation(
    project: &CloudTestProject,
    client: &HttpClient,
    tokens: &TestTokens,
) -> Result<()> {
    create_invalid_watch(project, client, &tokens.tenant_a, "tenant-a-invalid-watch")?;
    create_invalid_watch(project, client, &tokens.tenant_b, "tenant-b-invalid-watch")?;

    let a_list = post_tool(client, "/watch/list", &tokens.tenant_a, &json!({}))?;
    assert_body_contains(&a_list.body, "tenant-a-invalid-watch")?;
    assert_body_lacks(&a_list.body, "tenant-b-invalid-watch")?;

    let b_list = post_tool(client, "/watch/list", &tokens.tenant_b, &json!({}))?;
    assert_body_contains(&b_list.body, "tenant-b-invalid-watch")?;
    assert_body_lacks(&b_list.body, "tenant-a-invalid-watch")?;

    let history = client.get(
        "/history?watch=tenant-a-invalid-watch&limit=10",
        &auth_origin(&tokens.tenant_b, ALLOWED_ORIGIN),
    )?;
    assert_success("cross-tenant history request", &history)?;
    assert_body_lacks(&history.body, "tenant-a-invalid-watch")
}

fn assert_scheduled_failure_bookkeeping(
    root: &Path,
    project: &CloudTestProject,
    client: &HttpClient,
    token: &str,
) -> Result<()> {
    create_invalid_watch(project, client, token, "scheduled-invalid-watch")?;
    let scheduled = client.post_raw("/__scheduled?cron=*+*+*+*+*", &[], "")?;
    assert_scheduled_status(&scheduled)?;
    poll_watch_run_error(root, project, "tenant-a", "scheduled-invalid-watch")
}

fn create_invalid_watch(
    project: &CloudTestProject,
    client: &HttpClient,
    token: &str,
    watch_id: &str,
) -> Result<()> {
    let body = json!({
        "watch_id": watch_id,
        "query": "local fixture invalid source",
        "interval_seconds": 1,
        "source": ["invalid-source"],
        "limit": 1
    });
    let response = post_tool(client, "/watch/add", token, &body)?;
    assert_success("watch add", &response).map_err(|error| {
        format!(
            "{error}. Worker log tail:
{}",
            project.log_tail()
        )
        .into()
    })
}

fn poll_watch_run_error(
    root: &Path,
    project: &CloudTestProject,
    tenant_id: &str,
    watch_id: &str,
) -> Result<()> {
    let query = "SELECT tenant_id, watch_id, status, error FROM watch_runs ORDER BY started_at_epoch_seconds";
    let mut last_output = Value::Null;
    for _ in 0..20 {
        let output = query_d1(root, project, query)?;
        if json_contains_watch_error(&output, tenant_id, watch_id) {
            return Ok(());
        }
        last_output = output;
        thread::sleep(Duration::from_millis(500));
    }
    let watches = query_d1(
        root,
        project,
        "SELECT tenant_id, watch_id, next_due_epoch_seconds, lease_owner, lease_expires_epoch_seconds FROM watches ORDER BY watch_id",
    )?;
    Err(format!(
        "scheduled queue did not record a failed run for watch `{watch_id}`. Last watch_runs query result: {last_output}. Watches query result: {watches}. Worker log tail:
{}",
        project.log_tail()
    )
    .into())
}

fn json_contains_watch_error(value: &Value, tenant_id: &str, watch_id: &str) -> bool {
    match value {
        Value::Object(object) => object_matches_run(object, tenant_id, watch_id),
        Value::Array(items) => items
            .iter()
            .any(|item| json_contains_watch_error(item, tenant_id, watch_id)),
        _ => false,
    }
}

fn object_matches_run(
    object: &serde_json::Map<String, Value>,
    tenant_id: &str,
    watch_id: &str,
) -> bool {
    let fields_match = object.get("tenant_id").and_then(Value::as_str) == Some(tenant_id)
        && object.get("watch_id").and_then(Value::as_str) == Some(watch_id);
    let error_recorded = object
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|error| !error.is_empty());
    fields_match && error_recorded
        || object
            .values()
            .any(|value| json_contains_watch_error(value, tenant_id, watch_id))
}

fn post_tool(client: &HttpClient, path: &str, token: &str, body: &Value) -> Result<HttpResponse> {
    client.post_json(path, &auth_origin(token, ALLOWED_ORIGIN), body)
}

fn assert_scheduled_status(response: &HttpResponse) -> Result<()> {
    match response.status {
        200 | 202 | 204 => Ok(()),
        status => Err(format!(
            "scheduled trigger returned HTTP {status}: {}",
            response.body
        )
        .into()),
    }
}

fn mcp_initialize() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "comsat-cloud-test", "version": "0.1.0"}
        }
    })
}

fn mcp_tools_list() -> Value {
    json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
}

fn mcp_headers(token: &str, session: Option<&str>) -> Vec<(String, String)> {
    let mut headers = auth_origin(token, ALLOWED_ORIGIN);
    headers.push((
        "Accept".to_owned(),
        "application/json, text/event-stream".to_owned(),
    ));
    headers.push((
        "MCP-Protocol-Version".to_owned(),
        PROTOCOL_VERSION.to_owned(),
    ));
    if let Some(session) = session {
        headers.push(("Mcp-Session-Id".to_owned(), session.to_owned()));
    }
    headers
}

fn auth_origin(token: &str, origin: &str) -> Vec<(String, String)> {
    vec![
        ("Authorization".to_owned(), format!("Bearer {token}")),
        ("Origin".to_owned(), origin.to_owned()),
    ]
}

fn assert_success(label: &str, response: &HttpResponse) -> Result<()> {
    if (200..300).contains(&response.status) {
        Ok(())
    } else {
        Err(format!(
            "{label} returned HTTP {}: {}",
            response.status, response.body
        )
        .into())
    }
}

fn assert_status(label: &str, response: &HttpResponse, expected: u16) -> Result<()> {
    if response.status == expected {
        Ok(())
    } else {
        Err(format!(
            "{label} returned HTTP {}, expected {expected}",
            response.status
        )
        .into())
    }
}

fn assert_json_bool(body: &str, path: &[&str], expected: bool) -> Result<()> {
    let value: Value = serde_json::from_str(body)?;
    let actual = path
        .iter()
        .try_fold(&value, |current, key| current.get(*key));
    if actual.and_then(Value::as_bool) == Some(expected) {
        Ok(())
    } else {
        Err(format!("JSON body did not contain {path:?}={expected}").into())
    }
}

fn assert_body_contains(body: &str, needle: &str) -> Result<()> {
    if body.contains(needle) {
        Ok(())
    } else {
        Err(format!("response body did not contain `{needle}`").into())
    }
}

fn assert_any_body_contains(body: &str, needles: &[&str]) -> Result<()> {
    if needles.iter().any(|needle| body.contains(needle)) {
        Ok(())
    } else {
        Err(format!("response body did not contain any of {needles:?}. Body: {body}").into())
    }
}

fn assert_body_lacks(body: &str, needle: &str) -> Result<()> {
    if body.contains(needle) {
        Err(format!("response body unexpectedly contained `{needle}`").into())
    } else {
        Ok(())
    }
}

pub struct TestTokens {
    pub tenant_a: String,
    pub tenant_b: String,
}

impl TestTokens {
    fn new() -> Self {
        Self {
            tenant_a: token("tenant-a"),
            tenant_b: token("tenant-b"),
        }
    }
}

fn token(label: &str) -> String {
    let nanos = now_nanos();
    let mut token = format!("{label}-{}-{nanos}", std::process::id());
    while token.len() < 40 {
        token.push('x');
    }
    token
}
