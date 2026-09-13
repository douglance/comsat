#![allow(
    clippy::future_not_send,
    reason = "Cloudflare Workers run wasm futures on an isolate event loop; worker request streams and bindings are non-Send by design."
)]

use std::{collections::BTreeMap, sync::Arc};

use comsat_engine::SourceCatalog;
use comsat_store::Store;
use incurs::tool::{ToolCallOptions, ToolCallOutcome, ToolCatalog};
use incurs_mcp_cloudflare::{McpHttpOptions, McpHttpRequest, handle_mcp_request};
use serde_json::{Value, json};
use worker::{Env, Request, Response, Result};

pub use crate::runtime_watch::{
    WatchClaimConfig, WatchClaimMessage, enqueue_due_watches_for_tenants, run_claimed_watch,
};
use crate::{
    auth::{TenantTokenMap, origin_allowed, parse_origin_allowlist},
    cloud_config::MAX_HTTP_BODY_BYTES,
    runtime_http::{
        BodyReadError, json_response, preflight, query_pair, read_bounded_body, with_cors,
    },
};

pub async fn route_request<S>(
    mut request: Request,
    env: Env,
    store: Arc<S>,
    source_catalog: Arc<SourceCatalog>,
) -> Result<Response>
where
    S: Store + 'static,
{
    let origins = allowed_origins(&env);
    let origin = request.headers().get("Origin")?;
    if !origin_allowed(origin.as_deref(), &origins) {
        return Response::error("Origin is not allowed", 403);
    }
    if request.method().as_ref() == "OPTIONS" {
        return preflight(origin.as_deref());
    }
    let Some(tenant_id) = authorized_tenant(&request, &env)? else {
        return with_cors(unauthorized()?, origin.as_deref());
    };
    let app_catalog = tenant_catalog(source_catalog, store, tenant_id);
    let response = dispatch_request(&mut request, &app_catalog).await?;
    with_cors(response, origin.as_deref())
}

fn tenant_catalog<S>(
    source_catalog: Arc<SourceCatalog>,
    store: Arc<S>,
    tenant_id: String,
) -> ToolCatalog
where
    S: Store + 'static,
{
    crate::source_graph::app_catalog(source_catalog, store, tenant_id, current_epoch_seconds)
}

async fn dispatch_request(request: &mut Request, app_catalog: &ToolCatalog) -> Result<Response> {
    match request.path().as_str() {
        "/health" => Response::from_json(&json!({"ok": true})),
        "/mcp" => mcp_response(request, app_catalog).await,
        "/history" => history_response(request, app_catalog).await,
        path => dispatch_tool(request, app_catalog, path).await,
    }
}

async fn dispatch_tool(
    request: &mut Request,
    app_catalog: &ToolCatalog,
    path: &str,
) -> Result<Response> {
    let Some(tool) = tool_for_path(path) else {
        return Response::error("Not found", 404);
    };
    tool_response(request, app_catalog, tool).await
}

fn tool_for_path(path: &str) -> Option<&'static str> {
    [
        ("/search", "comsat_search"),
        ("/fetch", "comsat_fetch"),
        ("/follow", "comsat_follow"),
        ("/watch/add", "watch_add"),
        ("/watch/list", "watch_list"),
        ("/watch/delete", "watch_delete"),
    ]
    .into_iter()
    .find_map(|(route, tool)| (path == route).then_some(tool))
}

async fn history_response(request: &Request, app_catalog: &ToolCatalog) -> Result<Response> {
    if !request.method().as_ref().eq_ignore_ascii_case("GET") {
        return Response::error("Method not allowed", 405);
    }
    let url = request.url()?;
    let mut arguments = BTreeMap::new();
    for name in ["watch", "since"] {
        if let Some(value) = query_pair(&url, name) {
            arguments.insert(name.to_string(), Value::String(value));
        }
    }
    if let Some(value) = query_pair(&url, "limit") {
        let Ok(limit) = value.parse::<u32>() else {
            return invalid_arguments("limit must be a nonnegative integer");
        };
        arguments.insert("limit".into(), json!(limit));
    }
    tool_outcome_response(
        app_catalog
            .call("history", arguments, ToolCallOptions::isolated())
            .await,
    )
}

