use std::path::Path;
use std::process::{Command, ExitStatus};

use crate::{Result, architecture, cloud_test, complexity, conformance, native_test, wasm};

pub fn run(root: &Path) -> Result<()> {
    for step in command_steps().into_iter().chain(function_steps()) {
        step.run(root)?;
    }
    Ok(())
}

const fn command_steps() -> [Step; 5] {
    [
        command_step("cargo fmt --check", "cargo", &["fmt", "--check"]),
        command_step(
            "cargo clippy --workspace --all-targets --all-features",
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ],
        ),
        command_step(
            "cargo test --workspace --all-features",
            "cargo",
            &["test", "--workspace", "--all-features"],
        ),
        command_step("cargo deny check", "cargo", &["deny", "check"]),
        command_step(
            "cargo-machete --skip-target-dir",
            "cargo-machete",
            &["--skip-target-dir"],
        ),
    ]
}

fn function_steps() -> [Step; 6] {
    [
        function_step("cargo xtask architecture", architecture::run),
        function_step("cargo xtask complexity", complexity::run),
        function_step("cargo xtask conformance", conformance::run),
        function_step("cargo xtask wasm", wasm::run),
        function_step("cargo xtask native-test", native_test::run),
        function_step("cargo xtask cloud-test", cloud_test::run),
    ]
}

const fn command_step(
    name: &'static str,
    program: &'static str,
    args: &'static [&'static str],
) -> Step {
    Step::Command {
        name,
        program,
        args,
    }
}

const fn function_step(name: &'static str, run: fn(&Path) -> Result<()>) -> Step {
    Step::Function { name, run }
}

enum Step {
    Command {
        name: &'static str,
        program: &'static str,
        args: &'static [&'static str],
    },
    Function {
        name: &'static str,
        run: fn(&Path) -> Result<()>,
    },
}

impl Step {
    fn run(self, root: &Path) -> Result<()> {
        match self {
            Self::Command {
                name,
                program,
                args,
            } => run_command(root, name, program, args),
            Self::Function { name, run } => {
                eprintln!("==> {name}");
                run(root)
            }
        }
    }
}

fn run_command(root: &Path, name: &str, program: &str, args: &[&str]) -> Result<()> {
    eprintln!("==> {name}");
    let status = Command::new(program).args(args).current_dir(root).status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(command_error(name, status).into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(format!(
            "`{name}` could not run because `{program}` is missing. Install the tool and rerun `cargo xtask check`."
        )
        .into()),
        Err(error) => Err(format!("`{name}` failed to start: {error}").into()),
    }
}

fn command_error(name: &str, status: ExitStatus) -> String {
    status.code().map_or_else(
        || format!("`{name}` terminated without an exit code"),
        |code| format!("`{name}` exited with status {code}"),
    )
}
