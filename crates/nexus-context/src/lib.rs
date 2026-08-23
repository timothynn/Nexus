//! Repository discovery and deterministic context budgeting.

use std::{fs, path::{Path, PathBuf}};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct ContextSnapshot {
    pub root: PathBuf,
    pub files: Vec<ContextFile>,
    pub total_estimated_tokens: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct ContextOptions {
    pub max_files: usize,
    pub max_bytes_per_file: usize,
    pub token_budget: usize,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self { max_files: 64, max_bytes_per_file: 64 * 1024, token_budget: 24_000 }
    }
}

pub fn discover(root: impl AsRef<Path>, options: &ContextOptions) -> Result<ContextSnapshot, ContextError> {
    let root = root.as_ref().canonicalize().map_err(ContextError::Io)?;
    let mut paths = Vec::new();
    collect_paths(&root, &root, &mut paths)?;
    paths.sort();
    let mut files = Vec::new();
    let mut total_estimated_tokens = 0;
    let mut truncated = false;
    for path in paths.into_iter().take(options.max_files) {
        let bytes = fs::read(&path).map_err(ContextError::Io)?;
        if bytes.len() > options.max_bytes_per_file || bytes.contains(&0) { truncated = true; continue; }
        let content = String::from_utf8_lossy(&bytes).to_string();
        let estimated_tokens = estimate_tokens(&content);
        if total_estimated_tokens.saturating_add(estimated_tokens) > options.token_budget { truncated = true; break; }
        total_estimated_tokens += estimated_tokens;
        files.push(ContextFile { path: path.strip_prefix(&root).unwrap_or(&path).to_path_buf(), content, estimated_tokens });
    }
    Ok(ContextSnapshot { root, files, total_estimated_tokens, truncated })
}

pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() { 0 } else { text.len().div_ceil(4) }
}

fn collect_paths(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), ContextError> {
    for entry in fs::read_dir(directory).map_err(ContextError::Io)? {
        let entry = entry.map_err(ContextError::Io)?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if ignored(relative) { continue; }
        let file_type = entry.file_type().map_err(ContextError::Io)?;
        if file_type.is_dir() { collect_paths(root, &path, paths)?; }
        else if file_type.is_file() { paths.push(path); }
    }
    Ok(())
}

fn ignored(path: &Path) -> bool {
    path.components().any(|component| matches!(component.as_os_str().to_str(), Some(".git" | ".nexus" | "target" | "node_modules")))
}

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("context discovery failed: {0}")] Io(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::estimate_tokens;
    #[test]
    fn estimates_non_empty_text() { assert_eq!(estimate_tokens("abcd"), 1); assert_eq!(estimate_tokens("abcde"), 2); }
}
