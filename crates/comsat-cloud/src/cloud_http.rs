#![allow(
    clippy::future_not_send,
    reason = "Cloudflare Workers run fetch and response streams on the isolate event loop; worker bindings are non-Send by design."
)]

use async_trait::async_trait;
use comsat_source::{HttpClient, SourceResult};
use comsat_types::{ErrorClass, SourceError, SourceId};
use futures::StreamExt;
use http::{Request as HttpRequest, Response as HttpResponse};
use worker::{
    AbortController, AbortSignal, Fetch, Headers, Method, Request, RequestInit, RequestRedirect,
    Response, ResponseBody,
    send::{IntoSendFuture, SendWrapper},
};

const MAX_SOURCE_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const FETCH_TIMEOUT_MS: u32 = 15_000;

#[derive(Debug, Clone)]
pub struct CloudflareHttpClient {
    source_id: SourceId,
}

impl CloudflareHttpClient {
    pub const fn new(source_id: SourceId) -> Self {
        Self { source_id }
    }
}

#[async_trait]
impl HttpClient for CloudflareHttpClient {
    async fn send(&self, request: HttpRequest<Vec<u8>>) -> SourceResult<HttpResponse<Vec<u8>>> {
        let started = worker::Date::now().as_millis();
        let result = self.send_request(request).await;
        worker::console_log!(
            "{}",
            serde_json::json!({
                "event": "source_http_request",
                "source": self.source_id,
                "status": result.as_ref().ok().map(http::Response::status).map(|s| s.as_u16()),
                "error_class": result.as_ref().err().map(|error| error.class),
                "latency_ms": worker::Date::now().as_millis().saturating_sub(started)
            })
        );
        result
    }
}

impl CloudflareHttpClient {
    async fn send_request(
        &self,
        request: HttpRequest<Vec<u8>>,
    ) -> SourceResult<HttpResponse<Vec<u8>>> {
        let guard = WorkerAbortGuard::new();
        let signal = guard.signal();
        let mut response = self.fetch(request, &signal).await?;
        let status = response.status_code();
        let headers = response.headers().entries().collect::<Vec<_>>();
        let body = read_limited_body(&mut response, &self.source_id).await?;
        guard.disarm();
        response_from_parts(status, headers, body, &self.source_id)
    }
}

impl CloudflareHttpClient {
    async fn fetch(
        &self,
        request: HttpRequest<Vec<u8>>,
        signal: &AbortSignal,
    ) -> SourceResult<Response> {
        Fetch::Request(worker_request(request, &self.source_id)?)
            .send_with_signal(signal)
            .into_send()
            .await
            .map_err(|_| source_error(&self.source_id, ErrorClass::Timeout, "source fetch failed"))
    }
}

struct WorkerAbortGuard {
    controller: Option<AbortController>,
}

impl WorkerAbortGuard {
    fn new() -> Self {
        Self {
            controller: Some(AbortController::default()),
        }
    }

    fn signal(&self) -> AbortSignal {
        combined_abort_signal(&self.controller_signal(), &timeout_signal())
    }

    fn disarm(mut self) {
        self.controller = None;
    }

    fn controller_signal(&self) -> AbortSignal {
        self.controller
            .as_ref()
            .expect("abort controller exists while guard is armed")
            .signal()
    }
}

impl Drop for WorkerAbortGuard {
    fn drop(&mut self) {
        if let Some(controller) = self.controller.take() {
            controller.abort();
        }
    }
}

fn timeout_signal() -> AbortSignal {
    AbortSignal::from(worker::web_sys::AbortSignal::timeout_with_u32(
        FETCH_TIMEOUT_MS,
    ))
}

fn combined_abort_signal(controller: &AbortSignal, timeout: &AbortSignal) -> AbortSignal {
    let signals = worker::js_sys::Array::new();
    signals.push(controller.as_ref());
    signals.push(timeout.as_ref());
    AbortSignal::from(worker::web_sys::AbortSignal::any(&signals.into()))
}

