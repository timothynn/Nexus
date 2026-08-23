//! Repository discovery, instructions, Git-aware selection, and lightweight code search.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

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
        Self {
            max_files: 64,
            max_bytes_per_file: 64 * 1024,
            token_budget: 24_000,
        }
    }
}

pub fn discover(
    root: impl AsRef<Path>,
    options: &ContextOptions,
) -> Result<ContextSnapshot, ContextError> {
    let root = root.as_ref().canonicalize().map_err(ContextError::Io)?;
    let mut paths = Vec::new();
    collect_paths(&root, &root, &mut paths)?;
    paths.sort();
    build_snapshot(root, paths, options, None)
}

/// Prioritizes Git-modified and untracked files before the remaining repository.
pub fn discover_git_aware(
    root: impl AsRef<Path>,
    options: &ContextOptions,
) -> Result<GitAwareContextSnapshot, ContextError> {
    let root = root.as_ref().canonicalize().map_err(ContextError::Io)?;
    let changed = changed_files(&root);
    let mut paths = Vec::new();
    collect_paths(&root, &root, &mut paths)?;
    paths.sort_by(|left, right| {
        let left_changed = changed.contains(left.strip_prefix(&root).unwrap_or(left));
        let right_changed = changed.contains(right.strip_prefix(&root).unwrap_or(right));
        right_changed.cmp(&left_changed).then_with(|| left.cmp(right))
    });
    let snapshot = build_snapshot(root.clone(), paths, options, None)?;
    let prioritized_files = snapshot
        .files
        .iter()
        .filter(|file| changed.contains(&file.path))
        .map(|file| file.path.clone())
        .collect();
    Ok(GitAwareContextSnapshot {
        snapshot,
        prioritized_files,
        git_available: !changed.is_empty() || root.join(".git").exists(),
    })
}

fn build_snapshot(
    root: PathBuf,
    paths: Vec<PathBuf>,
    options: &ContextOptions,
    model: Option<&str>,
) -> Result<ContextSnapshot, ContextError> {
    let mut files = Vec::new();
    let mut total_estimated_tokens = 0;
    let mut truncated = false;
    for path in paths.into_iter().take(options.max_files) {
        let bytes = fs::read(&path).map_err(ContextError::Io)?;
        if bytes.len() > options.max_bytes_per_file || bytes.contains(&0) {
            truncated = true;
            continue;
        }
        let content = String::from_utf8_lossy(&bytes).to_string();
        let estimated_tokens = model.map_or_else(|| estimate_tokens(&content), |name| {
            estimate_tokens_for_model(&content, name)
        });
        if total_estimated_tokens.saturating_add(estimated_tokens) > options.token_budget {
            truncated = true;
            break;
        }
        total_estimated_tokens += estimated_tokens;
        files.push(ContextFile {
            path: path.strip_prefix(&root).unwrap_or(&path).to_path_buf(),
            content,
            estimated_tokens,
        });
    }
    Ok(ContextSnapshot {
        root,
        files,
        total_estimated_tokens,
        truncated,
    })
}

#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    estimate_tokens_for_model(text, "default")
}

