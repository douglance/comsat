use comsat_types::{ErrorClass, SourceError, SourceId, Target};
use url::Url;

pub fn target_id(source: SourceId, target: &Target) -> Result<u64, SourceError> {
    match target {
        Target::Record { record } => {
            ensure_source(source.clone(), &record.source)?;
            record
                .id
                .as_str()
                .strip_prefix("hacker-news:")
                .and_then(|id| id.parse().ok())
                .or_else(|| {
                    Url::parse(&record.url)
                        .ok()
                        .and_then(|url| id_from_url(&url))
                })
                .ok_or_else(|| {
                    invalid_query(
                        source,
                        "Hacker News record target does not contain an item id",
                    )
                })
        }
        Target::Url {
            source: target_source,
            url,
        } => {
            ensure_source(source.clone(), target_source)?;
            Url::parse(url)
                .ok()
                .and_then(|url| id_from_url(&url))
                .ok_or_else(|| invalid_query(source, "Hacker News URL target must include item id"))
        }
        Target::Native {
            source: target_source,
            id,
        } => {
            ensure_source(source.clone(), target_source)?;
            id.parse().map_err(|_| {
                invalid_query(source, "Hacker News native id must be a numeric item id")
            })
        }
    }
}

fn id_from_url(url: &Url) -> Option<u64> {
    if url.domain() != Some("news.ycombinator.com") {
        return None;
    }
    url.query_pairs()
        .find(|(name, _)| name == "id")
        .and_then(|(_, value)| value.parse().ok())
}

fn invalid_query(source: SourceId, message: impl Into<String>) -> SourceError {
    SourceError::new(source, ErrorClass::InvalidQuery, message)
}

fn ensure_source(source: SourceId, target_source: &SourceId) -> Result<(), SourceError> {
    if target_source == &source {
        return Ok(());
    }
    Err(invalid_query(
        source,
        "target source does not match hacker-news",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_mismatched_target_source() {
        let target = Target::Native {
            source: SourceId::new("web").unwrap(),
            id: "43192810".into(),
        };
        assert!(target_id(SourceId::new("hacker-news").unwrap(), &target).is_err());
    }
}
