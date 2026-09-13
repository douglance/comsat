use comsat_types::{ErrorClass, SourceError, SourceId, Target};
use serde_json::Value;
use url::Url;

pub fn question_id(source: SourceId, site: &str, target: &Target) -> Result<u64, SourceError> {
    match target {
        Target::Record { record } => {
            ensure_source(source.clone(), &record.source)?;
            record
                .metadata
                .get("question_id")
                .and_then(Value::as_u64)
                .or_else(|| {
                    Url::parse(&record.url)
                        .ok()
                        .and_then(|url| question_id_from_url(site, &url))
                })
                .ok_or_else(|| {
                    invalid_query(
                        source,
                        "Stack Exchange record target does not contain a question id",
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
                .and_then(|url| question_id_from_url(site, &url))
                .ok_or_else(|| {
                    invalid_query(
                        source,
                        "Stack Exchange URL target must include a question id",
                    )
                })
        }
        Target::Native {
            source: target_source,
            id,
        } => {
            ensure_source(source.clone(), target_source)?;
            id.parse().map_err(|_| {
                invalid_query(
                    source,
                    "Stack Exchange native id must be a numeric question id",
                )
            })
        }
    }
}

fn question_id_from_url(site: &str, url: &Url) -> Option<u64> {
    if !host_matches_site(site, url.domain()?) {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    let index = segments
        .iter()
        .position(|segment| matches!(*segment, "questions" | "q"))?;
    segments.get(index + 1)?.parse().ok()
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
        "target source does not match stack-exchange",
    ))
}

fn host_matches_site(site: &str, host: &str) -> bool {
    match site {
        "stackoverflow" => host == "stackoverflow.com",
        "serverfault" => host == "serverfault.com",
        "superuser" => host == "superuser.com",
        site => host == format!("{site}.stackexchange.com"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceId {
        SourceId::new("stack-exchange").unwrap()
    }

    #[test]
    fn extracts_question_id_from_matching_url() {
        let url = Url::parse("https://stackoverflow.com/questions/481/example").unwrap();
        assert_eq!(question_id_from_url("stackoverflow", &url), Some(481));
    }

    #[test]
    fn rejects_mismatched_host_url() {
        let target = Target::Url {
            source: source(),
            url: "https://evil.com/questions/481/example".into(),
        };
        assert!(question_id(source(), "stackoverflow", &target).is_err());
    }

    #[test]
    fn rejects_mismatched_target_source() {
        let target = Target::Native {
            source: SourceId::new("github").unwrap(),
            id: "481".into(),
        };
        assert!(question_id(source(), "stackoverflow", &target).is_err());
    }
}
