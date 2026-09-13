use std::{cell::RefCell, sync::Arc};

use comsat_app::{ComsatApp, build_cli};
use comsat_engine::{CatalogSource, SourceCatalog};
use comsat_github::GitHubSource;
use comsat_hacker_news::HackerNewsSource;
use comsat_source::{HttpClient, SourceRuntime, source_commands};
use comsat_stack_exchange::StackExchangeSource;
use comsat_store::Store;
use comsat_web::WebSource;
use incurs::cli::Cli;
use incurs::tool::ToolCatalog;
use worker::Env;

use crate::{cloud_config::cloud_limits, cloud_http::CloudflareHttpClient};

thread_local! {
    static SOURCE_CATALOG_CACHE: RefCell<Option<Arc<SourceCatalog>>> = const { RefCell::new(None) };
}

pub fn source_catalog(env: &Env) -> Arc<SourceCatalog> {
    if let Some(catalog) = cached_catalog() {
        return catalog;
    }
    let catalog = build_source_catalog(env);
    SOURCE_CATALOG_CACHE.with(|cache| {
        *cache.borrow_mut() = Some(catalog.clone());
    });
    catalog
}

fn cached_catalog() -> Option<Arc<SourceCatalog>> {
    SOURCE_CATALOG_CACHE.with(|cache| cache.borrow().clone())
}

fn build_source_catalog(env: &Env) -> Arc<SourceCatalog> {
    let github = GitHubSource::descriptor();
    let hacker_news = HackerNewsSource::descriptor();
    let web = WebSource::descriptor();
    let stack_exchange = StackExchangeSource::descriptor();
    Arc::new(SourceCatalog::new(vec![
        catalog_source(
            github.clone(),
            Arc::new(GitHubSource::new(
                http_client(&github),
                optional_secret(env, "GITHUB_TOKEN"),
            )),
        ),
        catalog_source(
            hacker_news.clone(),
            Arc::new(HackerNewsSource::new(http_client(&hacker_news))),
        ),
        catalog_source(
            web.clone(),
            Arc::new(WebSource::new(
                http_client(&web),
                optional_secret(env, "BRAVE_SEARCH_API_KEY").unwrap_or_default(),
            )),
        ),
        catalog_source(
            stack_exchange.clone(),
            Arc::new(StackExchangeSource::new(
                http_client(&stack_exchange),
                optional_var(env, "STACK_EXCHANGE_SITE"),
                optional_secret(env, "STACK_EXCHANGE_API_KEY"),
            )),
        ),
    ]))
}

fn http_client(descriptor: &comsat_source::SourceDescriptor) -> Arc<dyn HttpClient> {
    Arc::new(CloudflareHttpClient::new(descriptor.id.clone()))
}

pub fn app_catalog(
    catalog: Arc<SourceCatalog>,
    store: Arc<dyn Store>,
    tenant_id: String,
    clock: fn() -> i64,
) -> ToolCatalog {
    let app = ComsatApp::new(catalog, tenant_id, Arc::new(clock))
        .with_store(store)
        .with_limits(cloud_limits())
        .with_lease_owner("comsat-cloud");
    build_cli(Arc::new(app)).tool_catalog()
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the Arc documents source runtime ownership at the catalog construction boundary."
)]
fn catalog_source(
    descriptor: comsat_source::SourceDescriptor,
    runtime: Arc<dyn SourceRuntime>,
) -> CatalogSource {
    let mut cli = Cli::create(descriptor.id.as_str());
    for command in source_commands(&descriptor, runtime.clone()) {
        let name = command.name.clone();
        cli = cli.command(name, command);
    }
    CatalogSource::new(descriptor, cli.tool_catalog())
}

fn optional_secret(env: &Env, name: &str) -> Option<String> {
    env.secret(name)
        .ok()
        .map(|secret| secret.to_string())
        .filter(|value| !value.is_empty())
}

fn optional_var(env: &Env, name: &str) -> Option<String> {
    env.var(name)
        .ok()
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}
