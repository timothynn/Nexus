//! Plugin manifests and explicit capability boundaries.

use std::{collections::BTreeSet, fs, path::{Path, PathBuf}};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    FilesystemRead,
    FilesystemWrite,
    ShellExecute,
    Network,
    ModelAccess,
    WorkspaceManage,
    SessionRead,
    SessionWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub entrypoint: String,
    #[serde(default)]
    pub capabilities: BTreeSet<PluginCapability>,
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), PluginError> {
        if self.name.trim().is_empty() || !self.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
            return Err(PluginError::InvalidManifest("invalid plugin name".to_owned()));
        }
        if self.version.trim().is_empty() {
            return Err(PluginError::InvalidManifest("version is required".to_owned()));
        }
        if self.entrypoint.trim().is_empty() {
            return Err(PluginError::InvalidManifest("entrypoint is required".to_owned()));
        }
        Ok(())
    }

    #[must_use]
    pub fn allows(&self, capability: PluginCapability) -> bool {
        self.capabilities.contains(&capability)
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub root: PathBuf,
    pub manifest: PluginManifest,
}

pub fn discover_plugins(root: impl AsRef<Path>) -> Result<Vec<DiscoveredPlugin>, PluginError> {
    let directory = root.as_ref().join(".nexus").join("plugins");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut plugins = Vec::new();
    for entry in fs::read_dir(directory).map_err(PluginError::Io)? {
        let entry = entry.map_err(PluginError::Io)?;
        if !entry.file_type().map_err(PluginError::Io)?.is_dir() {
            continue;
        }
        let root = entry.path();
        let manifest_path = root.join("plugin.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest: PluginManifest = toml::from_str(&fs::read_to_string(&manifest_path).map_err(PluginError::Io)?)
            .map_err(PluginError::Toml)?;
        manifest.validate()?;
        plugins.push(DiscoveredPlugin { root, manifest });
    }
    plugins.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
    Ok(plugins)
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("invalid plugin manifest: {0}")]
    InvalidManifest(String),
    #[error("plugin I/O failed: {0}")]
    Io(std::io::Error),
    #[error("invalid plugin configuration: {0}")]
    Toml(toml::de::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use super::{PluginCapability, PluginManifest};

    #[test]
    fn capabilities_are_explicit() {
        let manifest = PluginManifest {
            name: "example".to_owned(),
            version: "1.0.0".to_owned(),
            description: String::new(),
            entrypoint: "plugin".to_owned(),
            capabilities: BTreeSet::from([PluginCapability::FilesystemRead]),
        };
        assert!(manifest.allows(PluginCapability::FilesystemRead));
        assert!(!manifest.allows(PluginCapability::ShellExecute));
    }
}
