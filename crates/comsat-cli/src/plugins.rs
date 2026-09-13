use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use comsat_engine::{CatalogSource, SourceCatalog};
use comsat_source::{SourceCommandNames, SourceDescriptor, SourceProfile};
use comsat_types::SourceId;
use incurs::{
    agent_plugin::loader::{AgentPluginLoadOptions, LoadedAgentPlugin, load_agent_plugin},
    agent_plugin_runtime::{AgentPluginRuntimeOptions, connect_agent_plugin},
    tool::ToolDefinition,
};
use serde_json::Value;

const PLUGIN_ENV: &str = "COMSAT_PLUGINS";
const SOURCE_EXTENSION: &str = "io.comsat.source";

pub async fn catalog_with_agent_plugins() -> Result<Arc<SourceCatalog>, PluginSourceError> {
    let report = load_sources_from_env().await?;
    for diagnostic in &report.diagnostics {
        eprintln!(
            "{}",
            serde_json::json!({
                "code": "comsat_plugin_source_load_failed",
                "root": diagnostic.root,
                "message": diagnostic.message,
            })
        );
    }
    Ok(Arc::new(SourceCatalog::new(report.sources)))
}

pub async fn load_sources_from_env() -> Result<PluginLoadReport, PluginSourceError> {
    let Some(paths) = env::var_os(PLUGIN_ENV) else {
        return Ok(PluginLoadReport::default());
    };
    let roots = env::split_paths(&paths).collect::<Vec<_>>();
    let data_root = plugin_data_root();
    load_sources(roots, &data_root).await
}

pub async fn load_sources(
    roots: impl IntoIterator<Item = PathBuf>,
    data_root: &Path,
) -> Result<PluginLoadReport, PluginSourceError> {
    let mut report = PluginLoadReport::default();
    let data_root = data_root
        .canonicalize()
        .unwrap_or_else(|_| data_root.to_path_buf());
    let mut sources = Vec::new();
    for root in roots {
        match load_source(&root, &data_root).await {
            Ok(source) => sources.push(source),
            Err(error) => report.diagnostics.push(PluginLoadDiagnostic {
                root,
                message: error.to_string(),
            }),
        }
    }
    report.sources = sources;
    Ok(report)
}

pub async fn load_source(
    root: impl AsRef<Path>,
    data_root: &Path,
) -> Result<CatalogSource, PluginSourceError> {
    let root = root
        .as_ref()
        .canonicalize()
        .unwrap_or_else(|_| root.as_ref().to_path_buf());
    let load_options = AgentPluginLoadOptions {
        plugin_data_root: data_root.join(plugin_data_name(&root)),
        ..AgentPluginLoadOptions::default()
    };
    let report = load_agent_plugin(root, &load_options);
    let diagnostics = report.diagnostics.iter().map(diagnostic_text).collect();
    let plugin = report
        .plugin
        .ok_or(PluginSourceError::InvalidPlugin { diagnostics })?;
    let profile = SourceExtension::from_plugin(&plugin)?;
    let connected = connect_agent_plugin(&plugin, &AgentPluginRuntimeOptions::default()).await?;
    let descriptor = profile.descriptor(&connected.catalog.definitions())?;
    Ok(CatalogSource::new(descriptor, connected.catalog))
}

#[derive(Default)]
pub struct PluginLoadReport {
    pub sources: Vec<CatalogSource>,
    pub diagnostics: Vec<PluginLoadDiagnostic>,
}

