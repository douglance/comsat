#![allow(
    clippy::future_not_send,
    reason = "Cloudflare Workers request streams are non-Send on the isolate event loop."
)]

use futures::StreamExt;
use worker::{Headers, Request, Response, Result};

pub enum BodyReadError {
    TooLarge,
    Worker(worker::Error),
}

impl From<worker::Error> for BodyReadError {
    fn from(error: worker::Error) -> Self {
        Self::Worker(error)
    }
}

pub async fn read_bounded_body(
    request: &mut Request,
    max_bytes: usize,
) -> std::result::Result<String, BodyReadError> {
    if content_length_exceeds(request, max_bytes)? {
        return Err(BodyReadError::TooLarge);
    }
    let mut stream = request.stream().map_err(BodyReadError::Worker)?;
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(BodyReadError::Worker)?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(BodyReadError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|error| BodyReadError::Worker(worker_error(error)))
}

pub fn landing_page() -> Result<Response> {
    let mut response = Response::from_html(crate::landing::HTML)?;
    response
        .headers_mut()
        .set("Cache-Control", "public, max-age=300")?;
    response
        .headers_mut()
        .set("X-Content-Type-Options", "nosniff")?;
    Ok(response)
}

pub fn preflight(origin: Option<&str>) -> Result<Response> {
    let response = Response::empty()?.with_status(204);
    apply_cors(response.headers(), origin)?;
    Ok(response)
}

pub fn with_cors(response: Response, origin: Option<&str>) -> Result<Response> {
    apply_cors(response.headers(), origin)?;
    Ok(response)
}

pub fn json_response(
    status: u16,
    body: &serde_json::Value,
    headers: std::collections::BTreeMap<String, String>,
) -> Result<Response> {
    let mut response = Response::from_json(body)?.with_status(status);
    for (name, value) in headers {
        response.headers_mut().set(&name, &value)?;
    }
    Ok(response)
}

pub fn query_pair(url: &worker::Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(key, value)| (key == name).then(|| value.into_owned()))
}

fn content_length_exceeds(request: &Request, max_bytes: usize) -> Result<bool> {
    Ok(request
        .headers()
        .get("Content-Length")?
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > max_bytes))
}

fn apply_cors(headers: &Headers, origin: Option<&str>) -> Result<()> {
    if let Some(origin) = origin {
        headers.set("Access-Control-Allow-Origin", origin)?;
    }
    headers.set(
        "Access-Control-Allow-Headers",
        "Authorization, Content-Type, Accept, MCP-Protocol-Version",
    )?;
    headers.set("Access-Control-Allow-Methods", "GET, POST, OPTIONS")?;
    apply_vary_origin(headers)
}

fn apply_vary_origin(headers: &Headers) -> Result<()> {
    let Some(current) = headers.get("Vary")? else {
        return headers.set("Vary", "Origin");
    };
    if has_origin_vary(&current) {
        return Ok(());
    }
    headers.set("Vary", &format!("{current}, Origin"))
}

fn has_origin_vary(value: &str) -> bool {
    value
        .split(',')
        .any(|entry| entry.trim().eq_ignore_ascii_case("origin"))
}

fn worker_error(error: impl std::fmt::Display) -> worker::Error {
    worker::Error::RustError(error.to_string())
}