/// Approximate token accounting with model-family profiles.
///
/// This is intentionally labelled an estimate; provider tokenizers remain the
/// source of truth for billing and exact context limits.
#[must_use]
pub fn estimate_tokens_for_model(text: &str, model: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let chars_per_token = if model.contains("gpt") || model.contains("o1") || model.contains("o3") {
        3.7
    } else if model.contains("claude") {
        3.8
    } else if model.contains("gemini") {
        4.0
    } else {
        4.0
    };
    (text.chars().count() as f64 / chars_per_token).ceil() as usize
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionDocument {
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct InstructionSet {
    pub documents: Vec<InstructionDocument>,
    pub agent_instructions: Option<String>,
}

impl InstructionSet {
    #[must_use]
    pub fn combined(&self) -> String {
        let mut sections = self
            .documents
            .iter()
            .map(|document| format!("## Instructions from {}\n{}", document.path.display(), document.content))
            .collect::<Vec<_>>();
        if let Some(instructions) = &self.agent_instructions {
            if !instructions.trim().is_empty() {
                sections.push(format!("## Agent instructions\n{instructions}"));
            }
        }
        sections.join("\n\n")
    }
}

/// Loads root-to-leaf AGENTS.md files plus project and agent-specific instructions.
pub fn discover_instructions(
    root: impl AsRef<Path>,
    target: impl AsRef<Path>,
    agent_instructions: Option<String>,
) -> Result<InstructionSet, ContextError> {
    let root = root.as_ref().canonicalize().map_err(ContextError::Io)?;
    let target = target.as_ref();
    let mut documents = Vec::new();
    let project = root.join(".nexus").join("instructions.md");
    if project.is_file() {
        documents.push(InstructionDocument {
            path: PathBuf::from(".nexus/instructions.md"),
            content: fs::read_to_string(project).map_err(ContextError::Io)?,
        });
    }

    let target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        root.join(target)
    };
    let directory = if target.is_dir() {
        target
    } else {
        target.parent().unwrap_or(&root).to_path_buf()
    };
    let relative = directory.strip_prefix(&root).unwrap_or(Path::new(""));
    let mut current = root.clone();
    append_agents_file(&mut documents, &root, &current)?;
    for component in relative.components() {
        current.push(component.as_os_str());
        append_agents_file(&mut documents, &root, &current)?;
    }
    Ok(InstructionSet {
        documents,
        agent_instructions,
    })
}

fn append_agents_file(
    documents: &mut Vec<InstructionDocument>,
    root: &Path,
    directory: &Path,
) -> Result<(), ContextError> {
    let path = directory.join("AGENTS.md");
    if path.is_file() {
        documents.push(InstructionDocument {
            path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
            content: fs::read_to_string(path).map_err(ContextError::Io)?,
        });
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct GitAwareContextSnapshot {
    pub snapshot: ContextSnapshot,
    pub prioritized_files: Vec<PathBuf>,
    pub git_available: bool,
}

fn changed_files(root: &Path) -> HashSet<PathBuf> {
    let mut changed = HashSet::new();
    for args in [
        ["diff", "--name-only", "HEAD"].as_slice(),
        ["diff", "--name-only", "--cached"].as_slice(),
        ["ls-files", "--others", "--exclude-standard"].as_slice(),
    ] {
        let Ok(output) = Command::new("git").current_dir(root).args(args).output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if !line.trim().is_empty() {
                changed.insert(PathBuf::from(line));
            }
        }
    }
    changed
}

#[derive(Debug, Clone)]
pub struct CodeIndex {
    entries: Vec<CodeIndexEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeIndexEntry {
    pub path: PathBuf,
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSearchMatch {
    pub path: PathBuf,
    pub line: usize,
    pub text: String,
    pub score: usize,
}

impl CodeIndex {
    #[must_use]
    pub fn build(snapshot: &ContextSnapshot) -> Self {
        let mut entries = Vec::new();
        for file in &snapshot.files {
            for (index, line) in file.content.lines().enumerate() {
                let text = line.trim();
                if text.is_empty() {
                    continue;
                }
                entries.push(CodeIndexEntry {
                    path: file.path.clone(),
                    line: index + 1,
                    text: text.to_owned(),
                });
            }
        }
        Self { entries }
    }

    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<CodeSearchMatch> {
        let terms = query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let mut matches = self
            .entries
            .iter()
            .filter_map(|entry| {
                let haystack = entry.text.to_ascii_lowercase();
                let score = terms.iter().filter(|term| haystack.contains(term.as_str())).count();
                (score > 0).then(|| CodeSearchMatch {
                    path: entry.path.clone(),
                    line: entry.line,
                    text: entry.text.clone(),
                    score,
                })
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.line.cmp(&right.line))
        });
        matches.truncate(limit);
        matches
    }
}

fn collect_paths(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), ContextError> {
    for entry in fs::read_dir(directory).map_err(ContextError::Io)? {
        let entry = entry.map_err(ContextError::Io)?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if ignored(relative) {
            continue;
        }
        let file_type = entry.file_type().map_err(ContextError::Io)?;
        if file_type.is_dir() {
            collect_paths(root, &path, paths)?;
        } else if file_type.is_file() {
            paths.push(path);
        }
    }
    Ok(())
}

fn ignored(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(".git" | ".nexus" | "target" | "node_modules")
        )
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("context discovery failed: {0}")]
    Io(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::{CodeIndex, ContextFile, ContextSnapshot, estimate_tokens, estimate_tokens_for_model};
    use std::path::PathBuf;

    #[test]
    fn estimates_non_empty_text() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert!(estimate_tokens_for_model("abcde", "gpt-5") >= 1);
    }

    #[test]
    fn search_returns_ranked_matches() {
        let snapshot = ContextSnapshot {
            root: PathBuf::from("."),
            files: vec![ContextFile {
                path: PathBuf::from("src/lib.rs"),
                content: "pub fn nexus_runtime() {}\nfn other() {}".to_owned(),
                estimated_tokens: 1,
            }],
            total_estimated_tokens: 1,
            truncated: false,
        };
        let matches = CodeIndex::build(&snapshot).search("nexus runtime", 5);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line, 1);
    }
}
