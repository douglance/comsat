use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::Result;
use crate::native_test::NativeEnv;

pub struct RunOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutput {
    pub fn expect_code(&self, expected: i32, label: &str) -> Result<()> {
        if self.code == expected {
            Ok(())
        } else {
            Err(format!(
                "{label} exited {}, expected {expected}. stdout: {} stderr: {}",
                self.code, self.stdout, self.stderr
            )
            .into())
        }
    }
}

pub fn run_comsat(
    env: &NativeEnv,
    args: &[&str],
    stdin: &str,
    timeout: Duration,
) -> Result<RunOutput> {
    let mut child = base_command(env)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    write_stdin(&mut child, stdin)?;
    let output = wait_output(child, timeout)?;
    Ok(RunOutput {
        code: exit_code(output.status),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub struct ServerProcess {
    child: Child,
}

impl ServerProcess {
    pub fn start(root: &Path, env: &NativeEnv) -> Result<Self> {
        let log_path = env.data_dir().with_file_name("serve.log");
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        let mut child = base_command(env)
            .arg("serve")
            .current_dir(root)
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()?;
        wait_for_port(8737, &mut child, &log_path)?;
        Ok(Self { child })
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn base_command(env: &NativeEnv) -> Command {
    let mut command = Command::new(env.binary());
    command
        .env("COMSAT_DATA_DIR", env.data_dir())
        .env("COMSAT_PLUGINS", env.plugin_root())
        .env("COMSAT_TENANT", "native-test")
        .env_remove("COMSAT_BRAVE_API_KEY")
        .env_remove("BRAVE_API_KEY")
        .env_remove("COMSAT_LOG");
    command
}

fn write_stdin(child: &mut Child, stdin: &str) -> Result<()> {
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin.as_bytes())?;
    }
    Ok(())
}

fn wait_output(mut child: Child, timeout: Duration) -> Result<std::process::Output> {
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let output = child.wait_with_output()?;
            return Err(format!(
                "comsat command timed out after {:?}. stdout: {} stderr: {}",
                timeout,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_port(port: u16, child: &mut Child, log_path: &Path) -> Result<()> {
    for _ in 0..120 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!(
                "comsat serve exited before listening: {status}. Log tail:\n{}",
                crate::cloud_test_support::log_tail(log_path)
            )
            .into());
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(format!(
        "comsat serve did not listen within 60 seconds. Log tail:\n{}",
        crate::cloud_test_support::log_tail(log_path)
    )
    .into())
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}
