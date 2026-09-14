//! Durable Code Mode lifecycle commands.
//!
//! Executions outlive the process that started them, so a program paused for
//! approval in one invocation is inspected, approved, replayed, rolled back, or
//! deleted from another.
#![allow(
    clippy::future_not_send,
    reason = "the Code Mode runtime is single-threaded; run_local keeps these futures on one blocking thread"
)]

use std::sync::Arc;

use comsat_app::ComsatApp;
use incurs::cli::Cli;
use incurs::command::{CommandDef, TypedResult};
use incurs_codemode::CodeMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{code_mode, run_local};
use crate::codemode_store::LazyCodeModeStore;

pub fn register(cli: Cli, app: &Arc<ComsatApp>, store: &Arc<LazyCodeModeStore>) -> Cli {
    let cli = register_inspection(cli, app, store);
    register_control(cli, app, store)
}

/// Commands that only read durable state.
fn register_inspection(cli: Cli, app: &Arc<ComsatApp>, store: &Arc<LazyCodeModeStore>) -> Cli {
    cli.command(
        "list",
        command(app, store, "list", "List durable executions", list),
    )
    .command(
        "show",
        command(app, store, "show", "Show one execution", show),
    )
    .command(
        "events",
        command(app, store, "events", "Show retained events", events),
    )
    .command(
        "artifact",
        command(app, store, "artifact", "Read one artifact", artifact),
    )
}

/// Commands that advance or discard durable state.
fn register_control(cli: Cli, app: &Arc<ComsatApp>, store: &Arc<LazyCodeModeStore>) -> Cli {
    cli.command(
        "approve",
        command(app, store, "approve", "Approve a pending action", approve),
    )
    .command(
        "reject",
        command(app, store, "reject", "Reject a pending action", reject),
    )
    .command(
        "resume",
        command(app, store, "resume", "Resume a paused execution", resume),
    )
    .command(
        "cancel",
        command(app, store, "cancel", "Cancel an execution", cancel),
    )
    .command(
        "rollback",
        command(
            app,
            store,
            "rollback",
            "Compensate applied actions",
            rollback,
        ),
    )
    .command(
        "prune",
        command(
            app,
            store,
            "prune",
            "Keep only the newest executions",
            prune,
        ),
    )
}

#[derive(Debug, Deserialize, incurs::Args)]
struct ExecutionArgs {
    execution: String,
}

#[derive(Debug, Deserialize, incurs::Args)]
struct ArtifactArgs {
    execution: String,
    artifact: String,
}

#[derive(Debug, Deserialize, incurs::Args)]
struct DecisionArgs {
    execution: String,
    seq: u64,
}

#[derive(Debug, Deserialize, incurs::Args)]
struct PruneArgs {
    keep: usize,
}

async fn list(code_mode: CodeMode, (): ()) -> Result<Value, String> {
    value(
        code_mode
            .runtime()
            .executions()
            .await
            .map_err(|error| error.to_string())?,
    )
}

async fn show(code_mode: CodeMode, args: ExecutionArgs) -> Result<Value, String> {
    value(code_mode.execution_snapshot(&args.execution).await?)
}

async fn events(code_mode: CodeMode, args: ExecutionArgs) -> Result<Value, String> {
    value(code_mode.events(&args.execution).await?)
}

async fn artifact(code_mode: CodeMode, args: ArtifactArgs) -> Result<Value, String> {
    code_mode.artifact(&args.execution, &args.artifact).await
}

async fn approve(code_mode: CodeMode, args: DecisionArgs) -> Result<Value, String> {
    value(code_mode.approve(&args.execution, args.seq).await?)
}

async fn reject(code_mode: CodeMode, args: DecisionArgs) -> Result<Value, String> {
    value(code_mode.reject(&args.execution, args.seq).await?)
}

async fn resume(code_mode: CodeMode, args: ExecutionArgs) -> Result<Value, String> {
    value(code_mode.resume(&args.execution).await?)
}

async fn cancel(code_mode: CodeMode, args: ExecutionArgs) -> Result<Value, String> {
    value(code_mode.cancel(&args.execution).await?)
}

async fn rollback(code_mode: CodeMode, args: ExecutionArgs) -> Result<Value, String> {
    value(code_mode.rollback(&args.execution).await?)
}

async fn prune(code_mode: CodeMode, args: PruneArgs) -> Result<Value, String> {
    let removed = code_mode
        .runtime()
        .prune(args.keep)
        .await
        .map_err(|error| error.to_string())?;
    Ok(serde_json::json!({"removed": removed, "kept": args.keep}))
}

fn command<Args, Handler, HandlerFuture>(
    app: &Arc<ComsatApp>,
    store: &Arc<LazyCodeModeStore>,
    name: &'static str,
    description: &'static str,
    handler: Handler,
) -> CommandDef
where
    Args: incurs::schema::IncurSchema + for<'de> Deserialize<'de> + Send + Sync + 'static,
    Handler: Fn(CodeMode, Args) -> HandlerFuture + Copy + Send + Sync + 'static,
    // The Code Mode runtime is not `Send`, so these futures stay on the blocking
    // thread `run_local` creates for them.
    HandlerFuture: std::future::Future<Output = Result<Value, String>> + 'static,
{
    let app = Arc::clone(app);
    let store = Arc::clone(store);
    CommandDef::typed::<Args, (), (), Value, _, _>(name, move |ctx| {
        let app = Arc::clone(&app);
        let store = Arc::clone(&store);
        let args = ctx.args;
        async move {
            match run_local(move || async move { handler(code_mode(&app, store), args).await })
                .await
            {
                Ok(result) => result,
                Err(error) => TypedResult::error("codemode_error", error),
            }
        }
    })
    .description(description)
    .done()
}

fn value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}
