use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::Result;
use crate::cloud_test_support::TempDir;
use crate::native_test_codemode::assert_durable_lifecycle;
use crate::native_test_plugin::assert_plugin_process_cancellation;
use crate::native_test_process::{RunOutput, ServerProcess, run_comsat};

const FIXTURE_SOURCE: &str = "fixture-source";

pub fn run(root: &Path) -> Result<()> {
    build_comsat(root)?;
    let env = NativeEnv::new(root)?;

    // First, while the data directory still does not exist: Code Mode reads
    // must not create it.
    assert_durable_lifecycle(&env)?;
    assert_positional_search_jsonl(&env)?;
    assert_fetch_pipeline(&env)?;
    assert_follow_pipeline(&env)?;
    assert_invalid_invocation(&env)?;
    assert_partial_and_strict(&env)?;
    assert_self_host_watch_history(root, &env)?;
    assert_plugin_process_cancellation(&env)?;
    assert_source_contracts(&env)?;
    Ok(())
}

fn build_comsat(root: &Path) -> Result<()> {
    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "comsat-cli", "--bin", "comsat"])
        .current_dir(root)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo build -p comsat-cli --bin comsat exited with {status}").into())
    }
}

fn assert_positional_search_jsonl(env: &NativeEnv) -> Result<()> {
    let output = env.comsat(
        &[
            "search",
            "positional native",
            "--source",
            FIXTURE_SOURCE,
            "--limit",
            "1",
        ],
        "",
    )?;
    output.expect_code(0, "positional search")?;
    let records = parse_jsonl(&output.stdout)?;
    expect_record(&records, "search-result", Some("positional native"))?;
    expect_empty(&output.stderr, "positional search stderr")
}

fn assert_fetch_pipeline(env: &NativeEnv) -> Result<()> {
    let search = env.comsat(&["search", "fetch pipe", "--source", FIXTURE_SOURCE], "")?;
    search.expect_code(0, "search before fetch")?;
    let fetch = env.comsat(&["fetch"], &search.stdout)?;
    fetch.expect_code(0, "fetch pipeline")?;
    let records = parse_jsonl(&fetch.stdout)?;
    expect_record(&records, "document", Some("fixture:search"))
}

fn assert_follow_pipeline(env: &NativeEnv) -> Result<()> {
    let search = env.comsat(&["search", "follow pipe", "--source", FIXTURE_SOURCE], "")?;
    search.expect_code(0, "search before follow")?;
    let follow = env.comsat(&["follow"], &search.stdout)?;
    follow.expect_code(0, "follow pipeline")?;
    let records = parse_jsonl(&follow.stdout)?;
    expect_record(&records, "comment", Some("fixture:search"))
}

fn assert_invalid_invocation(env: &NativeEnv) -> Result<()> {
    let output = env.comsat(&["search"], "")?;
    output.expect_code(2, "invalid invocation")?;
    expect_empty(&output.stdout, "invalid invocation stdout")
}

fn assert_partial_and_strict(env: &NativeEnv) -> Result<()> {
    let args = [
        "search",
        "partial",
        "--source",
        FIXTURE_SOURCE,
        "--source",
        "web",
    ];
    let partial = env.comsat(&args, "")?;
    partial.expect_code(0, "partial search")?;
    expect_record(
        &parse_jsonl(&partial.stdout)?,
        "search-result",
        Some("partial"),
    )?;
    expect_contains(&partial.stderr, "authentication", "partial diagnostic")?;

    let strict = env.comsat(&[&args[..], &["--strict"]].concat(), "")?;
    strict.expect_code(3, "strict partial search")?;
    expect_record(
        &parse_jsonl(&strict.stdout)?,
        "search-result",
        Some("partial"),
    )?;
    expect_contains(&strict.stderr, "partial_failure", "strict diagnostic")
}

