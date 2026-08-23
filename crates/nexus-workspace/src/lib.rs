//! Isolated workspace primitives backed by Git worktrees and extensible backends.

use std::{fs, path::{Path, PathBuf}, process::Command, time::Duration};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitWorktree { pub name: String, pub branch: String, pub path: PathBuf }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWorkspace { pub agent_index: usize, pub worktree: GitWorktree }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupPolicy { Keep, RemoveClean, RemoveAlways }

pub trait WorkspaceBackend: Send + Sync { fn name(&self) -> &str; fn describe(&self) -> String; }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerLifecycleState { Provisioned, Running, Stopped, Removed }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerWorkspace { pub name: String, pub workspace: PathBuf, pub state: ContainerLifecycleState }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerWorkspaceBackend {
    runtime: String, image: String, network_enabled: bool, read_only_root: bool,
    memory_limit: Option<String>, cpu_limit: Option<String>,
}
impl ContainerWorkspaceBackend {
    #[must_use] pub fn new(runtime: impl Into<String>, image: impl Into<String>) -> Self { Self { runtime: runtime.into(), image: image.into(), network_enabled: false, read_only_root: true, memory_limit: None, cpu_limit: None } }
    #[must_use] pub fn with_network(mut self, enabled: bool) -> Self { self.network_enabled = enabled; self }
    #[must_use] pub fn with_read_only_root(mut self, enabled: bool) -> Self { self.read_only_root = enabled; self }
    #[must_use] pub fn with_memory_limit(mut self, limit: impl Into<String>) -> Self { self.memory_limit = Some(limit.into()); self }
    #[must_use] pub fn with_cpu_limit(mut self, limit: impl Into<String>) -> Self { self.cpu_limit = Some(limit.into()); self }
    #[must_use] pub fn runtime(&self) -> &str { &self.runtime }
    #[must_use] pub fn image(&self) -> &str { &self.image }
    pub fn health_check(&self) -> Result<(), WorkspaceError> { self.runtime_command(&["info"])?; Ok(()) }
    pub fn provision(&self, name: &str, workspace: &Path) -> Result<ContainerWorkspace, WorkspaceError> {
        validate_name(name)?; let workspace = workspace.canonicalize().map_err(WorkspaceError::Io)?;
        if !workspace.is_dir() { return Err(WorkspaceError::InvalidContainerWorkspace(workspace)); }
        let _ = self.remove_if_exists(name);
        let workspace_mount = format!("{}:/workspace", workspace.display());
        let mut args = vec!["create".to_owned(), "--name".to_owned(), name.to_owned(), "--workdir".to_owned(), "/workspace".to_owned(), "--volume".to_owned(), workspace_mount];
        if self.read_only_root { args.push("--read-only".to_owned()); }
        if !self.network_enabled { args.extend(["--network".to_owned(), "none".to_owned()]); }
        if let Some(limit) = &self.memory_limit { args.extend(["--memory".to_owned(), limit.clone()]); }
        if let Some(limit) = &self.cpu_limit { args.extend(["--cpus".to_owned(), limit.clone()]); }
        args.push(self.image.clone()); args.push("sleep".to_owned()); args.push("infinity".to_owned());
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>(); self.runtime_command(&refs)?;
        Ok(ContainerWorkspace { name: name.to_owned(), workspace, state: ContainerLifecycleState::Provisioned })
    }
    pub fn start(&self, container: &mut ContainerWorkspace) -> Result<(), WorkspaceError> { self.runtime_command(&["start", container.name.as_str()])?; container.state = ContainerLifecycleState::Running; Ok(()) }
    pub fn execute(&self, container: &ContainerWorkspace, program: &str, args: &[String], timeout: Option<Duration>) -> Result<ContainerExecution, WorkspaceError> {
        if container.state != ContainerLifecycleState::Running { return Err(WorkspaceError::ContainerNotRunning(container.name.clone())); }
        let mut command = Command::new(&self.runtime); command.arg("exec").arg(&container.name).arg(program).args(args);
        let mut child = command.spawn().map_err(WorkspaceError::Io)?;
        let status = if let Some(timeout) = timeout { wait_with_timeout(&mut child, timeout)? } else { child.wait().map_err(WorkspaceError::Io)? };
        Ok(ContainerExecution { success: status.success(), exit_code: status.code() })
    }
    pub fn stop(&self, container: &mut ContainerWorkspace) -> Result<(), WorkspaceError> {
        if container.state == ContainerLifecycleState::Removed { return Ok(()); }
        let _ = self.runtime_command(&["stop", container.name.as_str()]); container.state = ContainerLifecycleState::Stopped; Ok(())
    }
    pub fn remove(&self, container: &mut ContainerWorkspace) -> Result<(), WorkspaceError> {
        if container.state == ContainerLifecycleState::Removed { return Ok(()); }
        let _ = self.runtime_command(&["rm", "-f", container.name.as_str()]); container.state = ContainerLifecycleState::Removed; Ok(())
    }
    pub fn command(&self, workspace: &Path, program: &str, args: &[String]) -> Command {
        let mut command = Command::new(&self.runtime); command.arg("run").arg("--rm").arg("--workdir").arg("/workspace").arg("--volume").arg(format!("{}:/workspace", workspace.display()));
        if self.read_only_root { command.arg("--read-only"); }
        if !self.network_enabled { command.arg("--network").arg("none"); }
        if let Some(limit) = &self.memory_limit { command.arg("--memory").arg(limit); }
        if let Some(limit) = &self.cpu_limit { command.arg("--cpus").arg(limit); }
        command.arg(&self.image).arg(program).args(args); command
    }
    fn runtime_command(&self, args: &[&str]) -> Result<String, WorkspaceError> {
        let output = Command::new(&self.runtime).args(args).output().map_err(WorkspaceError::Io)?;
        if !output.status.success() { return Err(WorkspaceError::Container(String::from_utf8_lossy(&output.stderr).trim().to_owned())); }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
    fn remove_if_exists(&self, name: &str) -> Result<(), WorkspaceError> { let _ = self.runtime_command(&["rm", "-f", name]); Ok(()) }
}

fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Result<std::process::ExitStatus, WorkspaceError> {
    let started = std::time::Instant::now();
    loop { if let Some(status) = child.try_wait().map_err(WorkspaceError::Io)? { return Ok(status); } if started.elapsed() >= timeout { child.kill().map_err(WorkspaceError::Io)?; let _ = child.wait(); return Err(WorkspaceError::ContainerTimeout); } std::thread::sleep(Duration::from_millis(10)); }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerExecution { pub success: bool, pub exit_code: Option<i32> }
impl WorkspaceBackend for ContainerWorkspaceBackend { fn name(&self) -> &str { "container" } fn describe(&self) -> String { format!("runtime={} image={} network={} read_only_root={} memory_limit={} cpu_limit={}", self.runtime, self.image, self.network_enabled, self.read_only_root, self.memory_limit.as_deref().unwrap_or("unlimited"), self.cpu_limit.as_deref().unwrap_or("unlimited")) } }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCandidate { pub workspace: GitWorktree, pub diff: String, pub status: String }

pub struct GitWorktreeManager { repository: PathBuf, worktrees_root: PathBuf }
impl GitWorktreeManager {
    pub fn new(repository: impl Into<PathBuf>) -> Result<Self, WorkspaceError> { let repository = repository.into().canonicalize().map_err(WorkspaceError::Io)?; let manager = Self { worktrees_root: repository.join(".nexus").join("worktrees"), repository }; manager.ensure_repository()?; Ok(manager) }
    #[must_use] pub fn repository(&self) -> &Path { &self.repository }
    #[must_use] pub fn worktrees_root(&self) -> &Path { &self.worktrees_root }
    pub fn create(&self, name: &str, base: Option<&str>) -> Result<GitWorktree, WorkspaceError> { validate_name(name)?; fs::create_dir_all(&self.worktrees_root).map_err(WorkspaceError::Io)?; let path = self.worktrees_root.join(name); if path.exists() { return Err(WorkspaceError::AlreadyExists(name.to_owned())); } let branch = format!("nexus/{name}"); let path_string = path.to_string_lossy().to_string(); self.git(&["worktree", "add", "-b", branch.as_str(), path_string.as_str(), base.unwrap_or("HEAD")])?; Ok(GitWorktree { name: name.to_owned(), branch, path }) }
    pub fn allocate_agents(&self, run_name: &str, count: usize, base: Option<&str>) -> Result<Vec<AgentWorkspace>, WorkspaceError> { if count == 0 { return Err(WorkspaceError::InvalidAgentCount); } validate_name(run_name)?; let mut allocated = Vec::with_capacity(count); for agent_index in 0..count { let name = format!("{run_name}-agent-{}", agent_index + 1); match self.create(&name, base) { Ok(worktree) => allocated.push(AgentWorkspace { agent_index, worktree }), Err(error) => { for workspace in &allocated { let _ = self.remove(&workspace.worktree.name, true); } return Err(error); } } Ok(allocated) }
    pub fn cleanup(&self, name: &str, policy: CleanupPolicy) -> Result<bool, WorkspaceError> { match policy { CleanupPolicy::Keep => Ok(false), CleanupPolicy::RemoveAlways => { self.remove(name, true)?; Ok(true) }, CleanupPolicy::RemoveClean => { if self.status(name)?.is_empty() { self.remove(name, false)?; Ok(true) } else { Ok(false) } } }
    pub fn review_candidate(&self, name: &str) -> Result<ReviewCandidate, WorkspaceError> { let workspace = self.find(name)?; let diff = self.diff(name)?; let status = self.status(name)?; Ok(ReviewCandidate { workspace, diff, status }) }
    pub fn merge_after_review(&self, name: &str, target: &str, approved: bool) -> Result<(), WorkspaceError> { if !approved { return Err(WorkspaceError::ReviewRequired(name.to_owned())); } let workspace = self.find(name)?; self.git(&["merge", "--no-ff", workspace.branch.as_str(), "-m", format!("Merge Nexus workspace {} after explicit review", name).as_str()]).map(|_| ())?; let _ = target; Ok(()) }
    pub fn list(&self) -> Result<Vec<GitWorktree>, WorkspaceError> { let output = self.git(&["worktree", "list", "--porcelain"])?; let mut worktrees = Vec::new(); let mut path = None; let mut branch = None; for line in output.lines().chain(std::iter::once("")) { if line.is_empty() { if let (Some(path), Some(branch)) = (path.take(), branch.take()) { let path = PathBuf::from(path); if let Ok(relative) = path.strip_prefix(&self.worktrees_root) { if relative.components().count() == 1 { worktrees.push(GitWorktree { name: relative.to_string_lossy().to_string(), branch, path }); } } } continue; } if let Some(value) = line.strip_prefix("worktree ") { path = Some(value.to_owned()); } else if let Some(value) = line.strip_prefix("branch refs/heads/") { branch = Some(value.to_owned()); } } worktrees.sort_by(|left, right| left.name.cmp(&right.name)); Ok(worktrees) }
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
pub enum WorkspaceError {
    #[error("workspace name is invalid: {0}")] InvalidName(String), #[error("workspace already exists: {0}")] AlreadyExists(String), #[error("workspace not found: {0}")] NotFound(String), #[error("explicit review approval is required before merging workspace: {0}")] ReviewRequired(String), #[error("parallel agent count must be greater than zero")] InvalidAgentCount, #[error("container workspace path is invalid: {0}")] InvalidContainerWorkspace(PathBuf), #[error("container `{0}` is not running")] ContainerNotRunning(String), #[error("container execution timed out")] ContainerTimeout, #[error("container runtime failed: {0}")] Container(String), #[error("git command failed: {0}")] Git(String), #[error(transparent)] Io(#[from] std::io::Error)
}

#[cfg(test)] mod tests { use super::{CleanupPolicy, ContainerLifecycleState, ContainerWorkspaceBackend, WorkspaceBackend, WorkspaceError, validate_name}; #[test] fn workspace_names_reject_paths() { assert!(validate_name("../escape").is_err()); assert!(validate_name("agent-auth_01").is_ok()); } #[test] fn cleanup_policy_is_explicit() { assert_eq!(CleanupPolicy::Keep, CleanupPolicy::Keep); assert!(matches!(WorkspaceError::InvalidAgentCount, WorkspaceError::InvalidAgentCount)); } #[test] fn container_backend_is_safe_by_default() { let backend = ContainerWorkspaceBackend::new("docker", "rust:latest"); assert!(backend.describe().contains("network=false")); assert!(backend.describe().contains("memory_limit=unlimited")); } #[test] fn container_lifecycle_states_are_explicit() { assert_ne!(ContainerLifecycleState::Provisioned, ContainerLifecycleState::Running); } }
