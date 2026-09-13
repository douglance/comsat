use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::manifest::{self, DependencyEdge, DependencyKind, Package, Workspace};
use crate::{Result, display_path};

pub fn run(root: &Path) -> Result<()> {
    let workspace = manifest::read_workspace(root)?;
    let mut violations = Vec::new();

    check_rust_version(root, &workspace, &mut violations);
    check_declared_paths(root, &workspace, &mut violations);
    check_dependency_boundaries(&workspace, &mut violations);
    check_plugin_names(&workspace, &mut violations);
    check_unsafe_policy(root, &mut violations)?;

    if violations.is_empty() {
        println!("architecture checks passed");
        Ok(())
    } else {
        Err(format!("architecture check failed:\n{}", violations.join("\n")).into())
    }
}

fn check_rust_version(root: &Path, workspace: &Workspace, violations: &mut Vec<String>) {
    if workspace.rust_version.as_deref() != Some("1.98") {
        violations.push(format!(
            "workspace rust-version must be `1.98`, found `{}`",
            workspace.rust_version.as_deref().unwrap_or("<missing>")
        ));
    }

    let toolchain_path = root.join("rust-toolchain.toml");
    match fs::read_to_string(&toolchain_path) {
        Ok(source) if source.contains("channel = \"1.98.1\"") => {}
        Ok(_) => violations.push("rust-toolchain.toml must pin channel `1.98.1`".to_owned()),
        Err(error) => violations.push(format!("failed to read rust-toolchain.toml: {error}")),
    }
}

fn check_declared_paths(root: &Path, workspace: &Workspace, violations: &mut Vec<String>) {
    for (name, dependency) in &workspace.dependencies {
        if let Some(path) = workspace.dependency_path(name, dependency) {
            let resolved = root.join(path);
            if !resolved.exists() {
                violations.push(format!(
                    "workspace dependency `{name}` points at missing path `{path}`"
                ));
            }
        }
    }
}

fn check_dependency_boundaries(workspace: &Workspace, violations: &mut Vec<String>) {
    let package_names = workspace.package_names();
    for package in &workspace.packages {
        for edge in &package.dependencies {
            let dependency_name =
                workspace.dependency_package_name(&edge.declared_name, &edge.dependency);
            check_package_dependency(package, edge, &dependency_name, &package_names, violations);
        }
    }
}

// comsat-allow-complexity body LOC reason: dependency policy is kept as one visible crate-boundary table.
// comsat-allow-complexity cyclomatic complexity reason: each match arm represents one public architecture boundary.
fn check_package_dependency(
    package: &Package,
    edge: &DependencyEdge,
    dependency_name: &str,
    package_names: &BTreeSet<String>,
    violations: &mut Vec<String>,
) {
    if package.name == dependency_name || allowed_dev_dependency(package, edge, dependency_name) {
        return;
    }

    if is_plugin_package(package) {
        forbid_plugin_dependency(package, dependency_name, package_names, violations);
        return;
    }

    if package.name == "comsat-engine" {
        forbid_engine_dependency(package, dependency_name, violations);
        return;
    }

    if let Some(forbidden) = forbidden_dependencies(&package.name) {
        forbid_any(package, dependency_name, forbidden, violations);
    }

    if core_forbids_source_crates(&package.name) && is_plugin_crate(dependency_name, package_names)
    {
        violations.push(boundary_violation(package, dependency_name));
    }
}

fn allowed_dev_dependency(package: &Package, edge: &DependencyEdge, dependency_name: &str) -> bool {
    package.name == "comsat-cloud"
        && edge.kind == DependencyKind::Dev
        && dependency_name == "rusqlite"
}

fn forbidden_dependencies(package_name: &str) -> Option<&'static [&'static str]> {
    match package_name {
        "comsat-types" => Some(&["incurs", "worker", "rusqlite", "reqwest", "http"]),
        "comsat-source" => Some(&[
            "comsat-engine",
            "comsat-app",
            "comsat-cloud",
            "worker",
            "rusqlite",
            "reqwest",
        ]),
        "comsat-app" => Some(&[
            "comsat-cli",
            "comsat-cloud",
            "comsat-store-sqlite",
            "worker",
            "rusqlite",
        ]),
        "comsat-store" => Some(&[
            "incurs",
            "worker",
            "rusqlite",
            "reqwest",
            "comsat-engine",
            "comsat-app",
        ]),
        "comsat-store-sqlite" => Some(&["comsat-engine", "comsat-app", "comsat-cloud", "worker"]),
        "comsat-cloud" => Some(&["rusqlite", "comsat-store-sqlite"]),
        _ => None,
    }
}

fn forbid_engine_dependency(
    package: &Package,
    dependency_name: &str,
    violations: &mut Vec<String>,
) {
    if dependency_name == "reqwest"
        || dependency_name == "worker"
        || dependency_name == "rusqlite"
        || dependency_name.starts_with("comsat-github")
        || dependency_name.starts_with("comsat-hacker-news")
        || dependency_name.starts_with("comsat-web")
        || dependency_name.starts_with("comsat-stack-exchange")
    {
        violations.push(boundary_violation(package, dependency_name));
    }
}