#[derive(Debug)]
pub struct PluginLoadDiagnostic {
    pub root: PathBuf,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginSourceError {
    #[error("invalid Agent Plugin package: {}", .diagnostics.join("; "))]
    InvalidPlugin { diagnostics: Vec<String> },
    #[error("missing io.comsat.source manifest extension")]
    MissingExtension,
    #[error("COMSAT source profile version must be 1")]
    UnsupportedVersion,
    #[error("COMSAT source id is invalid: {0}")]
    InvalidSourceId(#[from] comsat_types::ValidationError),
    #[error("COMSAT source displayName is required")]
    MissingDisplayName,
    #[error("failed to connect Agent Plugin MCP tools: {0}")]
    Connect(#[from] incurs::agent_plugin_runtime::AgentPluginRuntimeError),
    #[error("COMSAT source `{source_id}` is missing required `{operation}` tool")]
    MissingTool {
        source_id: SourceId,
        operation: &'static str,
    },
    #[error("COMSAT source `{source_id}` has ambiguous `{operation}` tools: {}", .tools.join(", "))]
    AmbiguousTool {
        source_id: SourceId,
        operation: &'static str,
        tools: Vec<String>,
    },
}

#[derive(Clone)]
struct SourceExtension {
    id: SourceId,
    display_name: String,
    supports_fetch: bool,
    supports_follow: bool,
}

impl SourceExtension {
    fn from_plugin(plugin: &LoadedAgentPlugin) -> Result<Self, PluginSourceError> {
        let extension = plugin
            .extensions
            .get(SOURCE_EXTENSION)
            .ok_or(PluginSourceError::MissingExtension)?;
        let object = extension
            .as_object()
            .ok_or(PluginSourceError::MissingExtension)?;
        if object.get("version").and_then(Value::as_u64) != Some(1) {
            return Err(PluginSourceError::UnsupportedVersion);
        }
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let display_name = object
            .get("displayName")
            .and_then(Value::as_str)
            .ok_or(PluginSourceError::MissingDisplayName)?
            .to_string();
        Ok(Self {
            id: SourceId::new(id)?,
            display_name,
            supports_fetch: object
                .get("supportsFetch")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            supports_follow: object
                .get("supportsFollow")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn descriptor(
        &self,
        definitions: &[ToolDefinition],
    ) -> Result<SourceDescriptor, PluginSourceError> {
        let names = definitions
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>();
        let search = find_tool(&names, &self.id, "search")?.ok_or_else(|| {
            PluginSourceError::MissingTool {
                source_id: self.id.clone(),
                operation: "search",
            }
        })?;
        let fetch = optional_tool(&names, &self.id, "fetch", self.supports_fetch)?;
        let follow = optional_tool(&names, &self.id, "follow", self.supports_follow)?;
        Ok(SourceDescriptor {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            profile: SourceProfile {
                search: true,
                fetch: fetch.is_some(),
                follow: follow.is_some(),
            },
            commands: SourceCommandNames {
                search,
                fetch: fetch.unwrap_or_default(),
                follow: follow.unwrap_or_default(),
            },
        })
    }
}

fn optional_tool(
    names: &[&str],
    source: &SourceId,
    operation: &'static str,
    advertised: bool,
) -> Result<Option<String>, PluginSourceError> {
    if !advertised {
        return Ok(None);
    }
    find_tool(names, source, operation)?
        .map(Some)
        .ok_or_else(|| PluginSourceError::MissingTool {
            source_id: source.clone(),
            operation,
        })
}

fn find_tool(
    names: &[&str],
    source: &SourceId,
    operation: &'static str,
) -> Result<Option<String>, PluginSourceError> {
    let source_name = source.as_str().replace('-', "_");
    let comsat_name = format!("comsat_{source_name}_{operation}");
    let suffix = format!("_{operation}");
    for candidate in [operation, comsat_name.as_str()] {
        if names.contains(&candidate) {
            return Ok(Some(candidate.to_string()));
        }
    }
    let matches = names
        .iter()
        .filter(|name| name.ends_with(&suffix) || name.ends_with(&format!("_{comsat_name}")))
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [tool] => Ok(Some(tool.clone())),
        _ => Err(PluginSourceError::AmbiguousTool {
            source_id: source.clone(),
            operation,
            tools: matches,
        }),
    }
}

fn diagnostic_text(diagnostic: &incurs::agent_plugin::loader::AgentPluginDiagnostic) -> String {
    format!(
        "{}: {}: {}",
        diagnostic.path, diagnostic.code, diagnostic.message
    )
}

fn plugin_data_root() -> PathBuf {
    env::var_os("COMSAT_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("XDG_DATA_HOME").map(|root| PathBuf::from(root).join("comsat")))
        .unwrap_or_else(|| {
            env::var_os("HOME")
                .map_or_else(env::temp_dir, PathBuf::from)
                .join(".local/share/comsat")
        })
        .join("agent-plugin-data")
}

fn plugin_data_name(root: &Path) -> String {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("plugin")
        .to_string();
    let digest = stable_path_digest(root);
    format!("{name}-{digest:016x}")
}

fn stable_path_digest(path: &Path) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    path.to_string_lossy()
        .bytes()
        .fold(FNV_OFFSET, |hash, byte| {
            hash.wrapping_mul(FNV_PRIME) ^ u64::from(byte)
        })
}

#[cfg(test)]
mod tests;
