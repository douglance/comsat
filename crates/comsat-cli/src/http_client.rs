use std::{
    env,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use comsat_source::HttpClient;
use comsat_types::{ErrorClass, SourceError, SourceId};
use futures::StreamExt;

const RESPONSE_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
static SERVE_LOGGING: AtomicBool = AtomicBool::new(false);

pub struct SecureHttpClient;

impl SecureHttpClient {
    pub const fn new() -> Self {
        Self
    }
}

pub fn enable_serve_logging() {
    SERVE_LOGGING.store(true, Ordering::Relaxed);
}

#[async_trait::async_trait]
impl HttpClient for SecureHttpClient {
    async fn send(
        &self,
        request: http::Request<Vec<u8>>,
    ) -> comsat_source::SourceResult<http::Response<Vec<u8>>> {
        let source = source_for_request(&request);
        let started = Instant::now();
        let result = Self::send_request(request).await;
        log_http_request(&source, &result, started.elapsed());
        result
    }
}

impl SecureHttpClient {
    async fn send_request(
        request: http::Request<Vec<u8>>,
    ) -> comsat_source::SourceResult<http::Response<Vec<u8>>> {
        let source = source_for_request(&request);
        let uri = request.uri().clone();
        let url = reqwest::Url::parse(&uri.to_string())
            .map_err(|error| source_error(source.clone(), ErrorClass::InvalidQuery, error))?;
        let addresses = validate_url_target(&source, &url)?;
        let host = url
            .host_str()
            .ok_or_else(|| {
                SourceError::new(
                    source.clone(),
                    ErrorClass::InvalidQuery,
                    "request URL has no host",
                )
            })?
            .to_string();
        let client = pinned_client(&host, &addresses)
            .map_err(|error| source_error(source.clone(), ErrorClass::Internal, error))?;

        let (parts, body) = request.into_parts();
        let response = send_reqwest_request(client, parts, url, body, &source).await?;
        check_response_length(&source, &response)?;
        response_from_reqwest(response, source).await
    }
}

async fn send_reqwest_request(
    client: reqwest::Client,
    parts: http::request::Parts,
    url: reqwest::Url,
    body: Vec<u8>,
    source: &SourceId,
) -> comsat_source::SourceResult<reqwest::Response> {
    let mut builder = client.request(parts.method, url);
    for (name, value) in &parts.headers {
        builder = builder.header(name, value);
    }
    builder
        .body(body)
        .send()
        .await
        .map_err(|error| source_error(source.clone(), ErrorClass::Upstream, error.without_url()))
}

fn check_response_length(
    source: &SourceId,
    response: &reqwest::Response,
) -> comsat_source::SourceResult<()> {
    if response.content_length().unwrap_or_default() > RESPONSE_LIMIT_BYTES as u64 {
        return Err(response_limit_error(source.clone()));
    }
    Ok(())
}

async fn response_from_reqwest(
    response: reqwest::Response,
    source: SourceId,
) -> comsat_source::SourceResult<http::Response<Vec<u8>>> {
    let mut out = http::Response::builder().status(response.status());
    for (name, value) in response.headers() {
        out = out.header(name, value);
    }
    out.body(read_response_body(response, &source).await?)
        .map_err(|error| source_error(source, ErrorClass::Protocol, error))
}

async fn read_response_body(
    response: reqwest::Response,
    source: &SourceId,
) -> comsat_source::SourceResult<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            source_error(source.clone(), ErrorClass::Upstream, error.without_url())
        })?;
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT_BYTES {
            return Err(response_limit_error(source.clone()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn response_limit_error(source: SourceId) -> SourceError {
    SourceError::new(
        source,
        ErrorClass::Protocol,
        "response exceeded 2 MiB limit",
    )
}

fn pinned_client(host: &str, addresses: &[SocketAddr]) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .resolve_to_addrs(host, addresses)
        .build()
}

fn validate_url_target(
    source: &SourceId,
    url: &reqwest::Url,
) -> comsat_source::SourceResult<Vec<SocketAddr>> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SourceError::new(
            source.clone(),
            ErrorClass::InvalidQuery,
            "only HTTP(S) requests are allowed",
        ));
    }
    let host = url.host_str().ok_or_else(|| {
        SourceError::new(
            source.clone(),
            ErrorClass::InvalidQuery,
            "request URL has no host",
        )
    })?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| source_error(source.clone(), ErrorClass::Upstream, error))?;
    let addresses = addresses.collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(SourceError::new(
            source.clone(),
            ErrorClass::Upstream,
            "request host did not resolve",
        ));
    }
    for address in &addresses {
        validate_public_ip(source, address.ip())?;
    }
    Ok(addresses)
}

fn validate_public_ip(source: &SourceId, ip: IpAddr) -> comsat_source::SourceResult<()> {
    let rejected = match ip {
        IpAddr::V4(ip) => rejected_ipv4(ip),
        IpAddr::V6(ip) => rejected_ipv6(ip),
    };
    if rejected {
        return Err(SourceError::new(
            source.clone(),
            ErrorClass::Authorization,
            "request resolved to a private or local address",
        ));
    }
    Ok(())
}

const fn rejected_ipv4(ip: std::net::Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
}

const fn rejected_ipv6(ip: std::net::Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return rejected_ipv4(mapped);
    }
    ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local() || ip.is_unicast_link_local()
}

fn source_for_request(request: &http::Request<Vec<u8>>) -> SourceId {
    request.uri().host().map_or_else(
        || SourceId::new("web").expect("static source id is valid"),
        source_for_host,
    )
}

fn source_for_host(host: &str) -> SourceId {
    let source = match host {
        "api.github.com" => "github",
        "hn.algolia.com" | "hacker-news.firebaseio.com" => "hacker-news",
        "api.stackexchange.com" => "stack-exchange",
        _ => "web",
    };
    SourceId::new(source).expect("static source id is valid")
}

fn source_error(source: SourceId, class: ErrorClass, error: impl std::fmt::Display) -> SourceError {
    SourceError::new(source, class, error.to_string())
}

fn log_http_request(
    source: &SourceId,
    result: &comsat_source::SourceResult<http::Response<Vec<u8>>>,
    elapsed: Duration,
) {
    if env::var("COMSAT_LOG").ok().as_deref() != Some("1") && !SERVE_LOGGING.load(Ordering::Relaxed)
    {
        return;
    }
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "source_http_request",
            "source": source,
            "status": result.as_ref().ok().map(http::Response::status).map(|s| s.as_u16()),
            "error_class": result.as_ref().err().map(|error| error.class),
            "latency_ms": elapsed.as_millis()
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_dns_answers_mapping_private_ipv4_into_ipv6() {
        let source = SourceId::new("web").unwrap();
        for address in ["::ffff:127.0.0.1", "::ffff:10.0.0.1"] {
            assert!(validate_public_ip(&source, address.parse().unwrap()).is_err());
        }
    }
}
