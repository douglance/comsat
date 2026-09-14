use comsat_types::{ErrorClass, Query, SourceError, SourceId};

pub const MAX_LIMIT: u32 = 50;
pub const DEFAULT_LIMIT: u32 = 10;

/// The GitHub object class a search returns. COMSAT's canonical query carries
/// only text, so the class is selected with a `type:` term, matching GitHub's
/// own search vocabulary. `type:issue` and `type:pr` stay in the text for
/// GitHub to interpret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    Issues,
    Repositories,
    Discussions,
}

pub fn ensure_limit(source: SourceId, limit: Option<u32>) -> Result<(), SourceError> {
    if limit.unwrap_or(DEFAULT_LIMIT) > MAX_LIMIT {
        return Err(SourceError::new(
            source,
            ErrorClass::InvalidQuery,
            format!("GitHub source supports at most {MAX_LIMIT} results per search"),
        ));
    }
    Ok(())
}

/// Splits the canonical query into the GitHub object class and the search text
/// GitHub receives, including date bounds.
pub fn github_search(query: &Query) -> (SearchKind, String) {
    let mut kind = SearchKind::Issues;
    let mut terms = Vec::new();
    for term in query.text.split_whitespace() {
        match selected_kind(term) {
            Some(selected) => kind = selected,
            None => terms.push(term.to_owned()),
        }
    }
    terms.extend(date_bounds(query));
    (kind, terms.join(" "))
}

fn selected_kind(term: &str) -> Option<SearchKind> {
    match term.strip_prefix("type:")? {
        "repo" | "repository" | "repositories" => Some(SearchKind::Repositories),
        "discussion" | "discussions" => Some(SearchKind::Discussions),
        _ => None,
    }
}

fn date_bounds(query: &Query) -> Vec<String> {
    let mut bounds = Vec::new();
    if let Some(since) = query.since.as_deref() {
        bounds.push(format!("created:>={}", date(since)));
    }
    if let Some(until) = query.until.as_deref() {
        bounds.push(format!("created:<={}", date(until)));
    }
    bounds
}

fn date(value: &str) -> &str {
    value.split('T').next().unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::{SearchKind, github_search};
    use comsat_types::Query;

    fn query(text: &str) -> Query {
        Query {
            text: text.to_owned(),
            limit: None,
            since: None,
            until: None,
        }
    }

    #[test]
    fn defaults_to_issue_search_and_keeps_github_type_terms() {
        let (kind, text) = github_search(&query("tokio type:pr"));

        assert_eq!(kind, SearchKind::Issues);
        assert_eq!(text, "tokio type:pr");
    }

    #[test]
    fn selects_repositories_and_removes_the_selector() {
        let (kind, text) = github_search(&query("type:repo rust search"));

        assert_eq!(kind, SearchKind::Repositories);
        assert_eq!(text, "rust search");
    }

    #[test]
    fn selects_discussions_and_keeps_repository_scope() {
        let (kind, text) = github_search(&query("type:discussions repo:vercel/next.js turbopack"));

        assert_eq!(kind, SearchKind::Discussions);
        assert_eq!(text, "repo:vercel/next.js turbopack");
    }

    #[test]
    fn appends_created_bounds() {
        let mut value = query("comsat");
        value.since = Some("2026-01-01T00:00:00Z".into());
        value.until = Some("2026-02-01T00:00:00Z".into());

        let (_, text) = github_search(&value);

        assert_eq!(text, "comsat created:>=2026-01-01 created:<=2026-02-01");
    }
}
