use std::sync::Arc;

use comsat_app::{ComsatApp, build_cli};
use incurs::{
    cli::Cli,
    command::{CommandDef, TypedResult},
};
use incurs_codemode::{
    ArtifactStore, CodeMode, CodeModeRunOptions, ExecutionState, ExecutionStatus, IncurConnector,
    ReplayPolicy, RuntimeStore, ToolAnnotations, ToolOrigin, ToolPolicy, ToolPolicyResolver,
};
use incurs_codemode_local::LocalExecutor;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::codemode_store::{LazyCodeModeStore, store as codemode_store};
use crate::native_store::database_path;

mod connector;
mod lifecycle;

pub fn group(app: &Arc<ComsatApp>) -> Cli {
    let store = codemode_store(database_path());
    let cli = Cli::create("code")
        .description("Run durable Code Mode against COMSAT tools")
        .command("run", run_command(Arc::clone(app), Arc::clone(&store)))
        .command("tools", tools_command(Arc::clone(app), Arc::clone(&store)));
    lifecycle::register(cli, app, &store)
}

#[derive(Debug, Deserialize, incurs::Args)]
struct RunArgs {
    code: String,
}

#[derive(Debug, Deserialize, incurs::Args)]
struct ToolsArgs {
    query: String,
}

fn run_command(app: Arc<ComsatApp>, store: Arc<LazyCodeModeStore>) -> CommandDef {
    CommandDef::typed::<RunArgs, (), (), Value, _, _>("run", move |ctx| {
        let app = Arc::clone(&app);
        let store = Arc::clone(&store);
        let code = ctx.args.code;
        let (options, guard) = run_options(ctx.request);
        async move {
            let result = run_execution(move || async move {
                code_mode(&app, store).execute_with(&code, options).await
            })
            .await;
            drop(guard);
            match result {
                Ok(value) => value,
                Err(error) => TypedResult::error("codemode_error", error),
            }
        }
    })
    .description("Execute JavaScript Code Mode against COMSAT tools")
    .done()
}

fn tools_command(app: Arc<ComsatApp>, store: Arc<LazyCodeModeStore>) -> CommandDef {
    CommandDef::typed::<ToolsArgs, (), (), Value, _, _>("tools", move |ctx| {
        let app = Arc::clone(&app);
        let store = Arc::clone(&store);
        let query = ctx.args.query;
        async move {
            match run_local(move || async move { code_mode(&app, store).search(&query).await })
                .await
            {
                Ok(value) => value,
                Err(error) => TypedResult::error("codemode_error", error),
            }
        }
    })
    .description("Search Code Mode COMSAT tool capabilities")
    .done()
}

fn run_options(
    request: Option<incurs::command::RequestContext>,
) -> (CodeModeRunOptions, DropGuard) {
    let cancellation = CancellationToken::new();
    let guard = cancellation.clone().drop_guard();
    (
        CodeModeRunOptions {
            cancellation,
            request,
        },
        guard,
    )
}

