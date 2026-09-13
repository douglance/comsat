#![forbid(unsafe_code)]

mod codemode;
mod http_client;
mod native_store;
mod notifications;
mod output;
mod plugins;

use std::{
    env, io,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use comsat_app::{ComsatApp, TargetStream, TargetStreamProvider, build_cli};
use comsat_engine::{CatalogSource, SourceCatalog};
use comsat_source::{HttpClient, SourceRuntime, source_commands};
use comsat_types::{Record, Target};
use futures::SinkExt;
use incurs::cli::Cli;
use incurs::{
    command::{CommandContext, CommandDef, CommandHandler},
    output::CommandResult,
};
use serde::Deserialize;

use http_client::{SecureHttpClient, enable_serve_logging};
use native_store::{LazySqliteStore, database_path};

const RESPONSE_LIMIT_BYTES: usize = 2 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = Arc::new(build_app().await?);
    let cli = native_cli(Arc::clone(&app));
    let argv = env::args().skip(1).collect::<Vec<_>>();

    let mut writer = output::UnixOutput::new(io::stdout(), io::stderr());
    let exit = cli.serve_to(argv, &mut writer, false).await?;
    writer.finish()?;
    if env::var("COMSAT_LOG").ok().as_deref() == Some("1") {
        log_catalog_metrics(&app);
    }
    let invalid_invocation = writer.invalid_invocation();
    if let Some(code) = exit {
        std::process::exit(if code == 1 && invalid_invocation {
            2
        } else {
            code
        });
    }
    Ok(())
}

fn native_cli(app: Arc<ComsatApp>) -> Cli {
    build_cli(Arc::clone(&app))
        .group(codemode::group(Arc::clone(&app)))
        .command("serve", serve_command(app))
}

async fn build_app() -> Result<ComsatApp, Box<dyn std::error::Error>> {
    let mut sources = first_party_sources();
    let plugin_catalog = plugins::catalog_with_agent_plugins().await?;
    sources.extend(plugin_catalog.sources().iter().cloned());

    let store = Arc::new(LazySqliteStore::new(database_path()));
    let tenant = env::var("COMSAT_TENANT").unwrap_or_else(|_| "default".to_string());
    let lease_owner = format!("comsat-native-{}", std::process::id());
    Ok(notifications::configure(
        ComsatApp::new(
            Arc::new(SourceCatalog::try_new(sources)?),
            tenant,
            native_clock(),
        )
        .with_store(store)
        .with_target_stream_provider(Arc::new(StdinTargets))
        .with_lease_owner(lease_owner),
    )?)
}

fn first_party_sources() -> Vec<CatalogSource> {
    let client: Arc<dyn HttpClient> = Arc::new(SecureHttpClient::new());
    let sources: Vec<(comsat_source::SourceDescriptor, Arc<dyn SourceRuntime>)> = vec![
        (
            comsat_github::GitHubSource::descriptor(),
            Arc::new(comsat_github::GitHubSource::new(
                Arc::clone(&client),
                env::var("COMSAT_GITHUB_TOKEN")
                    .ok()
                    .or_else(|| env::var("GITHUB_TOKEN").ok()),
            )),
        ),
        (
            comsat_hacker_news::HackerNewsSource::descriptor(),
            Arc::new(comsat_hacker_news::HackerNewsSource::new(Arc::clone(
                &client,
            ))),
        ),
        (
            comsat_stack_exchange::StackExchangeSource::descriptor(),
            Arc::new(comsat_stack_exchange::StackExchangeSource::new(
                Arc::clone(&client),
                env::var("COMSAT_STACK_EXCHANGE_SITE").ok(),
                env::var("COMSAT_STACK_EXCHANGE_KEY").ok(),
            )),
        ),
        (
            comsat_web::WebSource::descriptor(),
            Arc::new(comsat_web::WebSource::new(
                Arc::clone(&client),
                env::var("COMSAT_BRAVE_API_KEY")
                    .ok()
                    .or_else(|| env::var("BRAVE_API_KEY").ok())
                    .unwrap_or_default(),
            )),
        ),
    ];
    sources
        .into_iter()
        .map(|(descriptor, runtime)| catalog_source(descriptor, &runtime))
        .collect()
}

fn catalog_source(
    descriptor: comsat_source::SourceDescriptor,
    runtime: &Arc<dyn SourceRuntime>,
) -> CatalogSource {
    let mut cli = Cli::create(descriptor.id.as_str());
    for command in source_commands(&descriptor, Arc::clone(runtime)) {
        cli = cli.command(command.name.clone(), command);
    }
    CatalogSource::new(descriptor, cli.tool_catalog())
}

struct StdinTargets;

impl TargetStreamProvider for StdinTargets {
    #[allow(clippy::significant_drop_tightening)]
    fn read_targets(&self) -> TargetStream {
        let (mut sender, receiver) = futures::channel::mpsc::channel(1);
        std::thread::spawn(move || {
            read_stdin_targets(&mut sender);
        });
        Box::pin(receiver)
    }
}

