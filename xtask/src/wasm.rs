use std::path::Path;
use std::process::Command;

use crate::Result;
use crate::manifest;

const WASM_PACKAGES: &[&str] = &[
    "comsat-types",
    "comsat-source",
    "comsat-engine",
    "comsat-app",
    "comsat-store",
    "comsat-cloud",
    "comsat-github",
    "comsat-hacker-news",
    "comsat-web",
    "comsat-stack-exchange",
];

pub fn run(root: &Path) -> Result<()> {
    let workspace = manifest::read_workspace(root)?;
    let package_names = workspace.package_names();
    let mut checked = 0;

    for package in WASM_PACKAGES {
        if !package_names.contains(*package) {
            continue;
        }
        checked += 1;
        check_package(root, package)?;
    }

    if checked == 0 {
        return Err("no WASM-required packages are present".into());
    }

    clippy_cloud_wasm(root)?;
    println!("wasm checks passed for {checked} packages");
    Ok(())
}

fn check_package(root: &Path, package: &str) -> Result<()> {
    let label =
        format!("cargo check --target wasm32-unknown-unknown -p {package} --no-default-features");
    run_cargo(
        root,
        &label,
        &[
            "check",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            package,
            "--no-default-features",
        ],
    )
}

fn clippy_cloud_wasm(root: &Path) -> Result<()> {
    run_cargo(
        root,
        "cargo clippy --target wasm32-unknown-unknown -p comsat-cloud --no-default-features --lib --no-deps",
        &[
            "clippy",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            "comsat-cloud",
            "--no-default-features",
            "--lib",
            "--no-deps",
            "--",
            "-D",
            "warnings",
        ],
    )
}

fn run_cargo(root: &Path, label: &str, args: &[&str]) -> Result<()> {
    eprintln!("==> {label}");
    let status = Command::new("cargo").args(args).current_dir(root).status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("`{label}` exited with {status}").into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err("`cargo` is missing; install Rust toolchain 1.98.1 and rerun".into())
        }
        Err(error) => Err(format!("failed to start `{label}`: {error}").into()),
    }
}