fn forbid_plugin_dependency(
    package: &Package,
    dependency_name: &str,
    package_names: &BTreeSet<String>,
    violations: &mut Vec<String>,
) {
    let local_core_allowed = ["comsat-types", "comsat-source"];
    let upward_forbidden = [
        "comsat-engine",
        "comsat-app",
        "comsat-cloud",
        "comsat-cli",
        "comsat-store",
        "comsat-store-sqlite",
    ];

    if upward_forbidden.contains(&dependency_name) {
        violations.push(boundary_violation(package, dependency_name));
    }

    if dependency_name.starts_with("comsat-")
        && package_names.contains(dependency_name)
        && !local_core_allowed.contains(&dependency_name)
    {
        violations.push(format!(
            "`{}` must not depend on local COMSAT crate `{dependency_name}`",
            package.name
        ));
    }
}

fn forbid_any(
    package: &Package,
    dependency_name: &str,
    forbidden: &[&str],
    violations: &mut Vec<String>,
) {
    if forbidden.contains(&dependency_name) {
        violations.push(boundary_violation(package, dependency_name));
    }
}

fn core_forbids_source_crates(package_name: &str) -> bool {
    matches!(
        package_name,
        "comsat-types" | "comsat-source" | "comsat-app" | "comsat-store" | "comsat-store-sqlite"
    )
}

fn is_plugin_crate(dependency_name: &str, package_names: &BTreeSet<String>) -> bool {
    package_names.contains(dependency_name)
        && matches!(
            dependency_name,
            "comsat-github" | "comsat-hacker-news" | "comsat-web" | "comsat-stack-exchange"
        )
}

fn boundary_violation(package: &Package, dependency_name: &str) -> String {
    format!(
        "`{}` has forbidden dependency `{dependency_name}`",
        package.name
    )
}

fn check_plugin_names(workspace: &Workspace, violations: &mut Vec<String>) {
    for package in &workspace.packages {
        if is_plugin_package(package) {
            let source_id = package.root.file_name().and_then(|name| name.to_str());
            if let Some(source_id) = source_id {
                let expected = format!("comsat-{source_id}");
                if package.name != expected {
                    violations.push(format!(
                        "plugin `{source_id}` package name must be `{expected}`, found `{}`",
                        package.name
                    ));
                }
            }
        }
    }
}

fn is_plugin_package(package: &Package) -> bool {
    package
        .root
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name.to_str() == Some("plugins"))
}

fn check_unsafe_policy(root: &Path, violations: &mut Vec<String>) -> Result<()> {
    for package in manifest::read_workspace(root)?.packages {
        for file in crate_root_files(&package) {
            if !file.exists() {
                continue;
            }
            let source = fs::read_to_string(&file)?;
            if source.contains("#![forbid(unsafe_code)]") {
                continue;
            }
            violations.push(format!(
                "{} must contain `#![forbid(unsafe_code)]`",
                display_path(root, &file).display()
            ));
        }
    }
    Ok(())
}

fn crate_root_files(package: &Package) -> [std::path::PathBuf; 2] {
    [
        package.root.join("src/lib.rs"),
        package.root.join("src/main.rs"),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use crate::architecture::{check_package_dependency, is_plugin_package};
    use crate::manifest::{Dependency, DependencyEdge, DependencyKind, Package};

    #[test]
    fn plugin_packages_reject_upward_dependencies() {
        let package = Package {
            name: "comsat-github".to_owned(),
            root: PathBuf::from("/workspace/plugins/github"),
            dependencies: Vec::new(),
        };
        let package_names = BTreeSet::from(["comsat-engine".to_owned()]);
        let mut violations = Vec::new();

        let edge = edge("comsat-engine", DependencyKind::Normal);
        check_package_dependency(
            &package,
            &edge,
            "comsat-engine",
            &package_names,
            &mut violations,
        );

        assert!(!violations.is_empty());
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("forbidden dependency"))
        );
    }

    #[test]
    fn plugin_path_detection_uses_parent_directory() {
        let package = Package {
            name: "comsat-web".to_owned(),
            root: PathBuf::from("/workspace/plugins/web"),
            dependencies: Vec::new(),
        };

        assert!(is_plugin_package(&package));
    }

    #[test]
    fn cloud_dev_rusqlite_is_allowed_but_normal_rusqlite_is_forbidden() {
        let package = Package {
            name: "comsat-cloud".to_owned(),
            root: PathBuf::from("/workspace/crates/comsat-cloud"),
            dependencies: Vec::new(),
        };
        let package_names = BTreeSet::new();
        let mut violations = Vec::new();

        let dev_edge = edge("rusqlite", DependencyKind::Dev);
        check_package_dependency(
            &package,
            &dev_edge,
            "rusqlite",
            &package_names,
            &mut violations,
        );
        assert!(violations.is_empty());

        let normal_edge = edge("rusqlite", DependencyKind::Normal);
        check_package_dependency(
            &package,
            &normal_edge,
            "rusqlite",
            &package_names,
            &mut violations,
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("rusqlite"))
        );
    }

    fn edge(name: &str, kind: DependencyKind) -> DependencyEdge {
        DependencyEdge {
            declared_name: name.to_owned(),
            dependency: Dependency::Version("0".to_owned()),
            kind,
        }
    }
}
