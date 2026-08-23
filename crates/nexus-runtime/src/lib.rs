//! Agent execution loop. Concrete models and tools plug into this layer later.

use nexus_config::Config;
use nexus_core::Task;

#[derive(Debug, Clone)]
pub struct RunResult {
    pub message: String,
}

pub struct AgentRuntime {
    config: Config,
}

impl AgentRuntime {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn run(&self, prompt: &str) -> RunResult {
        let task = Task::new(prompt);
        RunResult {
            message: format!(
                "Nexus [{}] accepted task {}: {}",
                self.config.default_agent, task.id, task.prompt
            ),
        }
    }
}
