use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

#[test]
fn source_list_does_not_create_native_database() {
    let data_dir = temp_dir("source-list-no-db");

    let output = comsat(&data_dir)
        .args(["source", "list"])
        .output()
        .expect("source list should run");

    assert_success(&output);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("fixture-source"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !data_dir.join("comsat.sqlite3").exists(),
        "source list must not create the native sqlite database"
    );
}

#[test]
fn mcp_stdio_initialize_and_tools_list_respond() {
    let data_dir = temp_dir("mcp-stdio");
    let mut child = comsat_without_plugins(&data_dir)
        .arg("--mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mcp server should spawn");

    let stdout = child.stdout.take().expect("stdout should be piped");
    let (stdout_tx, stdout_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let _ =
                stdout_tx.send(line.unwrap_or_else(|error| format!("stdout read error: {error}")));
        }
    });

    let stderr = child.stderr.take().expect("stderr should be piped");
    let (stderr_tx, stderr_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            text.push_str(&line);
            text.push('\n');
        }
        let _ = stderr_tx.send(text);
    });

    let stdin = child.stdin.as_mut().expect("stdin should be piped");
    write_json_rpc(
        stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "comsat-cli-test", "version": "0.0.0" }
            }
        }),
    );
    let initialized = wait_for_json_id(&stdout_rx, 1, Duration::from_secs(5), &mut child);
    assert_eq!(initialized["result"]["serverInfo"]["name"], "comsat");

    let stdin = child.stdin.as_mut().expect("stdin should remain piped");
    write_json_rpc(
        stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    write_json_rpc(
        stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    let listed = wait_for_json_id(&stdout_rx, 2, Duration::from_secs(5), &mut child);
    let has_search_tools = listed["result"]["tools"]
        .as_array()
        .expect("tools/list should return tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .any(|name| name == "search_tools");
    assert!(has_search_tools, "tools: {listed}");

    stop_child(child);
    let _ = stderr_rx.recv_timeout(Duration::from_millis(100));
}

#[test]
fn codemode_fixture_search_fetch_follow_uses_production_policy() {
    let data_dir = temp_dir("codemode-fixture");
    let code = "const found = await comsat.comsat_search({ text: 'pinned runtime', source: ['fixture-source'], limit: 1 }); \
        const fetched = await comsat.comsat_fetch({ type: 'record', record: found[0] }); \
        const followed = await comsat.comsat_follow({ type: 'record', record: found[0] }); \
        const inspected = await comsat.source_inspect({ source: 'fixture-source' }); \
        const watches = await comsat.watch_list({}); \
        const history = await comsat.history({}); \
        return { status: 'ok', found: found[0].id, fetched: fetched.id, followed: followed[0].id, inspected: inspected.source.id, watchCount: watches.watches.length, historyCount: history.length };";

    let output = comsat(&data_dir)
        .args(["code", "run", code])
        .output()
        .expect("code run should execute");

    assert_success(&output);
    let state = first_json_line(&output.stdout);
    assert_eq!(state["status"], "completed", "{state}");
    assert_eq!(state["result"]["status"], "ok", "{state}");
    assert_eq!(state["result"]["found"], "fixture:search", "{state}");
    assert_eq!(state["result"]["fetched"], "fixture:fetch", "{state}");
    assert_eq!(state["result"]["followed"], "fixture:follow", "{state}");
    assert_eq!(state["result"]["inspected"], "fixture-source", "{state}");
    assert_eq!(state["result"]["watchCount"], 0, "{state}");
    assert_eq!(state["result"]["historyCount"], 0, "{state}");
}

#[test]
fn codemode_paused_execution_exits_nonzero_with_state() {
    let data_dir = temp_dir("codemode-paused");
    let code = "await comsat.watch_add({ query: 'pinned runtime', source: ['fixture-source'], interval_seconds: 60 });";

    let output = comsat(&data_dir)
        .args(["code", "run", code])
        .output()
        .expect("code run should execute");

    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout:
{}
stderr:
{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let state = first_json_line(&output.stdout);
    assert_eq!(state["status"], "paused", "{state}");
    assert_eq!(state["result"], serde_json::Value::Null, "{state}");
    assert!(
        state["log"].as_array().is_some_and(|log| !log.is_empty()),
        "{state}"
    );
}

#[test]
fn stdin_record_targets_feed_fetch_and_follow() {
    let data_dir = temp_dir("stdin-targets");
    let search = comsat(&data_dir)
        .args([
            "search",
            "pinned runtime",
            "--source",
            "fixture-source",
            "--limit",
            "1",
        ])
        .output()
        .expect("search should run");
    assert_success(&search);

    let record = first_json_line(&search.stdout);
    assert_eq!(record["source"], "fixture-source");
    let line = format!("{record}\n");

    let fetch = run_with_stdin(&data_dir, &["fetch"], &line);
    assert_success(&fetch);
    let fetched = first_json_line(&fetch.stdout);
    assert_eq!(fetched["kind"], "document");
    assert_eq!(fetched["source"], "fixture-source");

    let follow = run_with_stdin(&data_dir, &["follow"], &line);
    assert_success(&follow);
    let followed = first_json_line(&follow.stdout);
    assert_eq!(followed["kind"], "comment");
    assert_eq!(followed["source"], "fixture-source");
}

fn comsat(data_dir: &Path) -> Command {
    let mut command = comsat_without_plugins(data_dir);
    command.env("COMSAT_PLUGINS", fixture_plugin_root());
    command
}

fn comsat_without_plugins(data_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_comsat"));
    command
        .env("COMSAT_DATA_DIR", data_dir)
        .env_remove("COMSAT_PLUGINS")
        .env_remove("XDG_DATA_HOME");
    command
}

fn write_json_rpc(stdin: &mut impl Write, value: &Value) {
    writeln!(stdin, "{value}").expect("json-rpc message should write");
    stdin.flush().expect("json-rpc message should flush");
}

fn wait_for_json_id(
    stdout: &Receiver<String>,
    id: i64,
    timeout: Duration,
    child: &mut Child,
) -> Value {
    let deadline = Instant::now() + timeout;
    let mut lines = Vec::new();
    loop {
        let now = Instant::now();
        if now >= deadline {
            panic_with_child_output(child, &lines, id);
        }
        let remaining = deadline.saturating_duration_since(now);
        match stdout.recv_timeout(remaining.min(Duration::from_millis(250))) {
            Ok(line) => {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && value["id"] == id
                {
                    return value;
                }
                lines.push(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if child
                    .try_wait()
                    .expect("mcp process should be observable")
                    .is_some()
                {
                    panic_with_child_output(child, &lines, id);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => panic_with_child_output(child, &lines, id),
        }
    }
}

fn panic_with_child_output(child: &mut Child, lines: &[String], id: i64) -> ! {
    let _ = child.kill();
    let status = child.wait().ok();
    panic!("missing MCP response id {id}; status: {status:?}; stdout lines: {lines:?}");
}

fn stop_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn run_with_stdin(data_dir: &Path, args: &[&str], input: &str) -> Output {
    let mut child = comsat(data_dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("command should spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin should be piped")
        .write_all(input.as_bytes())
        .expect("stdin should accept target");
    child.wait_with_output().expect("command should complete")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn first_json_line(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_else(|| panic!("missing JSONL output: {text}"));
    serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}: {line}"))
}

fn fixture_plugin_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/external-source")
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("comsat-cli-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).expect("temp dir should be created");
    path
}
