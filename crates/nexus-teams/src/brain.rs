//! How bots think: the [`BotBrain`] contract plus offline implementations.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use nexus_context::estimate_tokens;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    BotAction, BotProfile, BotTurn, Message, MessageKind, MissionState, TaskStatus, TeamError,
    TurnUsage,
    context::render_turn_prompt,
    floor::{expertise_overlap, tokenize},
    roster::{HUMAN, SYSTEM},
};

/// Reports in-turn workspace activity (tool calls, model requests) to the engine.
#[derive(Debug, Clone)]
pub struct ActivityReporter {
    bot: String,
    sender: Option<mpsc::UnboundedSender<(String, String, String)>>,
}

impl ActivityReporter {
    #[must_use]
    pub fn new(
        bot: impl Into<String>,
        sender: mpsc::UnboundedSender<(String, String, String)>,
    ) -> Self {
        Self {
            bot: bot.into(),
            sender: Some(sender),
        }
    }

    /// A reporter that drops everything (for tests and embedding without observers).
    #[must_use]
    pub fn disabled(bot: impl Into<String>) -> Self {
        Self {
            bot: bot.into(),
            sender: None,
        }
    }

    pub fn report(&self, kind: impl Into<String>, detail: impl Into<String>) {
        if let Some(sender) = &self.sender {
            let _ = sender.send((self.bot.clone(), kind.into(), detail.into()));
        }
    }
}

/// Everything a bot needs to take one turn.
#[derive(Debug, Clone)]
pub struct TurnContext {
    pub bot: BotProfile,
    /// Why the floor policy gave this bot the floor.
    pub reason: String,
    /// Channel the bot's plain replies are posted to.
    pub channel: String,
    /// Snapshot of the mission when the turn started.
    pub state: Arc<MissionState>,
    pub activity: ActivityReporter,
}

impl TurnContext {
    #[must_use]
    pub fn is_lead(&self) -> bool {
        self.state.team.lead_handle() == self.bot.handle
    }

    /// Renders the (system, user) prompt pair for model-backed brains.
    #[must_use]
    pub fn prompt(&self, history_token_budget: usize) -> (String, String) {
        render_turn_prompt(self, history_token_budget)
    }
}

/// Decides a bot's actions for one turn.
#[async_trait]
pub trait BotBrain: Send + Sync {
    async fn take_turn(
        &self,
        context: TurnContext,
        cancellation: CancellationToken,
    ) -> Result<BotTurn, TeamError>;
}

/// Deterministic brain that simulates a competent team without any model.
///
/// It exercises the full protocol — planning, task assignment, progress
/// updates, handoffs, completion proposals, and votes — which makes it useful
/// for demos, offline development, and end-to-end tests of the engine.
#[derive(Debug, Default, Clone, Copy)]
pub struct SimulatedBrain;

#[async_trait]
impl BotBrain for SimulatedBrain {
    async fn take_turn(
        &self,
        context: TurnContext,
        cancellation: CancellationToken,
    ) -> Result<BotTurn, TeamError> {
        if cancellation.is_cancelled() {
            return Err(TeamError::Cancelled);
        }
        let actions = if context.is_lead() {
            lead_actions(&context)
        } else {
            member_actions(&context)
        };
        let (system, user) = context.prompt(4_000);
        let output = actions
            .iter()
            .map(|action| serde_json::to_string(action).unwrap_or_default())
            .collect::<String>();
        let usage = TurnUsage {
            input_tokens: (estimate_tokens(&system) + estimate_tokens(&user)) as u64,
            output_tokens: estimate_tokens(&output) as u64,
        };
        context
            .activity
            .report("simulate", format!("{} action(s)", actions.len()));
        Ok(BotTurn { actions, usage })
    }
}

