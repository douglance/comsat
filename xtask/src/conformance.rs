use std::fs;
use std::path::Path;
use std::process::Command;

use crate::manifest::{self, Package, Workspace};
use crate::{Result, display_path};

pub fn run(root: &Path) -> Result<()> {
    let workspace = manifest::read_workspace(root)?;
    let mut violations = Vec::new();

    check_plugin_manifests(root, &workspace, &mut violations)?;
    if !violations.is_empty() {
        return Err(format!("source conformance failed:\n{}", violations.join("\n")).into());
    }

    run_conformance_tests(root, &workspace)?;
    println!("source conformance checks passed");
    Ok(())
}

fn check_plugin_manifests(
    root: &Path,
    workspace: &Workspace,
    violations: &mut Vec<String>,
) -> Result<()> {
    for package in workspace
        .packages
        .iter()
        .filter(|package| is_plugin(package))
    {
        check_plugin_manifest(root, package, violations)?;
    }
    Ok(())
}

fn check_plugin_manifest(
    root: &Path,
    package: &Package,
    violations: &mut Vec<String>,
) -> Result<()> {
    let plugin_json = package.root.join("plugin.json");
    if !plugin_json.exists() {
        violations.push(format!(
            "{} is missing plugin.json with io.comsat.source metadata",
            display_path(root, &package.root).display()
        ));
        return Ok(());
    }

    let source = fs::read_to_string(&plugin_json)?;
    let document: serde_json::Value = serde_json::from_str(&source)?;
    if source_extension(&document).is_none() {
        violations.push(format!(
            "{} must declare extensions.io.comsat.source",
            display_path(root, &plugin_json).display()
        ));
    }
    Ok(())
}

fn run_conformance_tests(root: &Path, workspace: &Workspace) -> Result<()> {
    run_package_test(root, "comsat-source", None)?;
    for package in workspace
        .packages
        .iter()
        .filter(|package| is_plugin(package))
    {
        run_package_test(root, &package.name, Some("conformance"))?;
    }
    Ok(())
}

fn run_package_test(root: &Path, package: &str, test_target: Option<&str>) -> Result<()> {
    eprintln!("==> cargo test -p {package} conformance");
    let mut command = Command::new("cargo");
    command.args(["test", "-p", package]);
    if let Some(test_target) = test_target {
        command.args(["--test", test_target]);
    }
    command.args(["--", "--nocapture"]).current_dir(root);

    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => {
            Err(format!("`cargo test -p {package} conformance` exited with {status}").into())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err("`cargo` is missing; install Rust toolchain 1.98.1 and rerun".into())
        }
        Err(error) => Err(format!("failed to start cargo for `{package}`: {error}").into()),
    }
}

fn is_plugin(package: &Package) -> bool {
    package
        .root
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name.to_str() == Some("plugins"))
}

fn source_extension(
    document: &serde_json::Value,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    let extension = document
        .get("extensions")?
        .get("io.comsat.source")?
        .as_object()?;
    match (
        extension.get("version").and_then(serde_json::Value::as_u64),
        extension.get("id").and_then(serde_json::Value::as_str),
        extension
            .get("displayName")
            .and_then(serde_json::Value::as_str),
    ) {
        (Some(1), Some(id), Some(display_name)) if !id.is_empty() && !display_name.is_empty() => {
            Some(extension)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::conformance::source_extension;

    #[test]
    fn accepts_incurs_extension_namespace() {
        let document = json!({
            "extensions": {
                "io.comsat.source": {
                    "version": 1,
                    "id": "github",
                    "displayName": "GitHub"
                }
            }
        });

        assert!(source_extension(&document).is_some());
    }

    #[test]
    fn rejects_top_level_extension_shortcut() {
        let document = json!({
            "io.comsat.source": {
                "version": 1,
                "id": "github",
                "displayName": "GitHub"
            }
        });

        assert!(source_extension(&document).is_none());
    }
}