fn assert_self_host_watch_history(root: &Path, env: &NativeEnv) -> Result<()> {
    let watch = env.comsat(
        &[
            "watch",
            "add",
            "--watch-id",
            "native-fixture-watch",
            "--query",
            "self host fixture",
            "--source",
            FIXTURE_SOURCE,
            "--interval-seconds",
            "1",
            "--limit",
            "1",
        ],
        "",
    )?;
    watch.expect_code(0, "watch add")?;

    let server = ServerProcess::start(root, env)?;
    poll_history(env, "native-fixture-watch")?;
    drop(server);
    Ok(())
}

/// Every registered source, including the externally loaded plugin, must pass
/// the fixture-free part of source conformance: declared operations exist and
/// advertise internally consistent schemas.
fn assert_source_contracts(env: &NativeEnv) -> Result<()> {
    let output = env.comsat(&["source", "test"], "")?;
    output.expect_code(0, "source test")?;
    let report: Value = serde_json::from_str(output.stdout.trim())?;
    if report["failures"] != Value::Array(Vec::new()) {
        return Err(format!("source contract failures: {}", report["failures"]).into());
    }
    if report["sources_checked"].as_u64().unwrap_or_default() < 5 {
        return Err(format!(
            "source test checked {} sources, expected the plugin to be included",
            report["sources_checked"]
        )
        .into());
    }
    Ok(())
}

fn poll_history(env: &NativeEnv, watch_id: &str) -> Result<()> {
    for _ in 0..40 {
        let output = env.comsat(&["history", "--watch", watch_id, "--limit", "10"], "")?;
        output.expect_code(0, "history")?;
        if history_contains_fixture(&output.stdout)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(750));
    }
    Err(format!("history did not contain fixture record for watch `{watch_id}`").into())
}

fn history_contains_fixture(stdout: &str) -> Result<bool> {
    let records = parse_jsonl(stdout)?;
    Ok(records.iter().any(|record| {
        record.get("source").and_then(Value::as_str) == Some(FIXTURE_SOURCE)
            && record.get("kind").and_then(Value::as_str) == Some("search-result")
            && record.get("url").and_then(Value::as_str).is_some()
            && record.get("id").and_then(Value::as_str).is_some()
    }))
}

fn parse_jsonl(stdout: &str) -> Result<Vec<Value>> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

fn expect_record(records: &[Value], kind: &str, text: Option<&str>) -> Result<()> {
    let found = records.iter().any(|record| {
        record.get("source").and_then(Value::as_str) == Some(FIXTURE_SOURCE)
            && record.get("kind").and_then(Value::as_str) == Some(kind)
            && text_matches(record, text)
    });
    if found {
        Ok(())
    } else {
        Err(format!("expected fixture record kind `{kind}` with text {text:?}").into())
    }
}

fn text_matches(record: &Value, expected: Option<&str>) -> bool {
    expected.is_none_or(|text| record.get("text").and_then(Value::as_str) == Some(text))
}

fn expect_empty(value: &str, label: &str) -> Result<()> {
    if value.is_empty() {
        Ok(())
    } else {
        Err(format!("{label} was not empty: {value}").into())
    }
}

fn expect_contains(value: &str, needle: &str, label: &str) -> Result<()> {
    if value.contains(needle) {
        Ok(())
    } else {
        Err(format!("{label} did not contain `{needle}`: {value}").into())
    }
}

pub struct NativeEnv {
    _temp: TempDir,
    data_dir: PathBuf,
    plugin_root: PathBuf,
    binary: PathBuf,
}

impl NativeEnv {
    fn new(root: &Path) -> Result<Self> {
        let temp = TempDir::new("comsat-native-test")?;
        Ok(Self {
            data_dir: temp.path().join("data"),
            plugin_root: root.join("examples/external-source"),
            binary: root.join("target/debug/comsat"),
            _temp: temp,
        })
    }

    pub fn comsat(&self, args: &[&str], stdin: &str) -> Result<RunOutput> {
        run_comsat(self, args, stdin, Duration::from_secs(30))
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn plugin_root(&self) -> &Path {
        &self.plugin_root
    }
}
