use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::Result;
use crate::cloud_test::TestTokens;
use crate::cloud_test_support::log_tail;

const WRANGLER_PACKAGE: &str = "wrangler@4.131.1";
const WORKER_BUILD_VERSION: &str = "0.8.5";
pub const ALLOWED_ORIGIN: &str = "http://localhost:3000";

pub struct CloudTestProject {
    config: PathBuf,
    persist: PathBuf,
    log: PathBuf,
    cargo_tools: PathBuf,
}

impl CloudTestProject {
    pub fn write(root: &Path, temp: &Path, tokens: &TestTokens) -> Result<Self> {
        let project = Self::new(root, temp);
        fs::create_dir_all(&project.persist)?;
        fs::write(&project.config, wrangler_config(root))?;
        fs::write(temp.join(".dev.vars"), dev_vars(tokens))?;
        Ok(project)
    }

    pub fn log_tail(&self) -> String {
        log_tail(&self.log)
    }

    pub fn build_worker(&self, root: &Path) -> Result<()> {
        let status = Command::new(self.cargo_tools.join("bin").join("worker-build"))
            .args(["--release", "crates/comsat-cloud"])
            .current_dir(root)
            .env("CARGO_PROFILE_RELEASE_STRIP", "debuginfo")
            .env("PATH", tool_path(self))
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Worker release build exited with {status}").into())
        }
    }

    pub fn ensure_worker_build(&self) -> Result<()> {
        if self.installed_worker_build_matches()? {
            return Ok(());
        }
        let status = Command::new("cargo")
            .args([
                "install",
                "worker-build",
                "--version",
                WORKER_BUILD_VERSION,
                "--locked",
                "--root",
            ])
            .arg(&self.cargo_tools)
            .status()
            .map_err(|error| worker_build_start_error(&error))?;
        if status.success() {
            Ok(())
        } else {
            Err(
                format!("cargo install worker-build {WORKER_BUILD_VERSION} exited with {status}")
                    .into(),
            )
        }
    }

    fn new(root: &Path, temp: &Path) -> Self {
        Self {
            config: temp.join("wrangler.jsonc"),
            persist: temp.join("wrangler-state"),
            log: temp.join("wrangler-dev.log"),
            cargo_tools: root
                .join("target")
                .join("xtask-tools")
                .join(format!("worker-build-{WORKER_BUILD_VERSION}")),
        }
    }

    fn installed_worker_build_matches(&self) -> Result<bool> {
        let binary = self.cargo_tools.join("bin").join("worker-build");
        if !binary.exists() {
            return Ok(false);
        }
        let output = Command::new(binary).arg("--version").output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(output.status.success() && stdout.contains(WORKER_BUILD_VERSION))
    }
}

pub fn init_d1(root: &Path, project: &CloudTestProject) -> Result<()> {
    run_wrangler(
        root,
        project,
        &["d1", "migrations", "apply", "comsat", "--local"],
        &[],
    )
}

pub fn query_d1(root: &Path, project: &CloudTestProject, query: &str) -> Result<Value> {
    let output = run_wrangler_capture(
        root,
        project,
        &[
            "d1",
            "execute",
            "comsat",
            "--local",
            "--json",
            "--command",
            query,
        ],
        &[],
    )?;
    parse_wrangler_json(&output)
}

pub fn start_worker(root: &Path, project: &CloudTestProject, port: u16) -> Result<WorkerProcess> {
    let mut worker = WorkerProcess::start(root, project, port)?;
    wait_for_worker(port, &mut worker)?;
    Ok(worker)
}

pub struct WorkerProcess {
    child: Child,
    log: PathBuf,
}

