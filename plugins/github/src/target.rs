use comsat_types::{ErrorClass, SourceError, SourceId, Target};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use url::Url;

#[derive(Debug)]
pub struct IssueTarget {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl IssueTarget {
    pub fn from_target(source: SourceId, target: &Target) -> Result<Self, SourceError> {
        match target {
            Target::Record { record } => {
                ensure_source(source.clone(), &record.source)?;
                parse_issue_url(source, &record.url)
            }
            Target::Url {
                source: target_source,
                url,
            } => {
                ensure_source(source.clone(), target_source)?;
                parse_issue_url(source, url)
            }
            Target::Native {
                source: target_source,
                id,
            } => {
                ensure_source(source.clone(), target_source)?;
                parse_native_issue(source, id)
            }
        }
    }

    pub fn encoded_owner(&self) -> String {
        path_segment(&self.owner)
    }

    pub fn encoded_repo(&self) -> String {
        path_segment(&self.repo)
    }
}

fn parse_native_issue(source: SourceId, value: &str) -> Result<IssueTarget, SourceError> {
    let Some((repo, number)) = value.rsplit_once('#') else {
        return Err(invalid_query(
            source,
            "GitHub native id must be owner/repo#number",
        ));
    };
    let Some((owner, repo)) = repo.split_once('/') else {
        return Err(invalid_query(
            source,
            "GitHub native id must be owner/repo#number",
        ));
    };
    validate_segment(source.clone(), owner)?;
    validate_segment(source.clone(), repo)?;
    let number = number
        .parse()
        .map_err(|_| invalid_query(source.clone(), "GitHub target number is invalid"))?;
    validate_number(source, number)?;
    Ok(IssueTarget {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        number,
    })
}

fn parse_issue_url(source: SourceId, value: &str) -> Result<IssueTarget, SourceError> {
    let url = Url::parse(value).map_err(|error| {
        SourceError::new(source.clone(), ErrorClass::Protocol, error.to_string())
    })?;
    if url.domain() != Some("github.com") {
        return Err(SourceError::new(
            source,
            ErrorClass::Unsupported,
            "target is not a github.com URL",
        ));
    }
    let segments = url
        .path_segments()
        .map(Iterator::collect::<Vec<_>>)
        .unwrap_or_default();
    if segments.len() < 4 || !matches!(segments[2], "issues" | "pull") {
        return Err(SourceError::new(
            source,
            ErrorClass::Unsupported,
            "target URL is not a GitHub issue or pull request",
        ));
    }
    validate_segment(source.clone(), segments[0])?;
    validate_segment(source.clone(), segments[1])?;
    let number = segments[3]
        .parse()
        .map_err(|_| invalid_query(source.clone(), "GitHub target number is invalid"))?;
    validate_number(source, number)?;
    Ok(IssueTarget {
        owner: segments[0].to_owned(),
        repo: segments[1].to_owned(),
        number,
    })
}

fn ensure_source(source: SourceId, target_source: &SourceId) -> Result<(), SourceError> {
    if target_source == &source {
        return Ok(());
    }
    Err(invalid_query(source, "target source does not match github"))
}

fn validate_segment(source: SourceId, value: &str) -> Result<(), SourceError> {
    let safe = !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if safe {
        return Ok(());
    }
    Err(invalid_query(
        source,
        "GitHub target path segment is invalid",
    ))
}

fn validate_number(source: SourceId, number: u64) -> Result<(), SourceError> {
    if number > 0 {
        return Ok(());
    }
    Err(invalid_query(source, "GitHub target number is invalid"))
}

fn path_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn invalid_query(source: SourceId, message: impl Into<String>) -> SourceError {
    SourceError::new(source, ErrorClass::InvalidQuery, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceId {
        SourceId::new("github").unwrap()
    }

    #[test]
    fn rejects_mismatched_target_source() {
        let target = Target::Native {
            source: SourceId::new("web").unwrap(),
            id: "owner/repo#1".into(),
        };
        assert!(IssueTarget::from_target(source(), &target).is_err());
    }

    #[test]
    fn rejects_unsafe_native_segments() {
        let target = Target::Native {
            source: source(),
            id: "../repo#1".into(),
        };
        assert!(IssueTarget::from_target(source(), &target).is_err());
    }
}
