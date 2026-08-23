//! Isolated workspaces backed by Git worktrees.
//!
//! Nexus treats a worktree as an execution workspace rather than a temporary
//! shell trick. Agent runs can therefore be isolated, inspected, and cleaned up
//! without automatically merging changes into the user's primary checkout.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitWorktree {
    pub name: String,
    pub branch: String,
    pub path: PathBuf,
}

pub struct GitWorktreeManager {
    repository: PathBuf,
    worktrees_root: PathBuf,
}

impl GitWorktreeManager {
    pub fn new(repository: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let repository = repository
            .into()
            .canonicalize()
            .map_err(WorkspaceError::Io)?;
        let manager = Self {
            worktrees_root: repository.join(".nexus").join("worktrees"),
            repository,
        };
        manager.ensure_repository()?;
        Ok(manager)
    }

    #[must_use]
    pub fn repository(&self) -> &Path {
        &self.repository
    }

    #[must_use]
    pub fn worktrees_root(&self) -> &Path {
        &self.worktrees_root
    }

    pub fn create(&self, name: &str, base: Option<&str>) -> Result<GitWorktree, WorkspaceError> {
        validate_name(name)?;
        fs::create_dir_all(&self.worktrees_root).map_err(WorkspaceError::Io)?;

        let path = self.worktrees_root.join(name);
        if path.exists() {
            return Err(WorkspaceError::AlreadyExists(name.to_owned()));
        }

        let branch = format!("nexus/{name}");
        let base = base.unwrap_or("HEAD");
        let path_string = path.to_string_lossy().to_string();
        self.git(&[
            "worktree",
            "add",
            "-b",
            branch.as_str(),
            path_string.as_str(),
            base,
        ])?;

        Ok(GitWorktree {
            name: name.to_owned(),
            branch,
            path,
        })
    }

    pub fn list(&self) -> Result<Vec<GitWorktree>, WorkspaceError> {
        let output = self.git(&["worktree", "list", "--porcelain"])?;
        let mut worktrees = Vec::new();
        let mut path = None;
        let mut branch = None;

        for line in output.lines().chain(std::iter::once("")) {
            if line.is_empty() {
                if let (Some(path), Some(branch)) = (path.take(), branch.take()) {
                    let path = PathBuf::from(path);
                    if let Ok(relative) = path.strip_prefix(&self.worktrees_root) {
                        if relative.components().count() == 1 {
                            worktrees.push(GitWorktree {
                                name: relative.to_string_lossy().to_string(),
                                branch,
                                path,
                            });
                        }
                    }
                }
                continue;
            }

            if let Some(value) = line.strip_prefix("worktree ") {
                path = Some(value.to_owned());
            } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
                branch = Some(value.to_owned());
            }
        }

        worktrees.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(worktrees)
    }

    pub fn diff(&self, name: &str) -> Result<String, WorkspaceError> {
        let worktree = self.find(name)?;
        self.git_in(&worktree.path, &["diff", "--", "."])
    }

    pub fn status(&self, name: &str) -> Result<String, WorkspaceError> {
        let worktree = self.find(name)?;
        self.git_in(&worktree.path, &["status", "--short"])
    }

    pub fn remove(&self, name: &str, force: bool) -> Result<(), WorkspaceError> {
        let worktree = self.find(name)?;
        let path = worktree.path.to_string_lossy().to_string();
        if force {
            self.git(&["worktree", "remove", "--force", path.as_str()])?;
        } else {
            self.git(&["worktree", "remove", path.as_str()])?;
        }
        Ok(())
    }

    fn find(&self, name: &str) -> Result<GitWorktree, WorkspaceError> {
        self.list()?
            .into_iter()
            .find(|worktree| worktree.name == name)
            .ok_or_else(|| WorkspaceError::NotFound(name.to_owned()))
    }

    fn ensure_repository(&self) -> Result<(), WorkspaceError> {
        self.git(&["rev-parse", "--is-inside-work-tree"])
            .map(|_| ())
    }

    fn git(&self, args: &[&str]) -> Result<String, WorkspaceError> {
        self.git_in(&self.repository, args)
    }

    fn git_in(&self, directory: &Path, args: &[&str]) -> Result<String, WorkspaceError> {
        let output = Command::new("git")
            .current_dir(directory)
            .args(args)
            .output()
            .map_err(WorkspaceError::Io)?;

        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(WorkspaceError::Git(message));
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_owned())
    }
}

fn validate_name(name: &str) -> Result<(), WorkspaceError> {
    let valid = !name.is_empty()
        && !matches!(name, "." | "..")
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        });

    if !valid {
        return Err(WorkspaceError::InvalidName(name.to_owned()));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("workspace name is invalid: {0}")]
    InvalidName(String),
    #[error("workspace already exists: {0}")]
    AlreadyExists(String),
    #[error("workspace not found: {0}")]
    NotFound(String),
    #[error("git command failed: {0}")]
    Git(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::validate_name;

    #[test]
    fn workspace_names_reject_path_separators_and_dot_paths() {
        assert!(validate_name("agent/auth").is_err());
        assert!(validate_name("../escape").is_err());
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("agent-auth_01").is_ok());
    }
}