impl WorkerProcess {
    fn start(root: &Path, project: &CloudTestProject, port: u16) -> Result<Self> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&project.log)?;
        let child = wrangler_command(root, project)
            .args([
                "dev",
                "--local",
                "--test-scheduled",
                "--ip",
                "127.0.0.1",
                "--port",
            ])
            .arg(port.to_string())
            .args(common_wrangler_args(project))
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()
            .map_err(|error| format!("failed to start pinned Wrangler dev server: {error}"))?;
        Ok(Self {
            child,
            log: project.log.clone(),
        })
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for_worker(port: u16, worker: &mut WorkerProcess) -> Result<()> {
    for _ in 0..120 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        if let Some(status) = worker.child.try_wait()? {
            return Err(format!(
                "Wrangler dev server exited before accepting connections with {status}. Log tail:\n{}",
                log_tail(&worker.log)
            )
            .into());
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(format!(
        "Wrangler dev server did not accept connections within 60 seconds. Log tail:\n{}",
        log_tail(&worker.log)
    )
    .into())
}

fn run_wrangler(
    root: &Path,
    project: &CloudTestProject,
    args: &[&str],
    paths: &[&Path],
) -> Result<()> {
    let output = run_wrangler_capture(root, project, args, paths)?;
    drop(output);
    Ok(())
}

fn run_wrangler_capture(
    root: &Path,
    project: &CloudTestProject,
    args: &[&str],
    paths: &[&Path],
) -> Result<String> {
    let output = wrangler_command(root, project)
        .args(args)
        .args(paths.iter().map(|path| path.as_os_str()))
        .args(common_wrangler_args(project))
        .output()
        .map_err(|error| format!("failed to run pinned Wrangler: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(wrangler_failure(args, &output.stdout, &output.stderr).into())
}

fn wrangler_command(root: &Path, project: &CloudTestProject) -> Command {
    let mut command = Command::new("npx");
    command
        .arg("--yes")
        .arg(WRANGLER_PACKAGE)
        .current_dir(project.config.parent().unwrap_or(root))
        .env("PATH", tool_path(project));
    command
}

fn wrangler_config(root: &Path) -> String {
    let main = root.join("crates/comsat-cloud/build/worker/shim.mjs");
    let migrations = root.join("migrations");
    format!(
        r#"{{
  "$schema": "node_modules/wrangler/config-schema.json",
  "name": "comsat-cloud-test",
  "main": "{}",
  "compatibility_date": "2026-09-13",
  "vars": {{
    "MCP_ALLOWED_ORIGINS": "{}",
    "WATCH_LEASE_SECONDS": "1",
    "WATCH_CLAIM_LIMIT": "25"
  }},
  "d1_databases": [{{
    "binding": "COMSAT_DB",
    "database_name": "comsat",
    "database_id": "00000000-0000-0000-0000-000000000000",
    "migrations_dir": "{}"
  }}],
  "queues": {{
    "producers": [{{"binding": "COMSAT_WATCH_QUEUE", "queue": "comsat-watch-runs"}}],
    "consumers": [{{"queue": "comsat-watch-runs", "max_batch_size": 5}}]
  }},
  "triggers": {{"crons": ["*/5 * * * *"]}}
}}
"#,
        json_escape_path(&main),
        ALLOWED_ORIGIN,
        json_escape_path(&migrations),
    )
}

fn dev_vars(tokens: &TestTokens) -> String {
    let token_map = json!({&tokens.tenant_a: "tenant-a", &tokens.tenant_b: "tenant-b"});
    // Failed-source fixture watches produce no observations and must never enqueue a webhook.
    let webhooks = json!({"tenant-a": {"url": "https://notifications.invalid/comsat"}});
    format!("TENANT_TOKENS_JSON='{token_map}'\nTENANT_WEBHOOKS_JSON='{webhooks}'\n")
}

fn tool_path(project: &CloudTestProject) -> String {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let bin = project.cargo_tools.join("bin");
    format!("{}:{}", bin.to_string_lossy(), current.to_string_lossy())
}

fn common_wrangler_args(project: &CloudTestProject) -> Vec<OsString> {
    vec![
        "--config".into(),
        project.config.clone().into_os_string(),
        "--persist-to".into(),
        project.persist.clone().into_os_string(),
    ]
}

fn parse_wrangler_json(output: &str) -> Result<Value> {
    let trimmed = output.trim();
    if let Ok(value) = serde_json::from_str(trimmed) {
        return Ok(value);
    }
    let start = trimmed
        .find(['[', '{'])
        .ok_or("Wrangler did not emit JSON output")?;
    Ok(serde_json::from_str(&trimmed[start..])?)
}

fn worker_build_start_error(error: &std::io::Error) -> String {
    format!("failed to install worker-build {WORKER_BUILD_VERSION}: {error}")
}

fn wrangler_failure(args: &[&str], stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    format!(
        "pinned Wrangler command `{}` failed. stdout: {stdout}; stderr: {stderr}",
        args.join(" ")
    )
}

fn json_escape_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}
