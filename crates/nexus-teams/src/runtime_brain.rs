//! A [`BotBrain`] backed by the Nexus agent runtime and any model provider.

use std::sync::Arc;

use async_trait::async_trait;
use nexus_config::Config;
use nexus_models::ModelProvider;
use nexus_permissions::{PermissionDecision, RuleBasedPolicy};
use nexus_runtime::{AgentRuntime, AuditEvent, AuditSink, AuthorizedToolExecutor};
use nexus_tools::ToolRegistry;
use tokio_util::sync::CancellationToken;

use crate::{
    ActivityReporter, BotBrain, BotProfile, BotTurn, TeamError, TurnContext, TurnUsage,
    protocol::parse_reply,
};

/// Resolves the provider and model for a bot (honouring per-bot overrides).
pub type ProviderFactory =
    Arc<dyn Fn(&BotProfile) -> Result<(Arc<dyn ModelProvider>, String), TeamError> + Send + Sync>;

/// Builds the permissioned workspace tool executor for a bot.
pub type ExecutorFactory =
    Arc<dyn Fn(&BotProfile) -> Result<AuthorizedToolExecutor, TeamError> + Send + Sync>;

/// Model-backed brain: renders the turn prompt, runs the bounded tool loop, and
/// parses the reply through the action protocol.
pub struct RuntimeBrain {
    config: Config,
    providers: ProviderFactory,
    executors: Option<ExecutorFactory>,
    max_steps: usize,
    history_token_budget: usize,
}

impl RuntimeBrain {
    #[must_use]
    pub fn new(config: Config, providers: ProviderFactory) -> Self {
        let max_steps = config.max_steps.max(1);
        Self {
            config,
            providers,
            executors: None,
            max_steps,
            history_token_budget: 6_000,
        }
    }

    /// Enables workspace tools for bots that list them in their profile.
    #[must_use]
    pub fn with_executors(mut self, executors: ExecutorFactory) -> Self {
        self.executors = Some(executors);
        self
    }

    #[must_use]
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps.max(1);
        self
    }

    #[must_use]
    pub fn with_history_token_budget(mut self, budget: usize) -> Self {
        self.history_token_budget = budget;
        self
    }

    fn executor_for(
        &self,
        bot: &BotProfile,
        tool_calling: bool,
        activity: &ActivityReporter,
    ) -> Result<AuthorizedToolExecutor, TeamError> {
        match &self.executors {
            Some(factory) if !bot.tools.is_empty() && tool_calling => factory(bot),
            Some(_) if !bot.tools.is_empty() => {
                activity.report(
                    "tools.unavailable",
                    "provider does not support tool calling; continuing without workspace tools",
                );
                Ok(no_tools())
            }
            _ => Ok(no_tools()),
        }
    }
}

fn no_tools() -> AuthorizedToolExecutor {
    AuthorizedToolExecutor::new(
        ToolRegistry::new(),
        Arc::new(RuleBasedPolicy::new(PermissionDecision::Deny)),
    )
}

/// Forwards runtime audit events (model requests, tool calls) as bot activity.
struct ActivityAudit(ActivityReporter);

impl AuditSink for ActivityAudit {
    fn record(&self, event: AuditEvent) {
        let detail = match event.kind.as_str() {
            "tool.requested" | "tool.completed" | "tool.failed" => format!(
                "{} {}",
                event.payload["name"].as_str().unwrap_or("tool"),
                summarize(&event.payload)
            ),
            _ => summarize(&event.payload),
        };
        self.0.report(event.kind, detail);
    }
}

fn summarize(payload: &serde_json::Value) -> String {
    let text = payload.to_string();
    if text.chars().count() > 160 {
        format!("{}…", text.chars().take(160).collect::<String>())
    } else {
        text
    }
}

#[async_trait]
impl BotBrain for RuntimeBrain {
    async fn take_turn(
        &self,
        context: TurnContext,
        cancellation: CancellationToken,
    ) -> Result<BotTurn, TeamError> {
        let (provider, model) = (self.providers)(&context.bot)?;
        let tool_calling = provider.capabilities().tool_calling;
        let executor = self.executor_for(&context.bot, tool_calling, &context.activity)?;
        let runtime = AgentRuntime::new(self.config.clone(), provider, model);
        let (system, user) = context.prompt(self.history_token_budget);
        let audit = ActivityAudit(context.activity.clone());
        let result = runtime
            .run_with_tools_controlled_with_instructions(
                &user,
                Some(&system),
                &executor,
                self.max_steps,
                cancellation,
                Some(&audit),
            )
            .await
            .map_err(|error| match error {
                nexus_runtime::RuntimeError::Cancelled => TeamError::Cancelled,
                other => TeamError::Brain(other.to_string()),
            })?;
        let parsed = parse_reply(&result.message);
        for warning in parsed.warnings {
            context.activity.report("protocol.warning", warning);
        }
        Ok(BotTurn {
            actions: parsed.actions,
            usage: TurnUsage {
                input_tokens: result.usage.input_tokens,
                output_tokens: result.usage.output_tokens,
            },
        })
    }
}
