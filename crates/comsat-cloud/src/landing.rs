//! Public HTML served at `GET /` without authentication.

pub const HTML: &str = include_str!("landing.html");

pub fn is_landing_request(method: &str, path: &str) -> bool {
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
    fn wasm_request_router_still_serves_the_landing_module() {
        let runtime = include_str!("runtime.rs");
        let http = include_str!("runtime_http.rs");
        assert!(
            runtime.contains("crate::landing::is_landing_request"),
            "wasm router must keep the public landing request check"
        );
        assert!(
            http.contains("crate::landing::HTML"),
            "wasm landing response must keep the public HTML body"
        );
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
