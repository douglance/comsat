//! Public HTML served at `GET /` without authentication.

pub(crate) const HTML: &str = include_str!("landing.html");

pub(crate) fn is_landing_request(method: &str, path: &str) -> bool {
    matches!(path, "/" | "/index.html")
        && (method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD"))
}

#[cfg(test)]
mod tests {
    use super::{HTML, is_landing_request};

    #[test]
    fn public_get_and_head_of_root_are_the_landing_page() {
        assert!(is_landing_request("GET", "/"));
        assert!(is_landing_request("HEAD", "/"));
        assert!(is_landing_request("get", "/index.html"));
        assert!(!is_landing_request("GET", "/health"));
        assert!(!is_landing_request("POST", "/"));
        assert!(!is_landing_request("GET", "/search"));
    }

    #[test]
    fn landing_html_explains_the_record_pipeline() {
        for needle in [
            "lang=\"en\"",
            "Skip to how it works",
            "Sources produce canonical records",
            "GitHub",
            "Hacker News",
            "Stack Exchange",
            "does not write",
            "Authorization: Bearer",
            "comsat search",
            "name=\"viewport\"",
        ] {
            assert!(HTML.contains(needle), "landing HTML missing `{needle}`");
        }
    }
}
