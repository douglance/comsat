//! Durable Code Mode lifecycle, exercised through separate CLI processes so the
//! evidence is cross-process persistence rather than in-memory state.

use serde_json::Value;

use crate::Result;
use crate::native_test::NativeEnv;

const WATCH_ID: &str = "codemode-lifecycle";

pub fn assert_durable_lifecycle(env: &NativeEnv) -> Result<()> {
    assert_reads_create_nothing(env)?;
    let completed = assert_run_persists(env)?;
    assert_visible_from_another_process(env, &completed)?;
    let paused = assert_pause_persists(env)?;
    assert_approval_applies_and_rollback_compensates(env, &paused)?;
    assert_prune_keeps_newest(env)
}

/// Inspecting Code Mode on a machine that has never run a program must not
/// create the database.
fn assert_reads_create_nothing(env: &NativeEnv) -> Result<()> {
    let listed = env.comsat(&["code", "list"], "")?;
    listed.expect_code(0, "code list before any run")?;
    if env.data_dir().join("comsat.sqlite3").exists() {
        return Err("`code list` created the database".into());
    }
    if json(&listed.stdout)? != Value::Array(Vec::new()) {
        return Err("`code list` reported executions before any run".into());
    }
    Ok(())
}

fn assert_run_persists(env: &NativeEnv) -> Result<String> {
    let run = env.comsat(
        &[
            "code",
            "run",
            "const found = await comsat.comsat_search({text: 'durable', source: ['fixture-source'], limit: 1}); return found.length;",
        ],
        "",
    )?;
    run.expect_code(0, "code run")?;
    let state = json(&run.stdout)?;
    expect_status(&state, "completed", "code run")?;
    if state["result"] != 1 {
        return Err(format!("code run returned {}", state["result"]).into());
    }
    execution_id(&state)
}

fn assert_visible_from_another_process(env: &NativeEnv, execution: &str) -> Result<()> {
    let listed = env.comsat(&["code", "list"], "")?;
    listed.expect_code(0, "code list")?;
    let executions = json(&listed.stdout)?;
    let found = executions
        .as_array()
        .into_iter()
        .flatten()
        .any(|state| state["id"] == *execution);
    if !found {
        return Err(format!("execution `{execution}` was not durable across processes").into());
    }

    let shown = env.comsat(&["code", "show", execution], "")?;
    shown.expect_code(0, "code show")?;
    expect_status(&json(&shown.stdout)?, "completed", "code show")?;

    let events = env.comsat(&["code", "events", execution], "")?;
    events.expect_code(0, "code events")?;
    if json(&events.stdout)?.as_array().is_none_or(Vec::is_empty) {
        return Err("code events returned no retained events".into());
    }
    Ok(())
}

/// A mutating tool pauses for approval, and the pause outlives the process.
fn assert_pause_persists(env: &NativeEnv) -> Result<String> {
    let run = env.comsat(
        &[
            "code",
            "run",
            &format!(
                "return await comsat.watch_add({{watch_id: '{WATCH_ID}', query: 'durable', interval_seconds: 3600, source: ['fixture-source']}});"
            ),
        ],
        "",
    )?;
    run.expect_code(1, "paused execution exits non-zero")?;
    let execution = execution_id(&json(&run.stdout)?)?;

    let shown = env.comsat(&["code", "show", &execution], "")?;
    shown.expect_code(0, "code show paused")?;
    expect_status(&json(&shown.stdout)?, "paused", "paused execution")?;
    Ok(execution)
}

fn assert_approval_applies_and_rollback_compensates(
    env: &NativeEnv,
    execution: &str,
) -> Result<()> {
    let approved = env.comsat(&["code", "approve", execution, "0"], "")?;
    approved.expect_code(0, "code approve")?;
    expect_status(&json(&approved.stdout)?, "completed", "approved execution")?;
    if !watch_exists(env)? {
        return Err("approval did not apply the watch".into());
    }

    let rolled_back = env.comsat(&["code", "rollback", execution], "")?;
    rolled_back.expect_code(0, "code rollback")?;
    expect_status(&json(&rolled_back.stdout)?, "rolled_back", "rollback")?;
    if watch_exists(env)? {
        return Err("rollback did not compensate the applied watch".into());
    }
    Ok(())
}

fn assert_prune_keeps_newest(env: &NativeEnv) -> Result<()> {
    let pruned = env.comsat(&["code", "prune", "1"], "")?;
    pruned.expect_code(0, "code prune")?;
    let listed = env.comsat(&["code", "list"], "")?;
    listed.expect_code(0, "code list after prune")?;
    let remaining = json(&listed.stdout)?.as_array().map_or(0, Vec::len);
    if remaining != 1 {
        return Err(format!("prune kept {remaining} executions, expected 1").into());
    }
    Ok(())
}

fn watch_exists(env: &NativeEnv) -> Result<bool> {
    let listed = env.comsat(&["watch", "list"], "")?;
    listed.expect_code(0, "watch list")?;
    Ok(json(&listed.stdout)?["watches"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|watch| watch["watch_id"] == *WATCH_ID))
}

fn execution_id(state: &Value) -> Result<String> {
    state["id"]
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "execution state had no id".into())
}

fn expect_status(state: &Value, expected: &str, label: &str) -> Result<()> {
    if state["status"] == *expected {
        return Ok(());
    }
    Err(format!(
        "{label} status was {}, expected {expected}",
        state["status"]
    )
    .into())
}

fn json(stdout: &str) -> Result<Value> {
    serde_json::from_str(stdout.trim()).map_err(Into::into)
}
