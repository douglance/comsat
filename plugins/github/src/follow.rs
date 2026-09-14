//! Follow traversal: the discussion attached to a GitHub object. Issues yield
//! their comments, pull requests add their review discussion, repositories
//! yield their discussions, and a discussion yields its comments.

use comsat_types::{Record, SourceError, Target};

use crate::GitHubSource;
use crate::discussions::{self, Discussion, DiscussionComment};
use crate::normalize::{self, Issue, IssueComment, Review, ReviewComment, source_id};
use crate::query::MAX_LIMIT;
use crate::target::{GitHubTarget, RepositoryRef};

impl GitHubSource {
    pub(crate) async fn follow_records(&self, target: Target) -> Result<Vec<Record>, SourceError> {
        self.cooldowns.check(source_id(), "follow")?;
        match GitHubTarget::from_target(source_id(), &target)? {
            GitHubTarget::Repository(repository) => self.repository_discussions(&repository).await,
            GitHubTarget::Issue {
                repository,
                number,
                pull_request,
            } => {
                self.issue_discussion(&repository, number, pull_request)
                    .await
            }
            GitHubTarget::Discussion { repository, number } => {
                self.discussion_comments(&repository, number).await
            }
        }
    }

    /// Issue comments, plus the review discussion when the target is a pull
    /// request. An unmarked target is resolved once so a plain issue never pays
    /// for review lookups.
    async fn issue_discussion(
        &self,
        repository: &RepositoryRef,
        number: u64,
        pull_request: Option<bool>,
    ) -> Result<Vec<Record>, SourceError> {
        let pull_request = match pull_request {
            Some(known) => known,
            None => self.is_pull_request(repository, number).await?,
        };
        let mut records = self.issue_comments(repository, number).await?;
        if pull_request {
            records.extend(self.reviews(repository, number).await?);
            records.extend(self.review_comments(repository, number).await?);
        }
        Ok(records)
    }

    async fn is_pull_request(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<bool, SourceError> {
        let issue: Issue = self.rest("follow", issue_url(repository, number)).await?;
        Ok(issue.pull_request.is_some())
    }

    async fn issue_comments(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<Vec<Record>, SourceError> {
        let url = format!(
            "{}/comments?per_page={MAX_LIMIT}",
            issue_url(repository, number)
        );
        let comments: Vec<IssueComment> = self.rest("follow", url).await?;
        comments
            .iter()
            .map(|comment| normalize::issue_comment(comment, repository, number))
            .collect()
    }

    async fn reviews(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<Vec<Record>, SourceError> {
        let url = format!(
            "{}/reviews?per_page={MAX_LIMIT}",
            pull_url(repository, number)
        );
        let reviews: Vec<Review> = self.rest("follow", url).await?;
        reviews
            .iter()
            .map(|review| normalize::review(review, repository, number))
            .collect()
    }

    async fn review_comments(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<Vec<Record>, SourceError> {
        let url = format!(
            "{}/comments?per_page={MAX_LIMIT}",
            pull_url(repository, number)
        );
        let comments: Vec<ReviewComment> = self.rest("follow", url).await?;
        comments
            .iter()
            .map(|comment| normalize::review_comment(comment, repository, number))
            .collect()
    }

    async fn repository_discussions(
        &self,
        repository: &RepositoryRef,
    ) -> Result<Vec<Record>, SourceError> {
        let request = discussions::repository_request(repository, MAX_LIMIT);
        let data = self.graphql("follow", &request).await?;
        let nodes: Vec<Discussion> = discussions::nodes(&data, "/repository/discussions/nodes")?;
        nodes
            .iter()
            .map(|discussion| discussions::discussion_record(discussion, repository))
            .collect()
    }

    async fn discussion_comments(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<Vec<Record>, SourceError> {
        let request = discussions::comments_request(repository, number, MAX_LIMIT);
        let data = self.graphql("follow", &request).await?;
        let nodes: Vec<DiscussionComment> =
            discussions::nodes(&data, "/repository/discussion/comments/nodes")?;
        nodes
            .iter()
            .map(|comment| discussions::comment_record(comment, repository, number))
            .collect()
    }
}

fn issue_url(repository: &RepositoryRef, number: u64) -> String {
    format!(
        "https://api.github.com/repos/{}/{}/issues/{number}",
        repository.encoded_owner(),
        repository.encoded_repo()
    )
}

fn pull_url(repository: &RepositoryRef, number: u64) -> String {
    format!(
        "https://api.github.com/repos/{}/{}/pulls/{number}",
        repository.encoded_owner(),
        repository.encoded_repo()
    )
}
