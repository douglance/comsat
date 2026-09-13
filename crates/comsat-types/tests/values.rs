use comsat_types::{Query, Record, SourceId, Target};
use serde_json::json;

#[test]
fn source_identity_is_validated_during_deserialization() {
    for bad in [
        "",
        "GitHub",
        "github_issues",
        "-github",
        "git--hub",
        "git hub",
        "githüb",
    ] {
        assert!(serde_json::from_value::<SourceId>(json!(bad)).is_err());
    }
    assert_eq!(
        "hacker-news".parse::<SourceId>().unwrap().as_str(),
        "hacker-news"
    );
}

#[test]
fn dates_are_compared_as_instants() {
    let query = Query {
        text: "query".into(),
        limit: Some(1),
        since: Some("2026-01-01T01:00:00+01:00".into()),
        until: Some("2026-01-01T00:30:00Z".into()),
    };
    assert!(query.validate().is_ok());
    assert!(
        Query {
            limit: Some(0),
            ..query
        }
        .validate()
        .is_err()
    );
}

#[test]
fn target_retains_source_and_record_jsonl_roundtrips() {
    let record: Record = serde_json::from_value(json!({
        "id":"issue:1", "source":"github", "kind":"issue",
        "url":"https://github.com/a/b/issues/1", "title":"one\ntwo", "text":null,
        "author":null, "created_at":null, "updated_at":null, "metadata":{}
    }))
    .unwrap();
    record.validate().unwrap();
    let line = serde_json::to_string(&record).unwrap();
    assert!(!line.contains('\n'));
    let target = Target::Record {
        record: Box::new(record),
    };
    assert_eq!(target.source().as_str(), "github");
}

#[test]
fn record_validation_rejects_credentials_and_non_web_urls() {
    let base = json!({"id":"1","source":"web","kind":"page",
        "url":"https://example.com/", "metadata":{}});
    for url in [
        "file:///etc/passwd",
        "https://user:password@example.com/",
        "relative/path",
    ] {
        let mut value = base.clone();
        value["url"] = json!(url);
        let record: Record = serde_json::from_value(value).unwrap();
        assert!(record.validate().is_err());
    }
}

#[test]
fn target_record_uses_the_explicit_nested_wire_shape() {
    let value = json!({"type":"record","record":{
        "id":"1","source":"hacker-news","kind":"story",
        "url":"https://news.ycombinator.com/item?id=1","metadata":{}
    }});
    let target: Target = serde_json::from_value(value).unwrap();
    target.validate().unwrap();
    assert_eq!(target.source().as_str(), "hacker-news");
}

#[test]
fn record_schema_keeps_identity_fields_as_strings() {
    let schema = serde_json::to_value(schemars::schema_for!(Record)).unwrap();
    assert_eq!(schema["$defs"]["SourceId"]["type"], "string");
    assert_eq!(schema["$defs"]["RecordId"]["type"], "string");
    assert_eq!(schema["properties"]["metadata"]["type"], "object");
}