fn read_stdin_targets(sender: &mut futures::channel::mpsc::Sender<Result<Target, String>>) {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let mut state = StdinTargetState::default();
    loop {
        match read_stdin_line(&mut reader, state.line_number + 1) {
            StdinLine::Target(target) => {
                state.emitted = true;
                state.line_number += 1;
                send_target(sender, Ok(target));
            }
            StdinLine::Skip => state.line_number += 1,
            StdinLine::Eof => break,
            StdinLine::Error(error) => {
                state.failed = true;
                send_target(sender, Err(error));
                break;
            }
        }
    }
    if !state.emitted && !state.failed {
        send_target(
            sender,
            Err("stdin JSONL target stream was empty".to_string()),
        );
    }
}

#[derive(Default)]
struct StdinTargetState {
    line_number: usize,
    emitted: bool,
    failed: bool,
}

enum StdinLine {
    Target(Target),
    Skip,
    Eof,
    Error(String),
}

fn read_stdin_line(reader: &mut impl std::io::BufRead, line_number: usize) -> StdinLine {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => StdinLine::Eof,
        Ok(_) => parse_stdin_line(&line, line_number),
        Err(error) => StdinLine::Error(error.to_string()),
    }
}

fn parse_stdin_line(line: &str, line_number: usize) -> StdinLine {
    if line.len() > RESPONSE_LIMIT_BYTES {
        return StdinLine::Error(format!(
            "stdin line {line_number}: target JSONL line exceeded 2 MiB"
        ));
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return StdinLine::Skip;
    }
    parse_stdin_target(trimmed, line_number).map_or_else(StdinLine::Error, StdinLine::Target)
}

fn send_target(
    sender: &mut futures::channel::mpsc::Sender<Result<Target, String>>,
    item: Result<Target, String>,
) {
    let _ = futures::executor::block_on(sender.send(item));
}

fn parse_stdin_target(line: &str, line_number: usize) -> Result<Target, String> {
    let target = serde_json::from_str::<Target>(line)
        .or_else(|_| {
            serde_json::from_str::<Record>(line).map(|record| Target::Record {
                record: Box::new(record),
            })
        })
        .map_err(|error| format!("stdin line {line_number}: {error}"))?;
    target
        .validate()
        .map_err(|error| format!("stdin line {line_number}: {error}"))?;
    Ok(target)
}

fn serve_command(app: Arc<ComsatApp>) -> CommandDef {
    CommandDef::build("serve", ServeCommand { app })
        .description("Run COMSAT HTTP and watch scheduler")
        .done()
}

#[derive(Debug, Deserialize, incurs::Options)]
struct ServeOptions {
    addr: Option<String>,
}

struct ServeCommand {
    app: Arc<ComsatApp>,
}

#[async_trait::async_trait]
impl CommandHandler for ServeCommand {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let options = match serde_json::from_value::<ServeOptions>(ctx.options) {
            Ok(options) => options,
            Err(error) => return serve_error("invalid_query", error, false, 2),
        };
        let address = match serve_address(options.addr) {
            Ok(address) => address,
            Err(error) => return serve_error("invalid_query", error, false, 2),
        };
        let http_cli = build_cli(Arc::clone(&self.app));
        enable_serve_logging();
        match serve_native(Arc::clone(&self.app), &http_cli, address).await {
            Ok(()) => CommandResult::Ok {
                data: serde_json::json!({ "listening": address.to_string() }),
                cta: None,
                exit_code: None,
            },
            Err(error) => serve_error("operational", error, true, 1),
        }
    }
}

fn serve_error(
    code: impl Into<String>,
    error: impl std::fmt::Display,
    retryable: bool,
    exit_code: i32,
) -> CommandResult {
    CommandResult::Error {
        code: code.into(),
        message: error.to_string(),
        retryable,
        exit_code: Some(exit_code),
        cta: None,
    }
}

async fn serve_native(
    app: Arc<ComsatApp>,
    cli: &Cli,
    address: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let scheduler = watch_scheduler(app);
    tokio::pin!(scheduler);
    tokio::select! {
        result = incurs::http::serve_http(cli, address) => result,
        result = &mut scheduler => result,
    }
}

async fn watch_scheduler(app: Arc<ComsatApp>) -> Result<(), Box<dyn std::error::Error>> {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        match app.run_due_watch_once().await {
            Ok(Some(outcome)) => eprintln!(
                "{}",
                serde_json::json!({
                    "event": "watch_scheduler_run",
                    "status": outcome.status,
                    "run_id": outcome.run_id,
                    "records_seen": outcome.records_seen,
                    "records_inserted": outcome.records_inserted,
                    "watch_records_inserted": outcome.watch_records_inserted
                })
            ),
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "event": "watch_scheduler_run",
                        "status": "failed",
                        "message": error.to_string()
                    })
                );
            }
        }
        log_catalog_metrics(&app);
        notifications::deliver_pending(&app).await;
    }
}

fn log_catalog_metrics(app: &ComsatApp) {
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "source_metrics",
            "process_id": std::process::id(),
            "cumulative": app.catalog().metrics()
        })
    );
}

fn native_clock() -> Arc<dyn Fn() -> i64 + Send + Sync> {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .try_into()
            .unwrap_or(i64::MAX)
    })
}

fn serve_address(address: Option<String>) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    address
        .unwrap_or_else(|| "127.0.0.1:8737".to_string())
        .parse()
        .map_err(Into::into)
}