async fn tool_response(
    request: &mut Request,
    app_catalog: &ToolCatalog,
    tool: &str,
) -> Result<Response> {
    if !request.method().as_ref().eq_ignore_ascii_case("POST") {
        return Response::error("Method not allowed", 405);
    }
    let body = match read_bounded_body(request, MAX_HTTP_BODY_BYTES).await {
        Ok(body) => body,
        Err(BodyReadError::TooLarge) => return Response::error("Request body is too large", 413),
        Err(BodyReadError::Worker(error)) => return Err(error),
    };
    let Ok(arguments) = serde_json::from_str::<BTreeMap<String, Value>>(&body) else {
        return invalid_arguments("tool arguments must be a JSON object");
    };
    tool_outcome_response(
        app_catalog
            .call(tool, arguments, ToolCallOptions::isolated())
            .await,
    )
}

fn invalid_arguments(message: &str) -> Result<Response> {
    Response::from_json(&json!({"error": {"code": "invalid_query", "message": message}}))
        .map(|response| response.with_status(400))
}

fn tool_outcome_response(outcome: ToolCallOutcome) -> Result<Response> {
    match outcome {
        ToolCallOutcome::Ok { data, .. } => Response::from_json(&data),
        error @ ToolCallOutcome::Error { .. } => tool_error_response(error),
    }
}

fn tool_error_response(outcome: ToolCallOutcome) -> Result<Response> {
    let ToolCallOutcome::Error {
        code,
        message,
        retryable,
        field_errors,
        cta,
        exit_code,
    } = outcome
    else {
        return Response::error("invalid tool outcome", 500);
    };
    let body = json!({
        "error": {
            "code": code,
            "message": message,
            "retryable": retryable,
            "field_errors": field_errors,
            "cta": cta,
            "exit_code": exit_code,
        }
    });
    let mut response = Response::from_json(&body)?.with_status(400);
    response
        .headers_mut()
        .set("Content-Type", "application/json")?;
    Ok(response)
}

async fn mcp_response(request: &mut Request, app_catalog: &ToolCatalog) -> Result<Response> {
    // route_request has already checked this origin against the configured allowlist.
    // Preserve that decision when Incurs validates the MCP transport boundary.
    let options = McpHttpOptions {
        allowed_origins: request.headers().get("Origin")?.into_iter().collect(),
        ..McpHttpOptions::default()
    };
    let body = if request.method().as_ref().eq_ignore_ascii_case("POST") {
        match read_bounded_body(request, MAX_HTTP_BODY_BYTES).await {
            Ok(body) => Some(body),
            Err(BodyReadError::TooLarge) => {
                return Response::error("MCP request body is too large", 413);
            }
            Err(BodyReadError::Worker(error)) => return Err(error),
        }
    } else {
        None
    };
    let response = handle_mcp_request(
        app_catalog,
        McpHttpRequest {
            method: request.method().as_ref().to_string(),
            path: request.path(),
            headers: request.headers().entries().collect(),
            body,
        },
        &options,
    )
    .await;
    json_response(
        response.status,
        &response.body.unwrap_or_else(|| json!({})),
        response.headers,
    )
}

fn authorized_tenant(request: &Request, env: &Env) -> Result<Option<String>> {
    let map = tenant_token_map(env)?;
    let auth = request.headers().get("Authorization")?;
    match map.resolve(auth.as_deref()) {
        Ok(identity) => Ok(Some(identity.tenant_id)),
        Err(crate::auth::AuthError::MissingBearer | crate::auth::AuthError::UnknownToken) => {
            Ok(None)
        }
        Err(error) => Err(worker_error(error)),
    }
}

fn unauthorized() -> Result<Response> {
    let mut response = Response::error("Unauthorized", 401)?;
    response.headers_mut().set("WWW-Authenticate", "Bearer")?;
    Ok(response)
}

pub fn tenant_ids_from_env(env: &Env) -> Result<Vec<String>> {
    Ok(tenant_token_map(env)?.tenant_ids())
}

fn tenant_token_map(env: &Env) -> Result<TenantTokenMap> {
    if let Ok(secret) = env.secret("TENANT_TOKENS_JSON")
        && let Ok(map) = TenantTokenMap::parse(&secret.to_string())
    {
        return Ok(map);
    }
    let text = env.var("TENANT_TOKENS_JSON")?.to_string();
    TenantTokenMap::parse(&text).map_err(worker_error)
}

fn allowed_origins(env: &Env) -> Vec<String> {
    parse_origin_allowlist(
        env.var("MCP_ALLOWED_ORIGINS")
            .ok()
            .map(|value| value.to_string())
            .as_deref(),
    )
}

fn current_epoch_seconds() -> i64 {
    i64::try_from(worker::Date::now().as_millis() / 1000).unwrap_or(i64::MAX)
}

fn worker_error(error: impl std::fmt::Display) -> worker::Error {
    worker::Error::RustError(error.to_string())
}
