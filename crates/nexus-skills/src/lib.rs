//! Project-local skills, lifecycle hooks, and reusable agent templates.
//!
//! Skills live under `.nexus/skills/<name>/SKILL.md`. Agent templates live in
//! `.nexus/agents/<name>.toml`. Hook commands are explicit configuration and are
//! returned for the caller to execute through Nexus permissions rather than being
//! executed implicitly by this crate.

use std::{collections::BTreeMap, fs, path::{Path, PathBuf}};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub path: PathBuf,
    pub instructions: String,
}

pub fn discover_skills(root: impl AsRef<Path>) -> Result<Vec<Skill>, SkillError> {
    let directory = root.as_ref().join(".nexus").join("skills");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut skills = Vec::new();
    for entry in fs::read_dir(&directory).map_err(SkillError::Io)? {
        let entry = entry.map_err(SkillError::Io)?;
        if !entry.file_type().map_err(SkillError::Io)?.is_dir() {
            continue;
        }
        let path = entry.path().join("SKILL.md");
        if !path.is_file() {
            continue;
        }
        let instructions = fs::read_to_string(&path).map_err(SkillError::Io)?;
        let name = entry.file_name().to_string_lossy().to_string();
        skills.push(Skill { name, path, instructions });
    }
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(skills)
}

pub fn load_skill(root: impl AsRef<Path>, name: &str) -> Result<Skill, SkillError> {
    discover_skills(root)?
        .into_iter()
        .find(|skill| skill.name == name)
        .ok_or_else(|| SkillError::NotFound(name.to_owned()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    RunStarted,
    BeforeModel,
    BeforeTool,
    AfterTool,
    RunCompleted,
    RunFailed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookConfig {
    #[serde(default)]
    pub hooks: BTreeMap<String, Vec<String>>,
}

pub fn load_hooks(root: impl AsRef<Path>) -> Result<HookConfig, SkillError> {
    let path = root.as_ref().join(".nexus").join("hooks.toml");
    if !path.exists() {
        return Ok(HookConfig::default());
    }
    let raw = fs::read_to_string(path).map_err(SkillError::Io)?;
    toml::from_str(&raw).map_err(SkillError::Toml)
}

impl HookConfig {
    #[must_use]
    pub fn commands(&self, event: HookEvent) -> &[String] {
        self.hooks.get(event_key(event)).map_or(&[], Vec::as_slice)
    }
}

const fn event_key(event: HookEvent) -> &'static str {
    match event {
        HookEvent::RunStarted => "run_started",
        HookEvent::BeforeModel => "before_model",
        HookEvent::BeforeTool => "before_tool",
        HookEvent::AfterTool => "after_tool",
        HookEvent::RunCompleted => "run_completed",
        HookEvent::RunFailed => "run_failed",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub skills: Vec<String>,
}

pub fn load_agent_template(
    root: impl AsRef<Path>,
    name: &str,
) -> Result<AgentTemplate, SkillError> {
    let path = root
        .as_ref()
        .join(".nexus")
        .join("agents")
        .join(format!("{name}.toml"));
    if !path.exists() {
        return Err(SkillError::TemplateNotFound(name.to_owned()));
    }
    let raw = fs::read_to_string(path).map_err(SkillError::Io)?;
    toml::from_str(&raw).map_err(SkillError::Toml)
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill not found: {0}")]
    NotFound(String),
    #[error("agent template not found: {0}")]
    TemplateNotFound(String),
    #[error("skills I/O failed: {0}")]
    Io(std::io::Error),
    #[error("invalid skills configuration: {0}")]
    Toml(toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::{HookConfig, HookEvent};

    #[test]
    fn missing_hook_returns_empty_command_list() {
        let config = HookConfig::default();
        assert!(config.commands(HookEvent::BeforeTool).is_empty());
    }
}
