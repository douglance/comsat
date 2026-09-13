use super::*;
use incurs::tool::ToolDefinition;

#[test]
fn resolves_namespaced_source_tools() {
    let extension = SourceExtension {
        id: SourceId::new("fixture-source").unwrap(),
        display_name: "Fixture Source".to_string(),
        supports_fetch: true,
        supports_follow: false,
    };
    let definitions = vec![
        definition("fixture_search"),
        definition("fixture_fetch"),
        definition("fixture_diagnostic"),
    ];

    let descriptor = extension.descriptor(&definitions).unwrap();

    assert_eq!(descriptor.commands.search, "fixture_search");
    assert_eq!(descriptor.commands.fetch, "fixture_fetch");
    assert!(!descriptor.profile.follow);
}

#[test]
fn requires_advertised_tools() {
    let extension = SourceExtension {
        id: SourceId::new("fixture-source").unwrap(),
        display_name: "Fixture Source".to_string(),
        supports_fetch: true,
        supports_follow: false,
    };
    let error = extension
        .descriptor(&[definition("fixture_search")])
        .unwrap_err();

    assert!(error.to_string().contains("missing required `fetch` tool"));
}

#[test]
fn recognizes_comsat_tool_prefix() {
    let names = ["comsat_fixture_source_search"];
    let source = SourceId::new("fixture-source").unwrap();

    assert_eq!(
        find_tool(&names, &source, "search").unwrap().as_deref(),
        Some("comsat_fixture_source_search")
    );
}

#[test]
fn rejects_ambiguous_namespaced_tools() {
    let names = ["one_search", "two_search"];
    let source = SourceId::new("fixture-source").unwrap();
    let error = find_tool(&names, &source, "search").unwrap_err();

    assert!(error.to_string().contains("ambiguous"));
}

#[tokio::test]
async fn reports_empty_agent_plugin_package_without_aborting() {
    let root = unique_test_dir("empty-package");
    std::fs::create_dir_all(&root).unwrap();
    let data = unique_test_dir("empty-package-data");

    let report = load_sources([root.clone()], &data).await.unwrap();

    assert!(report.sources.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].root, root);
    assert!(
        report.diagnostics[0]
            .message
            .contains("invalid Agent Plugin package")
    );
}

#[tokio::test]
async fn reports_malformed_agent_plugin_manifest_without_aborting() {
    let root = unique_test_dir("malformed-package");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("plugin.json"), "{ not json").unwrap();
    let data = unique_test_dir("malformed-package-data");

    let report = load_sources([root.clone()], &data).await.unwrap();

    assert!(report.sources.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].root, root);
    assert!(
        report.diagnostics[0]
            .message
            .contains("invalid Agent Plugin package")
    );
}

#[tokio::test]
async fn loads_external_source_agent_plugin_over_stdio_mcp() {
    let source = load_fixture_source().await;
    assert_fixture_descriptor(&source);

    let source_id = SourceId::new("fixture-source").unwrap();
    let catalog = SourceCatalog::new(vec![source]);
    let search = assert_fixture_search(&catalog, &source_id).await;
    let native = native_target(&source_id, search.records[0].id.to_string());
    assert_fixture_fetch(&catalog, native.clone(), &source_id).await;
    assert_fixture_follow(&catalog, native).await;

    let record = record_target(search.records[0].clone());
    assert_fixture_fetch(&catalog, record.clone(), &source_id).await;
    assert_fixture_follow(&catalog, record).await;
}

async fn load_fixture_source() -> CatalogSource {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/external-source");
    let data = std::env::temp_dir().join(format!("comsat-plugin-test-{}", std::process::id()));

    load_source(root, &data).await.unwrap()
}

fn assert_fixture_descriptor(source: &CatalogSource) {
    assert_eq!(source.descriptor.id.as_str(), "fixture-source");
    assert!(
        source
            .descriptor
            .commands
            .search
            .ends_with("fixture_search")
    );
    assert!(source.descriptor.commands.fetch.ends_with("fixture_fetch"));
    assert!(
        source
            .descriptor
            .commands
            .follow
            .ends_with("fixture_follow")
    );
    assert!(source.descriptor.profile.fetch);
    assert!(source.descriptor.profile.follow);
}

async fn assert_fixture_search(
    catalog: &SourceCatalog,
    source_id: &SourceId,
) -> comsat_engine::SearchOutcome {
    let search = catalog
        .collect_search(
            search_request(source_id),
            comsat_engine::EngineLimits::default(),
        )
        .await;
    assert_eq!(search.diagnostics, Vec::new());
    assert_eq!(search.records.len(), 1);
    assert_eq!(search.records[0].source, *source_id);
    assert_eq!(search.records[0].kind, "search-result");
    search
}

fn search_request(source_id: &SourceId) -> comsat_engine::SearchRequest {
    comsat_engine::SearchRequest {
        query: comsat_types::Query {
            text: "pinned runtime".to_string(),
            limit: Some(1),
            since: None,
            until: None,
        },
        sources: vec![source_id.clone()],
        strict: true,
    }
}

fn native_target(source_id: &SourceId, id: String) -> comsat_types::Target {
    comsat_types::Target::Native {
        source: source_id.clone(),
        id,
    }
}

fn record_target(record: comsat_types::Record) -> comsat_types::Target {
    comsat_types::Target::Record {
        record: Box::new(record),
    }
}

async fn assert_fixture_fetch(
    catalog: &SourceCatalog,
    target: comsat_types::Target,
    source_id: &SourceId,
) {
    let fetched = catalog
        .fetch(target, comsat_engine::EngineLimits::default())
        .await
        .unwrap();
    assert_eq!(fetched.kind, "document");
    assert_eq!(fetched.source, *source_id);
}

async fn assert_fixture_follow(catalog: &SourceCatalog, target: comsat_types::Target) {
    let followed = catalog
        .collect_follow(target, comsat_engine::EngineLimits::default())
        .await;
    assert_eq!(followed.diagnostics, Vec::new());
    assert_eq!(followed.records.len(), 1);
    assert_eq!(followed.records[0].kind, "comment");
}

fn unique_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "comsat-plugin-{name}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ))
}

fn definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: String::new(),
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: None,
        annotations: None,
        instructions: None,
        examples: Vec::new(),
        result_content: Vec::new(),
    }
}
