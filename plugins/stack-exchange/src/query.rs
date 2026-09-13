use std::fmt::Write as _;

use comsat_types::{ErrorClass, Query, SourceError, SourceId};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const MAX_LIMIT: u32 = 50;

pub fn date_query(source: SourceId, query: &Query) -> Result<String, SourceError> {
    let mut value = String::new();
    if let Some(since) = query.since.as_deref() {
        let _ = write!(
            value,
            "&fromdate={}",
            rfc3339_to_unix(source.clone(), since)?
        );
    }
    if let Some(until) = query.until.as_deref() {
        let _ = write!(value, "&todate={}", rfc3339_to_unix(source, until)?);
    }
    Ok(value)
}

pub fn ensure_limit(source: SourceId, limit: Option<u32>) -> Result<(), SourceError> {
    if limit.unwrap_or(10) > MAX_LIMIT {
        return Err(SourceError::new(
            source,
            ErrorClass::InvalidQuery,
            format!("Stack Exchange source supports at most {MAX_LIMIT} results per search"),
        ));
    }
    Ok(())
}

pub fn unix_to_rfc3339(value: u64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(i64::try_from(value).ok()?)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
}

fn rfc3339_to_unix(source: SourceId, value: &str) -> Result<i64, SourceError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map(OffsetDateTime::unix_timestamp)
        .map_err(|error| SourceError::new(source, ErrorClass::Protocol, error.to_string()))
}
