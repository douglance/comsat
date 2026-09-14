//! External Agent Plugin cancellation.
//!
//! A source plugin is a separate process. Cancelling the work that started it
//! must tear that process down; otherwise a cancelled search leaks a running
//! program that still holds upstream credentials and quota.

use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::Result;
use crate::native_test::NativeEnv;

/// Search text the fixture plugin never answers.
const HANG_QUERY: &str = "hang-until-cancelled";
/// Substring identifying the fixture plugin process.
const PLUGIN_PROCESS: &str = "comsat-fixture-source";
const APPEAR_TIMEOUT: Duration = Duration::from_secs(120);
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn assert_plugin_process_cancellation(env: &NativeEnv) -> Result<()> {
    let mut search = Command::new(env.binary())
        .args(["search", HANG_QUERY, "--source", "fixture-source"])
        .env("COMSAT_DATA_DIR", env.data_dir())
        .env("COMSAT_PLUGINS", env.plugin_root())
        .env("COMSAT_TENANT", "native-test")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let started = wait_for(APPEAR_TIMEOUT, || Ok(!plugin_pids()?.is_empty()))?;
    if !started {
        let _ = search.kill();
        return Err("external plugin process never started for the hanging search".into());
    }

    search.kill()?;
    search.wait()?;

    let stopped = wait_for(EXIT_TIMEOUT, || Ok(plugin_pids()?.is_empty()))?;
    if !stopped {
        for pid in plugin_pids()? {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
        }
        return Err("external plugin process outlived the cancelled search".into());
    }
    Ok(())
}

fn wait_for(timeout: Duration, mut condition: impl FnMut() -> Result<bool>) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition()? {
            return Ok(true);
        }
        sleep(Duration::from_millis(250));
    }
    condition()
}

/// Process ids whose argument vector names the fixture plugin binary. `args` is
/// used rather than the command name because Linux truncates the latter.
fn plugin_pids() -> Result<Vec<u32>> {
    let output = Command::new("ps").args(["-eo", "pid=,args="]).output()?;
    let listing = String::from_utf8_lossy(&output.stdout);
    Ok(listing
        .lines()
        .filter(|line| line.contains(PLUGIN_PROCESS) && !line.contains("cargo"))
        .filter_map(|line| line.split_whitespace().next())
        .filter_map(|pid| pid.parse().ok())
        .collect())
}