fn lead_actions(context: &TurnContext) -> Vec<BotAction> {
    let state = &context.state;
    let lead = &context.bot.handle;
    if state.proposal.is_some() {
        return vec![BotAction::Pass];
    }
    if state.board.is_empty() {
        return plan(context);
    }
    if let Some(feedback) = human_feedback(state, lead) {
        return follow_up(state, lead, feedback);
    }
    if state.board.all_done() {
        let results = state
            .board
            .tasks()
            .filter(|task| task.status == TaskStatus::Done)
            .map(|task| {
                format!(
                    "{} {}: {}",
                    task.id,
                    task.title,
                    task.note.as_deref().unwrap_or("done")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        return vec![
            BotAction::post(format!(
                "All tasks are closed. Proposing completion of: {}",
                state.goal.statement
            )),
            BotAction::ProposeCompletion {
                summary: format!("{} — {results}", state.goal.statement),
            },
        ];
    }
    let mentions = state.unseen_mentions(lead);
    let done = state.board.count(TaskStatus::Done);
    let total = state.board.len();
    if let Some(message) = mentions.last() {
        let progress = format!("Progress is {done}/{total} tasks done; carry on with the board.");
        return vec![BotAction::post(if message.author == SYSTEM {
            format!("On it. {progress}")
        } else {
            format!("Thanks @{}. {progress}", message.author)
        })];
    }
    let idle = state
        .board
        .tasks()
        .filter(|task| task.status.is_open() && task.assignee.is_none())
        .map(|task| task.id)
        .collect::<Vec<_>>();
    if let (Some(task), Some(member)) = (
        idle.first(),
        state
            .team
            .members
            .iter()
            .find(|member| &member.handle != lead),
    ) {
        return vec![BotAction::AssignTask {
            task: *task,
            assignee: member.handle.clone(),
        }];
    }
    vec![BotAction::Pass]
}

/// The latest unseen operator message addressed to the lead (or to nobody in particular).
fn human_feedback<'s>(state: &'s MissionState, lead: &str) -> Option<&'s Message> {
    state.unseen_by(lead).into_iter().rev().find(|message| {
        message.author == HUMAN
            && matches!(message.kind, MessageKind::Chat | MessageKind::Direct)
            && (message.mentions.is_empty() || message.mentions(lead))
    })
}

/// Turns operator feedback into a tracked follow-up task for the best-placed member.
fn follow_up(state: &MissionState, lead: &str, feedback: &Message) -> Vec<BotAction> {
    let request = feedback.text.trim();
    let owner = feedback
        .mentions
        .iter()
        .find(|handle| handle.as_str() != lead && state.team.is_member(handle))
        .cloned()
        .unwrap_or_else(|| best_owner(state, lead, request));
    vec![
        BotAction::post(format!(
            "@human noted. @{owner} will handle it as a follow-up."
        )),
        BotAction::CreateTask {
            title: format!("Follow-up: {}", clip(request, 60)),
            description: request.to_owned(),
            assignee: Some(owner),
            depends_on: Vec::new(),
        },
    ]
}

/// The member whose expertise best matches `text` (first wins ties); the lead works solo.
fn best_owner(state: &MissionState, lead: &str, text: &str) -> String {
    let words = tokenize(text).into_iter().collect::<BTreeSet<_>>();
    let mut best: Option<(&str, usize)> = None;
    for member in state
        .team
        .members
        .iter()
        .filter(|member| member.handle != lead)
    {
        let score = expertise_overlap(member, &words);
        if best.is_none_or(|(_, top)| score > top) {
            best = Some((&member.handle, score));
        }
    }
    best.map_or_else(|| lead.to_owned(), |(handle, _)| handle.to_owned())
}

/// The first line of `text`, shortened to `max` characters.
fn clip(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() <= max {
        line.to_owned()
    } else {
        let kept = line.chars().take(max.saturating_sub(1)).collect::<String>();
        format!("{}…", kept.trim_end())
    }
}

fn plan(context: &TurnContext) -> Vec<BotAction> {
    let state = &context.state;
    let lead = &context.bot.handle;
    let members = state
        .team
        .members
        .iter()
        .filter(|member| &member.handle != lead)
        .collect::<Vec<_>>();
    if members.is_empty() {
        return vec![
            BotAction::post(format!("Working solo on: {}", state.goal.statement)),
            BotAction::CreateTask {
                title: state.goal.statement.clone(),
                description: String::new(),
                assignee: Some(lead.clone()),
                depends_on: Vec::new(),
            },
        ];
    }
    let is_gatekeeper = |member: &BotProfile| {
        matches!(
            member.archetype.as_deref(),
            Some("reviewer" | "tester" | "critic")
        )
    };
    let builders = members
        .iter()
        .filter(|member| !is_gatekeeper(member))
        .count();
    let mut actions = vec![BotAction::post(format!(
        "Plan for \"{}\": {}. Shout if anything is unclear.",
        state.goal.statement,
        members
            .iter()
            .map(|member| format!("@{} takes {}", member.handle, focus(member)))
            .collect::<Vec<_>>()
            .join(", ")
    ))];
    let mut next = state.board.next_id().0;
    let mut builder_ids = Vec::new();
    for member in &members {
        let gate = is_gatekeeper(member) && builders > 0;
        actions.push(BotAction::CreateTask {
            title: format!(
                "{} for: {}",
                capitalize(&focus(member)),
                state.goal.statement
            ),
            description: format!("Owned by {}", member.label()),
            assignee: Some(member.handle.clone()),
            depends_on: if gate {
                builder_ids.clone()
            } else {
                Vec::new()
            },
        });
        if !gate {
            builder_ids.push(crate::TaskId(next));
        }
        next += 1;
    }
    actions
}

fn focus(member: &BotProfile) -> String {
    match member.archetype.as_deref() {
        Some("architect") => "the design".to_owned(),
        Some("engineer") => "the implementation".to_owned(),
        Some("researcher") => "the research".to_owned(),
        Some("reviewer") => "the review".to_owned(),
        Some("tester") => "verification".to_owned(),
        Some("writer") => "the write-up".to_owned(),
        Some("critic") => "the risk analysis".to_owned(),
        Some("designer") => "the user experience".to_owned(),
        Some("analyst") => "the metrics".to_owned(),
        _ if !member.role.is_empty() => format!("the {} work", member.role.to_lowercase()),
        _ => "their part".to_owned(),
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn member_actions(context: &TurnContext) -> Vec<BotAction> {
    let state = &context.state;
    let me = &context.bot.handle;
    let lead = state.team.lead_handle().to_owned();
    if state.pending_voters().iter().any(|voter| voter == me) {
        let open = state.board.open_count();
        return if open == 0 {
            vec![BotAction::Vote {
                approve: true,
                reason: format!("All {} tasks verified against the goal.", state.board.len()),
            }]
        } else {
            vec![BotAction::Vote {
                approve: false,
                reason: format!("{open} task(s) are still open."),
            }]
        };
    }
    if let Some(task) = state.board.actionable_for(me).first() {
        return if task.status == TaskStatus::Todo {
            vec![
                BotAction::UpdateTask {
                    task: task.id,
                    status: TaskStatus::InProgress,
                    note: Some("started".to_owned()),
                },
                BotAction::post(format!("Picking up {}: {}", task.id, task.title)),
            ]
        } else {
            let result = format!("{} delivered by @{me}", focus(&context.bot));
            vec![
                BotAction::UpdateTask {
                    task: task.id,
                    status: TaskStatus::Done,
                    note: Some(result.clone()),
                },
                BotAction::post(format!("@{lead} {} is done: {result}.", task.id)),
            ]
        };
    }
    if let Some(message) = state.unseen_mentions(me).last() {
        return vec![BotAction::post(format!(
            "@{} acknowledged — nothing blocking on my side.",
            message.author
        ))];
    }
    vec![BotAction::Pass]
}

/// Plays back pre-recorded turns per bot; passes when a bot's script runs out.
#[derive(Debug, Default)]
pub struct ScriptedBrain {
    scripts: Mutex<BTreeMap<String, VecDeque<BotTurn>>>,
}

impl ScriptedBrain {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_turn(self, bot: &str, actions: Vec<BotAction>) -> Self {
        if let Ok(mut scripts) = self.scripts.lock() {
            scripts
                .entry(bot.to_owned())
                .or_default()
                .push_back(BotTurn::new(actions));
        }
        self
    }
}

#[async_trait]
impl BotBrain for ScriptedBrain {
    async fn take_turn(
        &self,
        context: TurnContext,
        cancellation: CancellationToken,
    ) -> Result<BotTurn, TeamError> {
        if cancellation.is_cancelled() {
            return Err(TeamError::Cancelled);
        }
        let mut scripts = self
            .scripts
            .lock()
            .map_err(|_| TeamError::Brain("script lock poisoned".to_owned()))?;
        Ok(scripts
            .get_mut(&context.bot.handle)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(BotTurn::pass))
    }
}

/// Delays every turn so people can follow a live mission (and interject).
pub struct PacedBrain {
    inner: Arc<dyn BotBrain>,
    delay: Duration,
}

impl PacedBrain {
    #[must_use]
    pub fn new(inner: Arc<dyn BotBrain>, delay: Duration) -> Self {
        Self { inner, delay }
    }
}

#[async_trait]
impl BotBrain for PacedBrain {
    async fn take_turn(
        &self,
        context: TurnContext,
        cancellation: CancellationToken,
    ) -> Result<BotTurn, TeamError> {
        tokio::select! {
            () = cancellation.cancelled() => return Err(TeamError::Cancelled),
            () = tokio::time::sleep(self.delay) => {}
        }
        self.inner.take_turn(context, cancellation).await
    }
}

/// Routes each bot to its own brain, falling back to a default.
pub struct BrainRouter {
    default: Arc<dyn BotBrain>,
    overrides: BTreeMap<String, Arc<dyn BotBrain>>,
}

impl BrainRouter {
    #[must_use]
    pub fn new(default: Arc<dyn BotBrain>) -> Self {
        Self {
            default,
            overrides: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_bot(mut self, handle: impl Into<String>, brain: Arc<dyn BotBrain>) -> Self {
        self.overrides.insert(handle.into(), brain);
        self
    }

    #[must_use]
    pub fn brain_for(&self, handle: &str) -> Arc<dyn BotBrain> {
        self.overrides
            .get(handle)
            .cloned()
            .unwrap_or_else(|| Arc::clone(&self.default))
    }
}
