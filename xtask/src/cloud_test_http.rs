use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use serde_json::Value;

use crate::Result;

pub struct HttpClient {
    port: u16,
}

impl HttpClient {
    pub const fn new(port: u16) -> Self {
        Self { port }
    }

    pub fn get(&self, path: &str, headers: &[(String, String)]) -> Result<HttpResponse> {
        self.request("GET", path, headers, "")
    }

    pub fn post_raw(
        &self,
        path: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<HttpResponse> {
        self.request("POST", path, headers, body)
    }

    pub fn post_json(
        &self,
        path: &str,
        headers: &[(String, String)],
        body: &Value,
    ) -> Result<HttpResponse> {
        let mut json_headers = headers.to_vec();
        json_headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        self.request("POST", path, &json_headers, &body.to_string())
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<HttpResponse> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port))?;
        write_request(&mut stream, method, path, headers, body)?;
        read_response(stream)
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

fn write_request(
    stream: &mut TcpStream,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<()> {
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n"
    )?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(stream, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    stream.flush()?;
    Ok(())
}

fn read_response(mut stream: TcpStream) -> Result<HttpResponse> {
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or("HTTP response missing header separator")?;
    let mut lines = head.lines();
    let status = parse_status(lines.next().ok_or("HTTP response missing status line")?)?;
    let headers = parse_headers(lines);
    let body = decode_body(body, &headers)?;
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn parse_status(line: &str) -> Result<u16> {
    let status = line
        .split_whitespace()
        .nth(1)
        .ok_or("HTTP status line missing code")?;
    Ok(status.parse()?)
}

fn parse_headers<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeMap<String, String> {
    lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect()
}

fn decode_body(body: &str, headers: &BTreeMap<String, String>) -> Result<String> {
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        decode_chunked(body)
    } else {
        Ok(body.to_owned())
    }
}

fn decode_chunked(body: &str) -> Result<String> {
    let mut rest = body;
    let mut decoded = String::new();
    loop {
        let (size_line, after_size) = rest.split_once("\r\n").ok_or("invalid chunked body")?;
        let size = usize::from_str_radix(size_line.trim(), 16)?;
        if size == 0 {
            return Ok(decoded);
        }
        if after_size.len() < size + 2 {
            return Err("invalid chunked body length".into());
        }
        decoded.push_str(&after_size[..size]);
        rest = &after_size[size + 2..];
    }
}
