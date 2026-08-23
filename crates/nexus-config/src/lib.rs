//! Configuration contracts and layered resolution for Nexus.

use std::{env, fs, path::{Path, PathBuf}};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_agent_name")]
    pub default_agent: String,
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
    #[serde(default)]
    pub context: ContextConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    #[serde(default = "default_context_files")]
    pub max_files: usize,
    #[serde(default = "default_context_bytes")]
    pub max_bytes_per_file: usize,
    #[serde(default = "default_context_tokens")]
    pub token_budget: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_files: default_context_files(),
            max_bytes_per_file: default_context_bytes(),
            token_budget: default_context_tokens(),
        }
    }
}

fn default_agent_name() -> String { "nexus-engineer".to_owned() }
const fn default_max_steps() -> usize { 16 }
const fn default_context_files() -> usize { 64 }
const fn default_context_bytes() -> usize { 64 * 1024 }
const fn default_context_tokens() -> usize { 24_000 }

impl Default for Config {
    fn default() -> Self {
        Self {
            default_agent: default_agent_name(),
            max_steps: default_max_steps(),
            context: ContextConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub config: Config,
    pub sources: Vec<PathBuf>,
}

impl Config {
    pub fn load_from(root: impl AsRef<Path>) -> Result<ResolvedConfig, ConfigError> {
        let root = root.as_ref();
        let mut config = Config::default();
        let mut sources = Vec::new();

        if let Some(home) = dirs::config_dir() {
            let path = home.join("nexus").join("config.toml");
            merge_file(&mut config, &mut sources, &path)?;
        }
        merge_file(&mut config, &mut sources, &root.join(".nexus").join("config.toml"))?;

        if let Ok(value) = env::var("NEXUS_DEFAULT_AGENT") {
            config.default_agent = value;
        }
        if let Ok(value) = env::var("NEXUS_MAX_STEPS") {
            config.max_steps = value.parse().map_err(|_| ConfigError::InvalidEnv("NEXUS_MAX_STEPS".to_owned()))?;
        }
        Ok(ResolvedConfig { config, sources })
    }
}

fn merge_file(config: &mut Config, sources: &mut Vec<PathBuf>, path: &Path) -> Result<(), ConfigError> {
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(path).map_err(ConfigError::Io)?;
    let overlay: Config = toml::from_str(&raw).map_err(ConfigError::Toml)?;
    *config = overlay;
    sources.push(path.to_path_buf());
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration I/O failed: {0}")]
    Io(std::io::Error),
    #[error("invalid configuration: {0}")]
    Toml(toml::de::Error),
    #[error("invalid environment variable: {0}")]
    InvalidEnv(String),
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn defaults_are_stable() {
        let config = Config::default();
        assert_eq!(config.max_steps, 16);
        assert_eq!(config.context.token_budget, 24_000);
    }
}