async fn read_limited_body(response: &mut Response, source_id: &SourceId) -> SourceResult<Vec<u8>> {
    match response.body() {
        ResponseBody::Empty => Ok(Vec::new()),
        ResponseBody::Body(bytes) => checked_bytes(bytes.clone(), source_id),
        ResponseBody::Stream(_) => read_limited_stream(response, source_id).await,
    }
}

async fn read_limited_stream(
    response: &mut Response,
    source_id: &SourceId,
) -> SourceResult<Vec<u8>> {
    let stream = response.stream().map_err(|_| {
        source_error(
            source_id,
            ErrorClass::Protocol,
            "source response body stream failed",
        )
    })?;
    let mut stream = SendWrapper::new(stream);
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().into_send().await {
        let chunk = chunk.map_err(|_| {
            source_error(
                source_id,
                ErrorClass::Protocol,
                "source response body stream failed",
            )
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_SOURCE_RESPONSE_BYTES {
            return Err(body_too_large(source_id));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn checked_bytes(bytes: Vec<u8>, source_id: &SourceId) -> SourceResult<Vec<u8>> {
    if bytes.len() > MAX_SOURCE_RESPONSE_BYTES {
        return Err(body_too_large(source_id));
    }
    Ok(bytes)
}

fn response_from_parts(
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    source_id: &SourceId,
) -> SourceResult<HttpResponse<Vec<u8>>> {
    let mut builder = HttpResponse::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder.body(body).map_err(|_| {
        source_error(
            source_id,
            ErrorClass::Protocol,
            "source response conversion failed",
        )
    })
}

fn worker_request(request: HttpRequest<Vec<u8>>, source_id: &SourceId) -> SourceResult<Request> {
    let (parts, body) = request.into_parts();
    let mut init = RequestInit::new();
    init.with_method(method(parts.method.as_str(), source_id)?);
    init.with_redirect(RequestRedirect::Manual);
    if !body.is_empty() {
        let array = worker::js_sys::Uint8Array::from(body.as_slice());
        init.with_body(Some(array.into()));
    }
    init.with_headers(worker_headers(&parts.headers, source_id)?);
    Request::new_with_init(&parts.uri.to_string(), &init).map_err(|_| {
        source_error(
            source_id,
            ErrorClass::Protocol,
            "source request conversion failed",
        )
    })
}

fn worker_headers(headers: &http::HeaderMap, source_id: &SourceId) -> SourceResult<Headers> {
    let worker_headers = Headers::new();
    for (name, value) in headers {
        let value = value.to_str().map_err(|_| {
            source_error(
                source_id,
                ErrorClass::Protocol,
                "source request header is invalid",
            )
        })?;
        worker_headers.set(name.as_str(), value).map_err(|_| {
            source_error(
                source_id,
                ErrorClass::Protocol,
                "source request header failed",
            )
        })?;
    }
    Ok(worker_headers)
}

fn method(value: &str, source_id: &SourceId) -> SourceResult<Method> {
    known_method(value).ok_or_else(|| {
        source_error(
            source_id,
            ErrorClass::Protocol,
            "unsupported HTTP method for Worker fetch",
        )
    })
}

fn known_method(value: &str) -> Option<Method> {
    [
        ("GET", Method::Get),
        ("POST", Method::Post),
        ("PUT", Method::Put),
        ("PATCH", Method::Patch),
        ("DELETE", Method::Delete),
        ("HEAD", Method::Head),
        ("OPTIONS", Method::Options),
    ]
    .into_iter()
    .find_map(|(name, method)| (value == name).then_some(method))
}

fn body_too_large(source_id: &SourceId) -> SourceError {
    source_error(
        source_id,
        ErrorClass::Protocol,
        "source response body exceeds Cloudflare adapter limit",
    )
}

fn source_error(source_id: &SourceId, class: ErrorClass, message: &'static str) -> SourceError {
    SourceError::new(source_id.clone(), class, message)
}
