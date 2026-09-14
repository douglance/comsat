use comsat_types::{ErrorClass, SourceError, SourceId, Target};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use url::Url;

/// A GitHub object a fetch or follow can address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubTarget {
    Repository(RepositoryRef),
    /// An issue or pull request. `pull_request` is known only when the address
    /// said so; otherwise the source asks GitHub.
    Issue {
        repository: RepositoryRef,
        number: u64,
        pull_request: Option<bool>,
    },
    Discussion {
        repository: RepositoryRef,
        number: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRef {
    pub owner: String,
    pub repo: String,
}

impl RepositoryRef {
    pub fn encoded_owner(&self) -> String {
        path_segment(&self.owner)
    }

    pub fn encoded_repo(&self) -> String {
        path_segment(&self.repo)
    }

    pub fn name_with_owner(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

impl GitHubTarget {
    pub fn from_target(source: SourceId, target: &Target) -> Result<Self, SourceError> {
        match target {
            Target::Record { record } => {
                ensure_source(source.clone(), &record.source)?;
                parse_url(source, &record.url)
            }
            Target::Url {
                source: target_source,
                url,
            } => {
                ensure_source(source.clone(), target_source)?;
                parse_url(source, url)
            }
            Target::Native {
                source: target_source,
                id,
            } => {
                ensure_source(source.clone(), target_source)?;
                parse_native(source, id)
            }
        }
    }
}

/// Native ids: `owner/repo`, `owner/repo#number`, `owner/repo/discussions/number`.
fn parse_native(source: SourceId, value: &str) -> Result<GitHubTarget, SourceError> {
    if let Some((repository, number)) = value.rsplit_once('#') {
        let repository = repository_ref(source.clone(), repository)?;
        return Ok(GitHubTarget::Issue {
            repository,
            number: parse_number(source, number)?,
            pull_request: None,
        });
    }
    let segments = value.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        [owner, repo] => Ok(GitHubTarget::Repository(named_repository(
            source, owner, repo,
        )?)),
        [owner, repo, "discussions", number] => Ok(GitHubTarget::Discussion {
            repository: named_repository(source.clone(), owner, repo)?,
            number: parse_number(source, number)?,
        }),
        _ => Err(invalid_query(
            source,
            "GitHub native id must be owner/repo, owner/repo#number, or owner/repo/discussions/number",
        )),
    }
}

fn parse_url(source: SourceId, value: &str) -> Result<GitHubTarget, SourceError> {
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
    match segments.as_slice() {
        [owner, repo] => Ok(GitHubTarget::Repository(named_repository(
            source, owner, repo,
        )?)),
        [owner, repo, kind @ ("issues" | "pull"), number] => Ok(GitHubTarget::Issue {
            repository: named_repository(source.clone(), owner, repo)?,
            number: parse_number(source, number)?,
            pull_request: Some(*kind == "pull"),
        }),
        [owner, repo, "discussions", number] => Ok(GitHubTarget::Discussion {
            repository: named_repository(source.clone(), owner, repo)?,
            number: parse_number(source, number)?,
        }),
        _ => Err(SourceError::new(
            source,
            ErrorClass::Unsupported,
            "target URL is not a GitHub repository, issue, pull request, or discussion",
        )),
    }
}

fn repository_ref(source: SourceId, value: &str) -> Result<RepositoryRef, SourceError> {
    let Some((owner, repo)) = value.split_once('/') else {
        return Err(invalid_query(
            source,
            "GitHub native id must name owner/repo",
        ));
    };
    named_repository(source, owner, repo)
}

fn named_repository(
    source: SourceId,
    owner: &str,
    repo: &str,
) -> Result<RepositoryRef, SourceError> {
    validate_segment(source.clone(), owner)?;
    validate_segment(source, repo)?;
    Ok(RepositoryRef {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
    })
}

fn parse_number(source: SourceId, value: &str) -> Result<u64, SourceError> {
    let number = value
        .parse()
        .map_err(|_| invalid_query(source.clone(), "GitHub target number is invalid"))?;
    if number == 0 {
        return Err(invalid_query(source, "GitHub target number is invalid"));
    }
    Ok(number)
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

fn path_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn invalid_query(source: SourceId, message: impl Into<String>) -> SourceError {
    SourceError::new(source, ErrorClass::InvalidQuery, message)
}

// The module file cannot live in a `target/` directory: that name is the build
// output pattern this repository ignores, so the file would never be committed.
#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
