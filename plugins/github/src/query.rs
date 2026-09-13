use comsat_types::{ErrorClass, Query, SourceError, SourceId};

pub const MAX_LIMIT: u32 = 50;

pub fn ensure_limit(source: SourceId, limit: Option<u32>) -> Result<(), SourceError> {
    if limit.unwrap_or(10) > MAX_LIMIT {
        return Err(SourceError::new(
            source,
            ErrorClass::InvalidQuery,
            format!("GitHub source supports at most {MAX_LIMIT} results per search"),
        ));
    }
    Ok(())
}

pub fn github_query_text(query: &Query) -> String {
    let mut parts = vec![query.text.clone()];
    if let Some(since) = query.since.as_deref() {
        parts.push(format!(
            "created:>={}",
            since.split('T').next().unwrap_or(since)
        ));
    }
    if let Some(until) = query.until.as_deref() {
        parts.push(format!(
            "created:<={}",
            until.split('T').next().unwrap_or(until)
        ));
    }
    parts.join(" ")
}
