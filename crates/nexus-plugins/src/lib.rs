//! Plugin manifests, capability boundaries, and permissioned runtime execution.

use std::{collections::BTreeSet, fs, path::{Path, PathBuf}, process::Command};
use serde::{Deserialize, Serialize};
use nexus_permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability { FilesystemRead, FilesystemWrite, ShellExecute, Network, ModelAccess, WorkspaceManage, SessionRead, SessionWrite }

impl PluginCapability {
    #[must_use] pub fn action(&self) -> &'static str { match self { Self::FilesystemRead => "plugin.filesystem.read", Self::FilesystemWrite => "plugin.filesystem.write", Self::ShellExecute => "plugin.shell.execute", Self::Network => "plugin.network", Self::ModelAccess => "plugin.model.access", Self::WorkspaceManage => "plugin.workspace.manage", Self::SessionRead => "plugin.session.read", Self::SessionWrite => "plugin.session.write" } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest { pub name: String, pub version: String, #[serde(default)] pub description: String, #[serde(default)] pub entrypoint: String, #[serde(default)] pub capabilities: BTreeSet<PluginCapability> }
impl PluginManifest {
    pub fn validate(&self) -> Result<(), PluginError> { if self.name.trim().is_empty() || !self.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) { return Err(PluginError::InvalidManifest("invalid plugin name".to_owned())); } if self.version.trim().is_empty() { return Err(PluginError::InvalidManifest("version is required".to_owned())); } if self.entrypoint.trim().is_empty() { return Err(PluginError::InvalidManifest("entrypoint is required".to_owned())); } Ok(()) }
    #[must_use] pub fn allows(&self, capability: PluginCapability) -> bool { self.capabilities.contains(&capability) }
}

#[derive(Debug, Clone)]
pub struct DiscoveredPlugin { pub root: PathBuf, pub manifest: PluginManifest }

pub fn discover_plugins(root: impl AsRef<Path>) -> Result<Vec<DiscoveredPlugin>, PluginError> {
    let directory = root.as_ref().join(".nexus").join("plugins"); if !directory.exists() { return Ok(Vec::new()); }
    let mut plugins = Vec::new();
    for entry in fs::read_dir(directory).map_err(PluginError::Io)? { let entry = entry.map_err(PluginError::Io)?; if !entry.file_type().map_err(PluginError::Io)?.is_dir() { continue; } let root = entry.path(); let manifest_path = root.join("plugin.toml"); if !manifest_path.is_file() { continue; } let manifest: PluginManifest = toml::from_str(&fs::read_to_string(&manifest_path).map_err(PluginError::Io)?).map_err(PluginError::Toml)?; manifest.validate()?; plugins.push(DiscoveredPlugin { root, manifest }); }
    plugins.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name)); Ok(plugins)
}

pub trait PluginAuditSink: Send + Sync { fn record(&self, event: PluginAuditEvent); }
pub struct NoopPluginAuditSink;
impl PluginAuditSink for NoopPluginAuditSink { fn record(&self, _event: PluginAuditEvent) {} }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginAuditEvent { CapabilityGranted { plugin: String, capability: PluginCapability }, CapabilityDenied { plugin: String, capability: PluginCapability }, Started { plugin: String }, Completed { plugin: String, success: bool } }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRunResult { pub plugin: String, pub success: bool, pub exit_code: Option<i32>, pub stdout: String, pub stderr: String }

pub struct PluginRuntime<P, A> { policy: P, audit: A }
impl<P: PermissionPolicy, A: PluginAuditSink> PluginRuntime<P, A> {
    #[must_use] pub fn new(policy: P, audit: A) -> Self { Self { policy, audit } }
    pub fn authorize(&self, plugin: &DiscoveredPlugin, capability: PluginCapability) -> Result<(), PluginError> {
        if !plugin.manifest.allows(capability.clone()) { self.audit.record(PluginAuditEvent::CapabilityDenied { plugin: plugin.manifest.name.clone(), capability }); return Err(PluginError::CapabilityUndeclared(capability)); }
        let request = PermissionRequest { action: capability.action().to_owned(), ..Default::default() };
        match self.policy.decide(&request) { PermissionDecision::Allow => { self.audit.record(PluginAuditEvent::CapabilityGranted { plugin: plugin.manifest.name.clone(), capability }); Ok(()) }, PermissionDecision::Ask | PermissionDecision::Deny | PermissionDecision::Sandbox => { self.audit.record(PluginAuditEvent::CapabilityDenied { plugin: plugin.manifest.name.clone(), capability }); Err(PluginError::CapabilityDenied(capability)) } }
    }
    pub fn run(&self, plugin: &DiscoveredPlugin, requested: &[PluginCapability], args: &[String]) -> Result<PluginRunResult, PluginError> {
        for capability in requested { self.authorize(plugin, capability.clone())?; }
        let entrypoint = plugin.root.join(&plugin.manifest.entrypoint); let entrypoint = entrypoint.canonicalize().map_err(PluginError::Io)?;
        if !entrypoint.starts_with(&plugin.root) { return Err(PluginError::EntrypointEscape); }
        self.audit.record(PluginAuditEvent::Started { plugin: plugin.manifest.name.clone() });
        let output = Command::new(&entrypoint).current_dir(&plugin.root).args(args).output().map_err(PluginError::Io)?;
        let result = PluginRunResult { plugin: plugin.manifest.name.clone(), success: output.status.success(), exit_code: output.status.code(), stdout: String::from_utf8_lossy(&output.stdout).to_string(), stderr: String::from_utf8_lossy(&output.stderr).to_string() };
        self.audit.record(PluginAuditEvent::Completed { plugin: result.plugin.clone(), success: result.success }); Ok(result)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError { #[error("invalid plugin manifest: {0}")] InvalidManifest(String), #[error("plugin capability was not declared: {0:?}")] CapabilityUndeclared(PluginCapability), #[error("plugin capability was denied: {0:?}")] CapabilityDenied(PluginCapability), #[error("plugin entrypoint escapes the plugin root")] EntrypointEscape, #[error("plugin I/O failed: {0}")] Io(std::io::Error), #[error("invalid plugin configuration: {0}")] Toml(toml::de::Error) }

#[cfg(test)] mod tests { use std::collections::BTreeSet; use super::{PluginCapability, PluginManifest}; #[test] fn capabilities_are_explicit() { let manifest = PluginManifest { name: "example".to_owned(), version: "1.0.0".to_owned(), description: String::new(), entrypoint: "plugin".to_owned(), capabilities: BTreeSet::from([PluginCapability::FilesystemRead]) }; assert!(manifest.allows(PluginCapability::FilesystemRead)); assert!(!manifest.allows(PluginCapability::ShellExecute)); } }