pub async fn run_local<MakeFuture, Future, Output>(
    make_future: MakeFuture,
) -> Result<TypedResult<Value>, String>
where
    MakeFuture: FnOnce() -> Future + Send + 'static,
    Future: std::future::Future<Output = Result<Output, String>>,
    Output: serde::Serialize,
{
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;

        runtime.block_on(async move {
            let output = make_future().await?;
            Ok(json_result(output))
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn run_execution<MakeFuture, Future>(
    make_future: MakeFuture,
) -> Result<TypedResult<Value>, String>
where
    MakeFuture: FnOnce() -> Future + Send + 'static,
    Future: std::future::Future<Output = Result<ExecutionState, String>>,
{
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;

        runtime.block_on(async move {
            let state = make_future().await?;
            Ok(execution_result(state))
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

fn execution_result(state: ExecutionState) -> TypedResult<Value> {
    let success = state.status == ExecutionStatus::Completed;
    match serde_json::to_value(state) {
        Ok(value) if success => TypedResult::ok(value),
        Ok(value) => TypedResult::ok_with_exit_code(value, 1),
        Err(error) => TypedResult::error("codemode_error", error.to_string()),
    }
}

fn json_result<T: serde::Serialize>(value: T) -> TypedResult<Value> {
    match serde_json::to_value(value) {
        Ok(value) => TypedResult::ok(value),
        Err(error) => TypedResult::error("codemode_error", error.to_string()),
    }
}

pub fn code_mode(app: &ComsatApp, store: Arc<LazyCodeModeStore>) -> CodeMode {
    let connector = connector::ComsatConnector::new(
        IncurConnector::new(
            build_cli(Arc::new(app.clone().without_target_stream_provider())).tool_catalog(),
        )
        .with_policy_resolver(Arc::new(ComsatRetrievalPolicy)),
    );
    CodeMode::with_artifact_store(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        store as Arc<dyn ArtifactStore>,
        LocalExecutor::default(),
        vec![Arc::new(connector)],
    )
}

#[derive(Debug)]
struct ComsatRetrievalPolicy;

impl ToolPolicyResolver for ComsatRetrievalPolicy {
    fn resolve(&self, origin: ToolOrigin, annotations: &ToolAnnotations) -> ToolPolicy {
        let read_only_local = origin == ToolOrigin::Local
            && annotations.read_only == Some(true)
            && annotations.destructive != Some(true);
        ToolPolicy {
            requires_approval: !read_only_local,
            replay: if read_only_local {
                ReplayPolicy::Reexecute
            } else {
                ReplayPolicy::Log
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ComsatRetrievalPolicy, ExecutionState, ExecutionStatus, ReplayPolicy, ToolAnnotations,
        ToolOrigin, ToolPolicy, ToolPolicyResolver, execution_result, run_options,
    };
    use serde_json::json;

    #[test]
    fn dropping_run_guard_cancels_codemode_token() {
        let (options, guard) = run_options(None);

        drop(guard);

        assert!(options.cancellation.is_cancelled());
    }

    #[test]
    fn retrieval_policy_allows_only_local_read_only_tools_without_approval() {
        let policy = ComsatRetrievalPolicy;
        let open_world_read = ToolAnnotations {
            read_only: Some(true),
            destructive: Some(false),
            open_world: Some(true),
            ..ToolAnnotations::default()
        };
        assert_eq!(
            policy.resolve(ToolOrigin::Local, &open_world_read),
            ToolPolicy {
                requires_approval: false,
                replay: ReplayPolicy::Reexecute,
            }
        );

        let mutating = ToolAnnotations {
            read_only: Some(false),
            destructive: Some(true),
            ..ToolAnnotations::default()
        };
        assert!(
            policy
                .resolve(ToolOrigin::Local, &mutating)
                .requires_approval
        );
        assert!(
            policy
                .resolve(ToolOrigin::RemoteMcp, &open_world_read)
                .requires_approval
        );
        assert!(
            policy
                .resolve(ToolOrigin::Local, &ToolAnnotations::default())
                .requires_approval
        );
    }

    #[test]
    fn paused_execution_result_exits_nonzero_with_state_payload() {
        let result = execution_result(ExecutionState {
            id: "paused-1".to_string(),
            code: "await comsat.watch_add({})".to_string(),
            status: ExecutionStatus::Paused,
            log: Vec::new(),
            result: None,
            error: None,
            logs: Vec::new(),
            connectors: vec!["comsat".to_string()],
            capabilities: None,
            events: Vec::new(),
            created_at: 1,
            updated_at: 1,
        });
        let super::TypedResult::Ok {
            data, exit_code, ..
        } = result
        else {
            panic!("paused execution should return state payload");
        };
        assert_eq!(exit_code, Some(1));
        assert_eq!(data["status"], json!("paused"));
        assert_eq!(data["result"], json!(null));
    }
}
