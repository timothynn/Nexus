//! Configuration contracts and resolution for Nexus.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_agent_name")]
    pub default_agent: String,
}

fn default_agent_name() -> String {
    "nexus-engineer".to_owned()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_agent: default_agent_name(),
        }
    }
}
