//! The long-running COMSAT service: one HTTP surface over the shared command
//! graph plus the watch scheduler that advances due watches.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use comsat_store::WatchRunOutcome;
use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler};
use incurs::output::CommandResult;
use serde::Deserialize;

use crate::{ComsatApp, build_cli};

/// Default listen address for `comsat serve`.
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:8737";

const SCHEDULER_INTERVAL: Duration = Duration::from_secs(30);

/// Deployment-specific behavior a host attaches to the service: transport
/// logging on start, and whatever the host must do after each watch tick, such
/// as delivering queued notifications.
#[async_trait]
pub trait ServeHooks: Send + Sync {
    fn started(&self, address: SocketAddr) {
        let _ = address;
    }

    async fn after_watch_tick(&self, app: &ComsatApp) {
        let _ = app;
    }
}

/// Hooks for a host with nothing to attach.
pub struct NoServeHooks;

impl ServeHooks for NoServeHooks {}

/// The shared command graph plus `serve`. The HTTP surface that `serve` exposes
/// is [`build_cli`], so a served COMSAT offers retrieval and watches but never
/// another server.
pub fn build_serving_cli(app: Arc<ComsatApp>, hooks: Arc<dyn ServeHooks>) -> Cli {
    build_cli(Arc::clone(&app)).command("serve", serve_command(app, hooks))
}

fn serve_command(app: Arc<ComsatApp>, hooks: Arc<dyn ServeHooks>) -> CommandDef {
    CommandDef::build("serve", ServeCommand { app, hooks })
        .description("Run COMSAT HTTP and watch scheduler")
        .done()
}

#[derive(Debug, Deserialize, incurs::Options)]
struct ServeOptions {
    addr: Option<String>,
}

struct ServeCommand {
    app: Arc<ComsatApp>,
    hooks: Arc<dyn ServeHooks>,
}

#[async_trait]
impl CommandHandler for ServeCommand {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let options = match serde_json::from_value::<ServeOptions>(ctx.options) {
            Ok(options) => options,
            Err(error) => return serve_error("invalid_query", &error, false, 2),
        };
        let address = match parse_address(options.addr.as_deref()) {
            Ok(address) => address,
            Err(error) => return serve_error("invalid_query", &error, false, 2),
        };
        self.hooks.started(address);
        match self.run_service(address).await {
            Ok(()) => CommandResult::Ok {
                data: serde_json::json!({ "listening": address.to_string() }),
                cta: None,
                exit_code: None,
            },
            Err(error) => serve_error("operational", &error, true, 1),
        }
    }
}

impl ServeCommand {
    async fn run_service(&self, address: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        let http_cli = build_cli(Arc::clone(&self.app));
        let scheduler = self.run_scheduler();
        tokio::pin!(scheduler);
        tokio::select! {
            result = incurs::http::serve_http(&http_cli, address) => result,
            result = &mut scheduler => result,
        }
    }

    async fn run_scheduler(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut interval = tokio::time::interval(SCHEDULER_INTERVAL);
        loop {
            interval.tick().await;
            report_watch_tick(self.app.run_due_watch_once().await);
            self.hooks.after_watch_tick(&self.app).await;
        }
    }
}

fn report_watch_tick(outcome: crate::AppResult<Option<WatchRunOutcome>>) {
    let event = match outcome {
        Ok(None) => return,
        Ok(Some(outcome)) => serde_json::json!({
            "event": "watch_scheduler_run",
            "status": outcome.status,
            "run_id": outcome.run_id,
            "records_seen": outcome.records_seen,
            "records_inserted": outcome.records_inserted,
            "watch_records_inserted": outcome.watch_records_inserted
        }),
        Err(error) => serde_json::json!({
            "event": "watch_scheduler_run",
            "status": "failed",
            "message": error.to_string()
        }),
    };
    eprintln!("{event}");
}

fn parse_address(address: Option<&str>) -> Result<SocketAddr, std::net::AddrParseError> {
    address.unwrap_or(DEFAULT_ADDRESS).parse()
}

fn serve_error(
    code: &str,
    error: &dyn std::fmt::Display,
    retryable: bool,
    exit_code: i32,
) -> CommandResult {
    CommandResult::Error {
        code: code.to_owned(),
        message: error.to_string(),
        retryable,
        exit_code: Some(exit_code),
        cta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_ADDRESS, parse_address};

    #[test]
    fn defaults_to_the_documented_loopback_address() {
        assert_eq!(parse_address(None).unwrap().to_string(), DEFAULT_ADDRESS);
    }

    #[test]
    fn rejects_a_malformed_address() {
        assert!(parse_address(Some("not-an-address")).is_err());
    }
}
