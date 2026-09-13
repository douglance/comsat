use std::collections::BTreeMap;

use serde::Deserialize;

const MIN_BEARER_TOKEN_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantIdentity {
    pub tenant_id: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("authorization header must use a bearer token")]
    MissingBearer,
    #[error("bearer token is not authorized")]
    UnknownToken,
    #[error("tenant token map must be a JSON object of token to tenant id")]
    InvalidTokenMap,
}

#[derive(Clone, Default, Deserialize)]
pub struct TenantTokenMap(BTreeMap<String, String>);

impl TenantTokenMap {
    pub fn parse(json: &str) -> Result<Self, AuthError> {
        let map = serde_json::from_str::<BTreeMap<String, String>>(json)
            .map_err(|_| AuthError::InvalidTokenMap)?;
        if map.is_empty() || !map.iter().all(valid_mapping) {
            return Err(AuthError::InvalidTokenMap);
        }
        Ok(Self(map))
    }

    pub fn resolve(&self, authorization: Option<&str>) -> Result<TenantIdentity, AuthError> {
        let token = bearer_token(authorization)?;
        self.0
            .get(token)
            .filter(|tenant| valid_tenant_id(tenant))
            .cloned()
            .map(|tenant_id| TenantIdentity { tenant_id })
            .ok_or(AuthError::UnknownToken)
    }

    pub fn tenant_ids(&self) -> Vec<String> {
        self.0
            .values()
            .filter(|tenant| valid_tenant_id(tenant))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

pub fn bearer_token(authorization: Option<&str>) -> Result<&str, AuthError> {
    let Some(value) = authorization else {
        return Err(AuthError::MissingBearer);
    };
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .ok_or(AuthError::MissingBearer)
}

pub fn origin_allowed(origin: Option<&str>, allowed: &[String]) -> bool {
    origin.is_none_or(|origin| allowed.iter().any(|candidate| candidate == origin))
}

pub fn parse_origin_allowlist(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn valid_tenant_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn valid_mapping((token, tenant): (&String, &String)) -> bool {
    valid_bearer_token(token) && valid_tenant_id(tenant)
}

fn valid_bearer_token(value: &str) -> bool {
    value.len() >= MIN_BEARER_TOKEN_BYTES && value.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::{AuthError, TenantTokenMap, bearer_token, origin_allowed};

    #[test]
    fn bearer_token_requires_bearer_scheme() {
        assert_eq!(bearer_token(None), Err(AuthError::MissingBearer));
        assert_eq!(
            bearer_token(Some("Basic abc")),
            Err(AuthError::MissingBearer)
        );
        assert_eq!(bearer_token(Some("Bearer secret")), Ok("secret"));
    }

    #[test]
    fn token_map_resolves_tenant_without_request_tenant_header() {
        let map =
            TenantTokenMap::parse(r#"{"0123456789abcdef0123456789abcdef":"tenant-a"}"#).unwrap();
        let identity = map
            .resolve(Some("Bearer 0123456789abcdef0123456789abcdef"))
            .unwrap();
        assert_eq!(identity.tenant_id, "tenant-a");
        assert_eq!(
            map.resolve(Some("Bearer token-b")),
            Err(AuthError::UnknownToken)
        );
    }

    #[test]
    fn token_map_rejects_empty_and_weak_configuration() {
        assert_eq!(
            TenantTokenMap::parse("{}").err(),
            Some(AuthError::InvalidTokenMap)
        );
        assert_eq!(
            TenantTokenMap::parse(r#"{"short":"tenant-a"}"#).err(),
            Some(AuthError::InvalidTokenMap)
        );
        assert_eq!(
            TenantTokenMap::parse(r#"{"0123456789abcdef0123456789abcdef":""}"#).err(),
            Some(AuthError::InvalidTokenMap)
        );
    }

    #[test]
    fn tenant_ids_are_unique_and_sorted() {
        let map = TenantTokenMap::parse(
            r#"{
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa":"tenant-b",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb":"tenant-a",
                "cccccccccccccccccccccccccccccccc":"tenant-a"
            }"#,
        )
        .unwrap();
        assert_eq!(map.tenant_ids(), vec!["tenant-a", "tenant-b"]);
    }

    #[test]
    fn origin_allowlist_allows_headless_requests() {
        let allowed = vec!["https://app.example".to_string()];
        assert!(origin_allowed(None, &allowed));
        assert!(origin_allowed(Some("https://app.example"), &allowed));
        assert!(!origin_allowed(Some("https://evil.example"), &allowed));
    }

    #[test]
    fn empty_origin_allowlist_rejects_every_browser_origin() {
        assert!(origin_allowed(None, &[]));
        assert!(!origin_allowed(Some("https://app.example"), &[]));
        assert!(!origin_allowed(Some("null"), &[]));
    }
}
