//! Isolated workspace primitives backed by Git worktrees.

use std::{fs, path::{Path, PathBuf}, process::Command};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitWorktree { pub name: String, pub branch: String, pub path: PathBuf }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWorkspace { pub agent_index: usize, pub worktree: GitWorktree }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupPolicy { Keep, RemoveClean, RemoveAlways }

pub trait WorkspaceBackend: Send + Sync {
    fn name(&self) -> &str;
    fn describe(&self) -> String;
}

pub struct GitWorktreeManager { repository: PathBuf, worktrees_root: PathBuf }
impl GitWorktreeManager {
    pub fn new(repository: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let repository = repository.into().canonicalize().map_err(WorkspaceError::Io)?;
        let manager = Self { worktrees_root: repository.join(".nexus").join("worktrees"), repository };
        manager.ensure_repository()?; Ok(manager)
    }
    #[must_use] pub fn repository(&self) -> &Path { &self.repository }
    #[must_use] pub fn worktrees_root(&self) -> &Path { &self.worktrees_root }
    pub fn create(&self, name: &str, base: Option<&str>) -> Result<GitWorktree, WorkspaceError> {
        validate_name(name)?; fs::create_dir_all(&self.worktrees_root).map_err(WorkspaceError::Io)?;
        let path = self.worktrees_root.join(name); if path.exists() { return Err(WorkspaceError::AlreadyExists(name.to_owned())); }
        let branch = format!("nexus/{name}"); let path_string = path.to_string_lossy().to_string();
        self.git(&["worktree", "add", "-b", branch.as_str(), path_string.as_str(), base.unwrap_or("HEAD")])?;
        Ok(GitWorktree { name: name.to_owned(), branch, path })
    }
    pub fn allocate_agents(&self, run_name: &str, count: usize, base: Option<&str>) -> Result<Vec<AgentWorkspace>, WorkspaceError> {
        if count == 0 { return Err(WorkspaceError::InvalidAgentCount); } validate_name(run_name)?;
        let mut allocated = Vec::with_capacity(count);
        for agent_index in 0..count { let name = format!("{run_name}-agent-{}", agent_index + 1); match self.create(&name, base) { Ok(worktree) => allocated.push(AgentWorkspace { agent_index, worktree }), Err(error) => { for workspace in &allocated { let _ = self.remove(&workspace.worktree.name, true); } return Err(error); } } }
        Ok(allocated)
    }
    pub fn cleanup(&self, name: &str, policy: CleanupPolicy) -> Result<bool, WorkspaceError> {
        match policy { CleanupPolicy::Keep => Ok(false), CleanupPolicy::RemoveAlways => { self.remove(name, true)?; Ok(true) }, CleanupPolicy::RemoveClean => { if self.status(name)?.is_empty() { self.remove(name, false)?; Ok(true) } else { Ok(false) } } }
    }
    pub fn list(&self) -> Result<Vec<GitWorktree>, WorkspaceError> {
        let output = self.git(&["worktree", "list", "--porcelain"])?; let mut worktrees = Vec::new(); let mut path = None; let mut branch = None;
        for line in output.lines().chain(std::iter::once("")) { if line.is_empty() { if let (Some(path), Some(branch)) = (path.take(), branch.take()) { let path = PathBuf::from(path); if let Ok(relative) = path.strip_prefix(&self.worktrees_root) { if relative.components().count() == 1 { worktrees.push(GitWorktree { name: relative.to_string_lossy().to_string(), branch, path }); } } } continue; } if let Some(value) = line.strip_prefix("worktree ") { path = Some(value.to_owned()); } else if let Some(value) = line.strip_prefix("branch refs/heads/") { branch = Some(value.to_owned()); } }
        worktrees.sort_by(|left, right| left.name.cmp(&right.name)); Ok(worktrees)
    }
    pub fn diff(&self, name: &str) -> Result<String, WorkspaceError> { let worktree = self.find(name)?; self.git_in(&worktree.path, &["diff", "--", "."]) }
    pub fn status(&self, name: &str) -> Result<String, WorkspaceError> { let worktree = self.find(name)?; self.git_in(&worktree.path, &["status", "--short"]) }
    pub fn remove(&self, name: &str, force: bool) -> Result<(), WorkspaceError> { let worktree = self.find(name)?; let path = worktree.path.to_string_lossy().to_string(); if force { self.git(&["worktree", "remove", "--force", path.as_str()])?; } else { self.git(&["worktree", "remove", path.as_str()])?; } Ok(()) }
    fn find(&self, name: &str) -> Result<GitWorktree, WorkspaceError> { self.list()?.into_iter().find(|worktree| worktree.name == name).ok_or_else(|| WorkspaceError::NotFound(name.to_owned())) }
    fn ensure_repository(&self) -> Result<(), WorkspaceError> { self.git(&["rev-parse", "--is-inside-work-tree"]).map(|_| ()) }
    fn git(&self, args: &[&str]) -> Result<String, WorkspaceError> { self.git_in(&self.repository, args) }
    fn git_in(&self, directory: &Path, args: &[&str]) -> Result<String, WorkspaceError> { let output = Command::new("git").current_dir(directory).args(args).output().map_err(WorkspaceError::Io)?; if !output.status.success() { return Err(WorkspaceError::Git(String::from_utf8_lossy(&output.stderr).trim().to_owned())); } Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned()) }
}

fn validate_name(name: &str) -> Result<(), WorkspaceError> { if name.is_empty() || matches!(name, "." | "..") || !name.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')) { return Err(WorkspaceError::InvalidName(name.to_owned())); } Ok(()) }
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError { #[error("workspace name is invalid: {0}")] InvalidName(String), #[error("workspace already exists: {0}")] AlreadyExists(String), #[error("workspace not found: {0}")] NotFound(String), #[error("parallel agent count must be greater than zero")] InvalidAgentCount, #[error("git command failed: {0}")] Git(String), #[error(transparent)] Io(#[from] std::io::Error) }

#[cfg(test)] mod tests { use super::{CleanupPolicy, WorkspaceError, validate_name}; #[test] fn workspace_names_reject_paths() { assert!(validate_name("../escape").is_err()); assert!(validate_name("agent-auth_01").is_ok()); } #[test] fn cleanup_policy_is_explicit() { assert_eq!(CleanupPolicy::Keep, CleanupPolicy::Keep); assert!(matches!(WorkspaceError::InvalidAgentCount, WorkspaceError::InvalidAgentCount)); } }
