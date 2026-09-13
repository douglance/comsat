use comsat_types::{ErrorClass, SourceError, SourceId};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub fn rfc3339_to_unix(source: SourceId, value: &str) -> Result<i64, SourceError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map(OffsetDateTime::unix_timestamp)
        .map_err(|error| SourceError::new(source, ErrorClass::Protocol, error.to_string()))
}

pub fn unix_to_rfc3339(value: u64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(i64::try_from(value).ok()?)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
}
