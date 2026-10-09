//! The bot action protocol: what a bot can do in one turn and how replies are parsed.

use serde::{Deserialize, Serialize};

use crate::{TaskId, board::TaskStatus};

/// A structured action a bot takes during its turn.
///
/// Every action is authorized against the bot's permission policy using
/// [`BotAction::permission`] before the engine applies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotAction {
    /// Post a message. Defaults to the channel the bot was addressed in.
    Post {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<String>,
        /// Reply in the thread rooted at this message id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<u64>,
    },
    /// Send a private message to another member.
    DirectMessage { to: String, text: String },
    /// Add a task to the board.
    CreateTask {
        title: String,
        #[serde(default)]
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assignee: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        depends_on: Vec<TaskId>,
    },
    /// Hand a task to a member.
    AssignTask { task: TaskId, assignee: String },
    /// Move a task through its lifecycle, optionally recording a result note.
    UpdateTask {
        task: TaskId,
        status: TaskStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Open a new channel.
    CreateChannel {
        name: String,
        #[serde(default)]
        purpose: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        members: Vec<String>,
    },
    /// Remember something privately across turns.
    Note { text: String },
    /// Escalate a question to the human operator.
    AskHuman { question: String },
    /// Pass the floor to a teammate with context.
    Handoff { to: String, summary: String },
    /// Propose that the mission's goal is met.
    ProposeCompletion { summary: String },
    /// Approve or reject a pending completion proposal.
    Vote {
        approve: bool,
        #[serde(default)]
        reason: String,
    },
    /// Nothing to add this turn.
    Pass,
}

impl BotAction {
    /// The permission action string checked before this action is applied.
    #[must_use]
    pub const fn permission(&self) -> &'static str {
        match self {
            Self::Post { .. } => "team.message.post",
            Self::DirectMessage { .. } => "team.message.direct",
            Self::CreateTask { .. } => "team.task.create",
            Self::AssignTask { .. } => "team.task.assign",
            Self::UpdateTask { .. } => "team.task.update",
            Self::CreateChannel { .. } => "team.channel.create",
            Self::Note { .. } => "team.note",
            Self::AskHuman { .. } => "team.human.ask",
            Self::Handoff { .. } => "team.handoff",
            Self::ProposeCompletion { .. } => "team.goal.propose",
            Self::Vote { .. } => "team.goal.vote",
            Self::Pass => "team.pass",
        }
    }

    /// Short label for logs and interfaces.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Post { .. } => "post",
            Self::DirectMessage { .. } => "direct_message",
            Self::CreateTask { .. } => "create_task",
            Self::AssignTask { .. } => "assign_task",
            Self::UpdateTask { .. } => "update_task",
            Self::CreateChannel { .. } => "create_channel",
            Self::Note { .. } => "note",
            Self::AskHuman { .. } => "ask_human",
            Self::Handoff { .. } => "handoff",
            Self::ProposeCompletion { .. } => "propose_completion",
            Self::Vote { .. } => "vote",
            Self::Pass => "pass",
        }
    }

    #[must_use]
    pub fn post(text: impl Into<String>) -> Self {
        Self::Post {
            text: text.into(),
            channel: None,
            thread: None,
        }
    }
}

