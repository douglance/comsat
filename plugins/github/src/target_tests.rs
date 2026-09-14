use comsat_types::{SourceError, SourceId, Target};

use super::{GitHubTarget, RepositoryRef};

fn source() -> SourceId {
    SourceId::new("github").unwrap()
}

fn native(id: &str) -> Result<GitHubTarget, SourceError> {
    GitHubTarget::from_target(
        source(),
        &Target::Native {
            source: source(),
            id: id.to_owned(),
        },
    )
}

fn url(value: &str) -> Result<GitHubTarget, SourceError> {
    GitHubTarget::from_target(
        source(),
        &Target::Url {
            source: source(),
            url: value.to_owned(),
        },
    )
}

fn repository() -> RepositoryRef {
    RepositoryRef {
        owner: "rust-lang".to_owned(),
        repo: "rust".to_owned(),
    }
}

#[test]
fn rejects_mismatched_target_source() {
    let target = Target::Native {
        source: SourceId::new("web").unwrap(),
        id: "owner/repo#1".into(),
    };

    assert!(GitHubTarget::from_target(source(), &target).is_err());
}

#[test]
fn rejects_unsafe_native_segments() {
    assert!(native("../repo#1").is_err());
    assert!(native("owner/repo/discussions/0").is_err());
    assert!(native("owner/repo/releases/1").is_err());
}

#[test]
fn reads_every_native_object_class() {
    assert_eq!(
        native("rust-lang/rust").unwrap(),
        GitHubTarget::Repository(repository())
    );
    assert_eq!(
        native("rust-lang/rust#12").unwrap(),
        GitHubTarget::Issue {
            repository: repository(),
            number: 12,
            pull_request: None,
        }
    );
    assert_eq!(
        native("rust-lang/rust/discussions/7").unwrap(),
        GitHubTarget::Discussion {
            repository: repository(),
            number: 7,
        }
    );
}

#[test]
fn reads_every_url_object_class_and_marks_pull_requests() {
    assert_eq!(
        url("https://github.com/rust-lang/rust").unwrap(),
        GitHubTarget::Repository(repository())
    );
    assert_eq!(
        url("https://github.com/rust-lang/rust/pull/9").unwrap(),
        GitHubTarget::Issue {
            repository: repository(),
            number: 9,
            pull_request: Some(true),
        }
    );
    assert_eq!(
        url("https://github.com/rust-lang/rust/issues/9").unwrap(),
        GitHubTarget::Issue {
            repository: repository(),
            number: 9,
            pull_request: Some(false),
        }
    );
    assert_eq!(
        url("https://github.com/rust-lang/rust/discussions/9").unwrap(),
        GitHubTarget::Discussion {
            repository: repository(),
            number: 9,
        }
    );
}

#[test]
fn rejects_unsupported_urls() {
    assert!(url("https://gitlab.com/rust-lang/rust").is_err());
    assert!(url("https://github.com/rust-lang/rust/actions/runs/1").is_err());
}
