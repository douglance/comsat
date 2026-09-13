#![forbid(unsafe_code)]
#![allow(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "xtask is a command-line quality gate and reports directly to the terminal."
)]

mod architecture;
mod checks;
mod cloud_test;
mod cloud_test_http;
mod cloud_test_project;
mod cloud_test_support;
mod complexity;
mod conformance;
mod manifest;
mod native_test;
mod native_test_process;
mod wasm;

use std::env;
use std::error::Error;
use std::path::PathBuf;

type DynError = Box<dyn Error>;
type Result<T> = std::result::Result<T, DynError>;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let root = env::current_dir()?;
    let command = env::args().nth(1).unwrap_or_else(|| "help".to_owned());

    if let Some((_, run)) = command_table().iter().find(|(name, _)| *name == command) {
        return run(&root);
    }

    if matches!(command.as_str(), "help" | "-h" | "--help") {
        print_help();
        return Ok(());
    }

    Err(format!("unknown xtask command `{command}`").into())
}

type CommandHandler = fn(&std::path::Path) -> Result<()>;

fn command_table() -> &'static [(&'static str, CommandHandler)] {
    &[
        ("architecture", architecture::run),
        ("check", checks::run),
        ("cloud-test", cloud_test::run),
        ("complexity", complexity::run),
        ("native-test", native_test::run),
        ("conformance", conformance::run),
        ("wasm", wasm::run),
    ]
}

fn print_help() {
    println!("usage: cargo xtask <command>\n\ncommands:");
    for (name, _) in command_table() {
        println!("  {name}");
    }
}

fn display_path(root: &std::path::Path, path: &std::path::Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}
