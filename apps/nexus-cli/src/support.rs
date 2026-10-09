//! Shared CLI plumbing: providers, approvals, tool executors, instructions, storage.

use std::{
    fs,
    io::{self, Write},
    path::Path,
    sync::Arc,
};

use anyhow::{Result, bail};
use clap::Args;
use nexus_context::{ContextOptions, discover_git_aware, discover_instructions};
use nexus_models::{ModelProvider, provider_from_name};
use nexus_permissions::{
    PermissionApprover, PermissionDecision, PermissionRequest, RuleBasedPolicy,
};
use nexus_runtime::AuthorizedToolExecutor;
use nexus_skills::{load_agent_template, load_skill};
use nexus_storage::SqliteStore;
use nexus_tools::{FileSystemTool, ShellTool, ToolRegistry};

/// Built-in workspace tools that can be granted to agents and bots.
pub const WORKSPACE_TOOLS: [&str; 2] = ["filesystem.read", "shell.execute"];

/// Model provider selection shared by every model-backed command.
#[derive(Debug, Clone, Args)]
pub struct ProviderArgs {
    /// Model identifier passed to the provider.
    #[arg(short, long, default_value = "mock-1")]
    pub model: String,
    /// Provider: `mock` or `openai-compatible`.
    #[arg(long, default_value = "mock")]
    pub provider: String,
    /// Base URL for OpenAI-compatible providers.
    #[arg(long, default_value = "https://api.openai.com/v1")]
    pub base_url: String,
    /// Environment variable holding the API key.
    #[arg(long, default_value = "OPENAI_API_KEY")]
    pub api_key_env: String,
}

impl ProviderArgs {
    pub fn build(&self) -> Result<Arc<dyn ModelProvider>> {
        build_provider(&self.provider, &self.base_url, &self.api_key_env)
    }
}

pub fn build_provider(
    provider_name: &str,
    base_url: &str,
    api_key_env: &str,
) -> Result<Arc<dyn ModelProvider>> {
    Ok(provider_from_name(provider_name, base_url, api_key_env)?)
}

/// Asks on stderr/stdin before an `ask` permission is granted.
pub struct StdinApprover;

impl PermissionApprover for StdinApprover {
    fn approve(&self, request: &PermissionRequest) -> bool {
        eprint!("[nexus] allow `{}`? [y/N] ", request.action);
        if io::stderr().flush().is_err() {
            return false;
        }
        let mut answer = String::new();
        match io::stdin().read_line(&mut answer) {
            Ok(_) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
            Err(_) => false,
        }
    }
}

/// Grants every `ask` decision (`--yes`).
pub struct ApproveAll;

impl PermissionApprover for ApproveAll {
    fn approve(&self, _request: &PermissionRequest) -> bool {
        true
    }
}

/// Rejects every `ask` decision (used when stdin is reserved for something else).
pub struct DenyAll;

impl PermissionApprover for DenyAll {
    fn approve(&self, _request: &PermissionRequest) -> bool {
        false
    }
}

/// How `ask` permission decisions are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Prompt,
    Always,
    Never,
}

impl Approval {
    #[must_use]
    pub fn approver(self) -> Arc<dyn PermissionApprover> {
        match self {
            Self::Prompt => Arc::new(StdinApprover),
            Self::Always => Arc::new(ApproveAll),
            Self::Never => Arc::new(DenyAll),
        }
    }
}

/// Builds a permissioned executor rooted at `root` exposing only `tools`.
///
/// Reads are allowed; process execution always asks.
pub fn build_tool_executor(
    root: &Path,
    tools: &[&str],
    approval: Approval,
) -> Result<AuthorizedToolExecutor> {
    let mut registry = ToolRegistry::new();
    for tool in tools {
        match *tool {
            "filesystem.read" => registry.register(Arc::new(FileSystemTool::new(root)))?,
            "shell.execute" => registry.register(Arc::new(ShellTool::new(root)))?,
            other => bail!(
                "unknown workspace tool `{other}`; available: {}",
                WORKSPACE_TOOLS.join(", ")
            ),
        }
    }
    let policy = RuleBasedPolicy::new(PermissionDecision::Deny)
        .with_rule("filesystem.read", PermissionDecision::Allow)
        .with_rule("shell.execute", PermissionDecision::Ask);
    Ok(AuthorizedToolExecutor::new(registry, Arc::new(policy)).with_approver(approval.approver()))
}

pub fn session_store(root: &Path) -> Result<Arc<SqliteStore>> {
    let directory = root.join(".nexus");
    fs::create_dir_all(&directory)?;
    Ok(Arc::new(SqliteStore::open(directory.join("nexus.db"))?))
}

pub fn resolved_instructions(
    root: &Path,
    target: &Path,
    agent_template: Option<&str>,
    skill_names: &[String],
    git_context: bool,
) -> Result<String> {
    let mut template_instructions = None;
    let mut selected_skills = skill_names.to_vec();
    if let Some(template_name) = agent_template {
        let template = load_agent_template(root, template_name)?;
        template_instructions = Some(template.instructions);
        selected_skills.extend(template.skills);
    }
    selected_skills.sort();
    selected_skills.dedup();
    let mut instruction_set = discover_instructions(root, target, template_instructions)?;
    for name in selected_skills {
        let skill = load_skill(root, &name)?;
        instruction_set
            .documents
            .push(nexus_context::InstructionDocument {
                path: skill
                    .path
                    .strip_prefix(root)
                    .unwrap_or(&skill.path)
                    .to_path_buf(),
                content: format!("# Skill: {}\n{}", skill.name, skill.instructions),
            });
    }
    let mut combined = instruction_set.combined();
    if git_context {
        let snapshot = discover_git_aware(root, &ContextOptions::default())?;
        let files = snapshot
            .prioritized_files
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        if !files.is_empty() {
            combined.push_str("\n\n## Git-aware priority\nPrioritize these changed or untracked files when investigating the task:\n");
            combined.push_str(&files.join("\n"));
        }
    }
    Ok(combined)
}