/// Token usage attributed to a single turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl TurnUsage {
    #[must_use]
    pub const fn total(self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    pub fn add(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Everything a bot produced in one turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BotTurn {
    pub actions: Vec<BotAction>,
    pub usage: TurnUsage,
}

impl BotTurn {
    #[must_use]
    pub fn new(actions: Vec<BotAction>) -> Self {
        Self {
            actions,
            usage: TurnUsage::default(),
        }
    }

    #[must_use]
    pub fn pass() -> Self {
        Self::new(vec![BotAction::Pass])
    }
}

/// Fence language used for the structured action block in free-text replies.
pub const ACTIONS_FENCE: &str = "nexus-actions";

/// Parses a free-text model reply into actions.
///
/// The prose outside the fenced `nexus-actions` block becomes a `post` action;
/// the block itself holds a JSON array (or single object) of [`BotAction`]s.
/// Malformed blocks are reported as warnings rather than failing the turn, so
/// one bad reply never derails a mission.
#[must_use]
pub fn parse_reply(reply: &str) -> ParsedReply {
    let mut prose = String::new();
    let mut actions = Vec::new();
    let mut warnings = Vec::new();
    let mut rest = reply;
    let opener = format!("```{ACTIONS_FENCE}");
    while let Some(start) = rest.find(&opener) {
        prose.push_str(&rest[..start]);
        let after = &rest[start + opener.len()..];
        let (block, remainder) = match after.find("```") {
            Some(end) => (&after[..end], &after[end + 3..]),
            None => (after, ""),
        };
        match parse_action_block(block) {
            Ok(parsed) => actions.extend(parsed),
            Err(error) => warnings.push(format!("ignored malformed action block: {error}")),
        }
        rest = remainder;
    }
    prose.push_str(rest);
    let prose = prose.trim();
    let explicit_post = actions
        .iter()
        .any(|action| matches!(action, BotAction::Post { channel: None, .. }));
    if !prose.is_empty() && !explicit_post {
        actions.insert(0, BotAction::post(prose));
    }
    if actions.is_empty() {
        actions.push(BotAction::Pass);
    }
    ParsedReply { actions, warnings }
}

fn parse_action_block(block: &str) -> Result<Vec<BotAction>, serde_json::Error> {
    let block = block.trim();
    if block.is_empty() {
        return Ok(Vec::new());
    }
    if block.starts_with('[') {
        serde_json::from_str(block)
    } else {
        serde_json::from_str::<BotAction>(block).map(|action| vec![action])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedReply {
    pub actions: Vec<BotAction>,
    pub warnings: Vec<String>,
}

/// Protocol documentation injected into model prompts.
#[must_use]
pub fn protocol_guide() -> String {
    format!(
        r#"## How to act
Write your message to the team as plain text. It is posted to the channel you were addressed in. Mention teammates with @handle to bring them in.

To take structured actions, append ONE fenced block tagged `{ACTIONS_FENCE}` containing a JSON array:

```{ACTIONS_FENCE}
[
  {{"type": "create_task", "title": "Draft the API", "assignee": "ada", "depends_on": []}},
  {{"type": "update_task", "task": "T1", "status": "done", "note": "Merged in worktree"}}
]
```

Available actions:
- post {{text, channel?, thread?}} — message a channel (or a thread by message id)
- direct_message {{to, text}} — private message to one member
- create_task {{title, description?, assignee?, depends_on?}}
- assign_task {{task, assignee}}
- update_task {{task, status: todo|in_progress|blocked|review|done|cancelled, note?}}
- create_channel {{name, purpose?, members?}}
- note {{text}} — private memory, visible only to you in later turns
- ask_human {{question}} — escalate to the human operator
- handoff {{to, summary}} — give the floor to a teammate
- propose_completion {{summary}} — claim the goal is met (lead only by default)
- vote {{approve, reason}} — approvers vote on a pending proposal
- pass — nothing to add

Keep messages short and concrete. Do not repeat what teammates already said."#
    )
}

#[cfg(test)]
mod tests {
    use super::{BotAction, TurnUsage, parse_reply};
    use crate::{TaskId, board::TaskStatus};

    #[test]
    fn prose_becomes_a_post() {
        let parsed = parse_reply("Hello team, starting now.");
        assert_eq!(
            parsed.actions,
            vec![BotAction::post("Hello team, starting now.")]
        );
    }

    #[test]
    fn action_blocks_are_extracted() {
        let reply = "Plan below.\n```nexus-actions\n[{\"type\":\"create_task\",\"title\":\"Write tests\",\"assignee\":\"qa\"},{\"type\":\"update_task\",\"task\":\"T1\",\"status\":\"done\"}]\n```\nThanks!";
        let parsed = parse_reply(reply);
        assert!(parsed.warnings.is_empty());
        assert_eq!(parsed.actions.len(), 3);
        assert_eq!(parsed.actions[0], BotAction::post("Plan below.\n\nThanks!"));
        assert!(matches!(
            &parsed.actions[2],
            BotAction::UpdateTask {
                task: TaskId(1),
                status: TaskStatus::Done,
                ..
            }
        ));
    }

    #[test]
    fn single_object_blocks_are_accepted() {
        let parsed = parse_reply("```nexus-actions\n{\"type\":\"pass\"}\n```");
        assert_eq!(parsed.actions, vec![BotAction::Pass]);
    }

    #[test]
    fn malformed_blocks_warn_without_failing() {
        let parsed = parse_reply("Still here.\n```nexus-actions\n{not json}\n```");
        assert_eq!(parsed.warnings.len(), 1);
        assert_eq!(parsed.actions, vec![BotAction::post("Still here.")]);
    }

    #[test]
    fn empty_replies_pass() {
        assert_eq!(parse_reply("   ").actions, vec![BotAction::Pass]);
    }

    #[test]
    fn explicit_default_post_suppresses_prose_duplication() {
        let parsed = parse_reply(
            "ignored\n```nexus-actions\n{\"type\":\"post\",\"text\":\"explicit\"}\n```",
        );
        assert_eq!(parsed.actions, vec![BotAction::post("explicit")]);
    }

    #[test]
    fn actions_map_to_permissions() {
        assert_eq!(BotAction::Pass.permission(), "team.pass");
        assert_eq!(
            BotAction::ProposeCompletion {
                summary: String::new()
            }
            .permission(),
            "team.goal.propose"
        );
    }

    #[test]
    fn usage_accumulates() {
        let mut usage = TurnUsage::default();
        usage.add(TurnUsage {
            input_tokens: 3,
            output_tokens: 4,
        });
        assert_eq!(usage.total(), 7);
    }
}
