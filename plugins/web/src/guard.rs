use std::net::{Ipv4Addr, Ipv6Addr};

use comsat_types::{ErrorClass, SourceError, SourceId, Target};
use http::Request;
use url::{Host, Url};

pub fn target_url(source: SourceId, target: &Target) -> Result<Url, SourceError> {
    match target {
        Target::Record { record } => {
            ensure_source(source.clone(), &record.source)?;
            Url::parse(&record.url).map_err(|error| protocol(source, error))
        }
        Target::Url {
            source: target_source,
            url,
        } => {
            ensure_source(source.clone(), target_source)?;
            Url::parse(url).map_err(|error| protocol(source, error))
        }
        Target::Native {
            source: target_source,
            id,
        } => {
            ensure_source(source.clone(), target_source)?;
            Url::parse(id)
                .map_err(|_| invalid_query(source, "web native id must be an absolute URL"))
        }
    }
}

pub fn validate_fetch_url(source: SourceId, url: &Url) -> Result<(), SourceError> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid_query(
            source,
            "web fetch URL must not contain credentials",
        ));
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SourceError::new(
            source,
            ErrorClass::Unsupported,
            "web fetch only supports http and https",
        ));
    }
    match url
        .host()
        .ok_or_else(|| invalid_query(source.clone(), "web fetch URL has no host"))?
    {
        Host::Domain(host) => validate_public_domain(source, host)?,
        Host::Ipv4(ip) => validate_public_ipv4(source, ip)?,
        Host::Ipv6(ip) => validate_public_ipv6(source, ip)?,
    }
    Ok(())
}

pub fn public_get(source: SourceId, url: &Url) -> Result<Request<Vec<u8>>, SourceError> {
    Request::builder()
        .method("GET")
        .uri(url.as_str())
        .header(
            "accept",
            "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.1",
        )
        .header("user-agent", "comsat")
        .body(Vec::new())
        .map_err(|error| protocol(source, error))
}

fn validate_public_ipv4(source: SourceId, ip: Ipv4Addr) -> Result<(), SourceError> {
    reject_private_ip(source, is_rejected_ipv4(ip))
}

fn validate_public_ipv6(source: SourceId, ip: Ipv6Addr) -> Result<(), SourceError> {
    reject_private_ip(source, is_rejected_ipv6(ip))
}

const fn is_rejected_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
}

const fn is_rejected_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_rejected_ipv4(mapped);
    }
    ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local() || ip.is_unicast_link_local()
}

fn reject_private_ip(source: SourceId, rejected: bool) -> Result<(), SourceError> {
    if !rejected {
        return Ok(());
    }
    Err(SourceError::new(
        source,
        ErrorClass::Authorization,
        "web fetch rejects non-public IP targets",
    ))
}

fn validate_public_domain(source: SourceId, host: &str) -> Result<(), SourceError> {
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(SourceError::new(
            source,
            ErrorClass::Authorization,
            "web fetch rejects local targets",
        ));
    }
    Ok(())
}

fn ensure_source(source: SourceId, target_source: &SourceId) -> Result<(), SourceError> {
    if target_source == &source {
        return Ok(());
    }
    Err(invalid_query(source, "target source does not match web"))
}

fn invalid_query(source: SourceId, message: impl Into<String>) -> SourceError {
    SourceError::new(source, ErrorClass::InvalidQuery, message)
}

fn protocol(source: SourceId, message: impl std::fmt::Display) -> SourceError {
    SourceError::new(source, ErrorClass::Protocol, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceId {
        SourceId::new("web").unwrap()
    }

    #[test]
    fn rejects_ipv6_loopback_literal() {
        let url = Url::parse("http://[::1]/admin").unwrap();
        assert!(validate_fetch_url(source(), &url).is_err());
    }

    #[test]
    fn rejects_ipv4_mapped_private_targets() {
        for target in ["http://[::ffff:127.0.0.1]/", "http://[::ffff:10.0.0.1]/"] {
            let url = Url::parse(target).unwrap();
            assert!(validate_fetch_url(source(), &url).is_err());
        }
    }

    #[test]
    fn rejects_localhost_domain() {
        let url = Url::parse("http://service.localhost/admin").unwrap();
        assert!(validate_fetch_url(source(), &url).is_err());
    }

    #[test]
    fn rejects_credentials_embedded_in_target_url() {
        let url = Url::parse("https://user:password@example.com/").unwrap();
        assert!(validate_fetch_url(source(), &url).is_err());
    }

    #[test]
    fn rejects_mismatched_target_source() {
        let target = Target::Native {
            source: SourceId::new("github").unwrap(),
            id: "https://example.com".into(),
        };
        assert!(target_url(source(), &target).is_err());
    }
}
