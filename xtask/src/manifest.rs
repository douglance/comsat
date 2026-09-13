use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::Result;

#[derive(Debug)]
pub struct Workspace {
    pub packages: Vec<Package>,
    pub rust_version: Option<String>,
    pub dependencies: BTreeMap<String, Dependency>,
}

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub root: PathBuf,
    pub dependencies: Vec<DependencyEdge>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyKind {
    Normal,
    Build,
    Dev,
}

#[derive(Clone, Debug)]
pub struct DependencyEdge {
    pub declared_name: String,
    pub dependency: Dependency,
    pub kind: DependencyKind,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    Version(String),
    Table(DependencyTable),
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct DependencyTable {
    pub package: Option<String>,
    pub path: Option<String>,
    pub workspace: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RootManifest {
    workspace: RootWorkspace,
}

#[derive(Debug, Deserialize)]
struct RootPackage {
    #[serde(rename = "rust-version")]
    rust_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RootWorkspace {
    package: RootPackage,
    #[serde(default)]
    dependencies: BTreeMap<String, Dependency>,
}

#[derive(Debug, Deserialize)]
struct PackageManifest {
    package: PackageSection,
    #[serde(default)]
    dependencies: BTreeMap<String, Dependency>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, Dependency>,
    #[serde(default, rename = "build-dependencies")]
    build_dependencies: BTreeMap<String, Dependency>,
}

#[derive(Debug, Deserialize)]
struct PackageSection {
    name: String,
}

pub fn read_workspace(root: &Path) -> Result<Workspace> {
    let root_manifest = read_root_manifest(root)?;
    let package_paths = package_manifests(root)?;
    let mut packages = Vec::new();

    for manifest_path in package_paths {
        let manifest = read_package_manifest(&manifest_path)?;
        let package_name = manifest.package.name.clone();
        let dependencies = dependency_edges(manifest);
        let package_root = manifest_path
            .parent()
            .ok_or_else(|| format!("manifest has no parent: {}", manifest_path.display()))?;
        packages.push(Package {
            name: package_name,
            root: package_root.to_path_buf(),
            dependencies,
        });
    }

    packages.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(Workspace {
        packages,
        rust_version: root_manifest.workspace.package.rust_version,
        dependencies: root_manifest.workspace.dependencies,
    })
}

impl Workspace {
    pub fn package_names(&self) -> BTreeSet<String> {
        self.packages
            .iter()
            .map(|package| package.name.clone())
            .collect()
    }

    pub fn dependency_package_name(&self, declared_name: &str, dependency: &Dependency) -> String {
        match dependency {
            Dependency::Version(version) => {
                let _ = version.as_str();
                declared_name.to_owned()
            }
            Dependency::Table(table) if table.workspace == Some(true) => {
                self.dependencies.get(declared_name).map_or_else(
                    || declared_name.to_owned(),
                    |workspace_dependency| {
                        self.dependency_package_name(declared_name, workspace_dependency)
                    },
                )
            }
            Dependency::Table(table) => table
                .package
                .clone()
                .unwrap_or_else(|| declared_name.to_owned()),
        }
    }

    pub fn dependency_path<'a>(
        &'a self,
        declared_name: &'a str,
        dependency: &'a Dependency,
    ) -> Option<&'a str> {
        match dependency {
            Dependency::Version(version) => {
                let _ = version.as_str();
                None
            }
            Dependency::Table(table) if table.workspace == Some(true) => self
                .dependencies
                .get(declared_name)
                .and_then(|workspace_dependency| {
                    self.dependency_path(declared_name, workspace_dependency)
                }),
            Dependency::Table(table) => table.path.as_deref(),
        }
    }
}

pub fn production_rs_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in ["crates", "plugins", "xtask"] {
        collect_rs_files(&root.join(entry), &mut files)?;
    }
    files.sort();
    Ok(files)
}

fn dependency_edges(manifest: PackageManifest) -> Vec<DependencyEdge> {
    let mut edges = Vec::new();
    push_edges(&mut edges, manifest.dependencies, DependencyKind::Normal);
    push_edges(
        &mut edges,
        manifest.build_dependencies,
        DependencyKind::Build,
    );
    push_edges(&mut edges, manifest.dev_dependencies, DependencyKind::Dev);
    edges
}

fn push_edges(
    edges: &mut Vec<DependencyEdge>,
    dependencies: BTreeMap<String, Dependency>,
    kind: DependencyKind,
) {
    edges.extend(
        dependencies
            .into_iter()
            .map(|(declared_name, dependency)| DependencyEdge {
                declared_name,
                dependency,
                kind,
            }),
    );
}

fn read_root_manifest(root: &Path) -> Result<RootManifest> {
    let manifest_path = root.join("Cargo.toml");
    let source = fs::read_to_string(&manifest_path)?;
    let manifest = toml::from_str(&source)?;
    Ok(manifest)
}

fn read_package_manifest(path: &Path) -> Result<PackageManifest> {
    let source = fs::read_to_string(path)?;
    let manifest = toml::from_str(&source)?;
    Ok(manifest)
}

fn package_manifests(root: &Path) -> Result<Vec<PathBuf>> {
    let mut manifests = Vec::new();
    collect_manifest_paths(&root.join("crates"), &mut manifests)?;
    collect_manifest_paths(&root.join("plugins"), &mut manifests)?;
    collect_manifest_paths(&root.join("xtask"), &mut manifests)?;
    manifests.sort();
    Ok(manifests)
}

fn collect_manifest_paths(dir: &Path, manifests: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let manifest_path = path.join("Cargo.toml");
            if manifest_path.exists() {
                manifests.push(manifest_path);
            }
        }
    }
    Ok(())
}

fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if is_excluded_dir(&path) {
                continue;
            }
            collect_rs_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn is_excluded_dir(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        matches!(
            name.to_str(),
            Some("tests" | "fixtures" | "testdata" | "snapshots")
        )
    })
}
