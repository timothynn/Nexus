//! The mission engine: rounds, turns, action application, approvals, and budgets.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::future::join_all;
use nexus_permissions::{
    PermissionApprover, PermissionDecision, PermissionPolicy, PermissionRequest, RuleBasedPolicy,
    enforce_with_approver,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    ActivityReporter, BoardTask, BotAction, BotTurn, BrainRouter, Channel, Goal, Message,
    MessageId, MissionId, MissionReport, MissionState, MissionStatus, TaskStatus, TeamError,
    TeamEvent, TeamSpec, TurnContext,
    conversation::{MessageKind, direct_channel_name, parse_mentions, valid_channel_name},
    floor::{FloorPolicy, Speaker, policy_for},
    roster::{HUMAN, SYSTEM},
};

/// Receives every mission event as it happens.
pub trait TeamEventSink: Send + Sync {
    fn record(&self, mission: &MissionId, event: &TeamEvent);
}

/// Collects events in memory (tests, embedding, and replay verification).
#[derive(Debug, Default)]
pub struct MemoryEventSink {
    events: Mutex<Vec<TeamEvent>>,
}

impl MemoryEventSink {
    #[must_use]
    pub fn events(&self) -> Vec<TeamEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl TeamEventSink for MemoryEventSink {
    fn record(&self, _mission: &MissionId, event: &TeamEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event.clone());
        }
    }
}

/// Forwards events into an async channel (interfaces such as the TUI).
#[derive(Debug, Clone)]
pub struct ChannelEventSink {
    sender: mpsc::UnboundedSender<TeamEvent>,
}

impl ChannelEventSink {
    #[must_use]
    pub fn new(sender: mpsc::UnboundedSender<TeamEvent>) -> Self {
        Self { sender }
    }
}

impl TeamEventSink for ChannelEventSink {
    fn record(&self, _mission: &MissionId, event: &TeamEvent) {
        let _ = self.sender.send(event.clone());
    }
}

/// Operator input delivered to a running mission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlInput {
    /// Post as `@human`. Mentions bring bots into the conversation.
    Post {
        channel: String,
        text: String,
        thread: Option<MessageId>,
    },
    /// Private message from `@human` to one bot (opens a direct channel).
    Direct {
        to: String,
        text: String,
    },
    /// Answer the pending question from a bot.
    Answer(String),
    /// Approve the pending completion proposal.
    Approve,
    /// Reject the pending completion proposal with a reason.
    Reject(String),
    Pause,
    Resume,
}

/// Cloneable handle for steering a running mission.
#[derive(Debug, Clone)]
pub struct MissionControl {
    sender: mpsc::UnboundedSender<ControlInput>,
    cancellation: CancellationToken,
}

impl MissionControl {
    pub fn send(&self, input: ControlInput) -> Result<(), TeamError> {
        self.sender
            .send(input)
            .map_err(|_| TeamError::MissionClosed)
    }

    pub fn post(
        &self,
        channel: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<(), TeamError> {
        self.send(ControlInput::Post {
            channel: channel.into(),
            text: text.into(),
            thread: None,
        })
    }

    pub fn direct(&self, to: impl Into<String>, text: impl Into<String>) -> Result<(), TeamError> {
        self.send(ControlInput::Direct {
            to: to.into(),
            text: text.into(),
        })
    }

    pub fn answer(&self, text: impl Into<String>) -> Result<(), TeamError> {
        self.send(ControlInput::Answer(text.into()))
    }

    pub fn approve(&self) -> Result<(), TeamError> {
        self.send(ControlInput::Approve)
    }

    pub fn reject(&self, reason: impl Into<String>) -> Result<(), TeamError> {
        self.send(ControlInput::Reject(reason.into()))
    }

    pub fn pause(&self) -> Result<(), TeamError> {
        self.send(ControlInput::Pause)
    }

    pub fn resume(&self) -> Result<(), TeamError> {
        self.send(ControlInput::Resume)
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    #[must_use]
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

/// Fresh budget granted to a resumed mission, on top of what it already spent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeOptions {
    /// Additional rounds (defaults to the team's round budget).
    pub rounds: Option<u32>,
    /// Additional turns (defaults to the team's turn budget).
    pub turns: Option<u32>,
    /// Additional tokens (defaults to the team's token budget, if any).
    pub tokens: Option<u64>,
    /// Feedback posted to `#general` as `@human` when the mission resumes.
    pub note: Option<String>,
}

/// Runs one mission for a team toward a goal.
pub struct MissionEngine {
    state: MissionState,
    brains: BrainRouter,
    policy: Box<dyn FloorPolicy>,
    sinks: Vec<Arc<dyn TeamEventSink>>,
    control: MissionControl,
    inputs: mpsc::UnboundedReceiver<ControlInput>,
    interactive: bool,
    global_policy: Option<Arc<dyn PermissionPolicy>>,
    approver: Option<Arc<dyn PermissionApprover>>,
    bot_policies: BTreeMap<String, RuleBasedPolicy>,
    activity_sender: mpsc::UnboundedSender<(String, String, String)>,
    activity: mpsc::UnboundedReceiver<(String, String, String)>,
    stalled_rounds: u32,
    started: bool,
    resume: Option<ResumeOptions>,
}

/// Outcome of waiting for the operator.
enum HumanReply {
    Input(ControlInput),
    TimedOut,
    Cancelled,
}

impl MissionEngine {
    pub fn new(team: TeamSpec, goal: Goal, brains: BrainRouter) -> Result<Self, TeamError> {
        team.validate()?;
        if goal.statement.trim().is_empty() {
            return Err(TeamError::InvalidTeam(
                "a mission goal is required".to_owned(),
            ));
        }
        let (sender, inputs) = mpsc::unbounded_channel();
        let (activity_sender, activity) = mpsc::unbounded_channel();
        let bot_policies = team
            .members
            .iter()
            .map(|member| {
                (
                    member.handle.clone(),
                    default_bot_policy(&team, &member.handle),
                )
            })
            .collect();
        Ok(Self {
            policy: policy_for(team.policy),
            state: MissionState::new(MissionId::generate(), team, goal),
            brains,
            sinks: Vec::new(),
            control: MissionControl {
                sender,
                cancellation: CancellationToken::new(),
            },
            inputs,
            interactive: false,
            global_policy: None,
            approver: None,
            bot_policies,
            activity_sender,
            activity,
            stalled_rounds: 0,
            started: false,
            resume: None,
        })
    }

    /// Continues a persisted mission (stalled, out of budget, failed, cancelled, or interrupted).
    ///
    /// A completed mission reopens only with a note describing follow-up work, so
    /// resuming never just re-approves finished work. New events extend the same
    /// mission id, so the full history still replays.
    pub fn resume(
        state: MissionState,
        brains: BrainRouter,
        options: ResumeOptions,
    ) -> Result<Self, TeamError> {
        let has_note = options
            .note
            .as_deref()
            .is_some_and(|note| !note.trim().is_empty());
        if state.status == MissionStatus::Completed && !has_note {
            return Err(TeamError::AlreadyCompleted);
        }
        let mut engine = Self::new(state.team.clone(), state.goal.clone(), brains)?;
        engine.state = state;
        engine.resume = Some(options);
        Ok(engine)
    }

    fn resumed_budget(&self, options: &ResumeOptions) -> crate::Budget {
        let current = &self.state.team.budget;
        let mut budget = current.clone();
        budget.max_rounds = self.state.round + options.rounds.unwrap_or(current.max_rounds).max(1);
        budget.max_turns = self.state.turns + options.turns.unwrap_or(current.max_turns).max(1);
        budget.max_messages = self.state.message_count() + current.max_messages;
        budget.max_tokens = options
            .tokens
            .or(current.max_tokens)
            .map(|tokens| self.state.usage.total() + tokens);
        budget
    }

    #[must_use]
    pub fn with_mission_id(mut self, id: MissionId) -> Self {
        self.state.id = id;
        self
    }

    #[must_use]
    pub fn with_sink(mut self, sink: Arc<dyn TeamEventSink>) -> Self {
        self.sinks.push(sink);
        self
    }

    /// Declares that a human operator is present to answer questions and approve completion.
    #[must_use]
    pub fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// Replaces the floor policy implementation (custom strategies).
    #[must_use]
    pub fn with_floor_policy(mut self, policy: Box<dyn FloorPolicy>) -> Self {
        self.policy = policy;
        self
    }

    /// Adds an operator-wide permission policy; the stricter of it and each bot's policy wins.
    #[must_use]
    pub fn with_permission_policy(mut self, policy: Arc<dyn PermissionPolicy>) -> Self {
        self.global_policy = Some(policy);
        self
    }

    /// Resolves `ask` decisions for team actions.
    #[must_use]
    pub fn with_approver(mut self, approver: Arc<dyn PermissionApprover>) -> Self {
        self.approver = Some(approver);
        self
    }

    #[must_use]
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.control.cancellation = cancellation;
        self
    }

    #[must_use]
    pub fn control(&self) -> MissionControl {
        self.control.clone()
    }

    #[must_use]
    pub fn state(&self) -> &MissionState {
        &self.state
    }

    #[must_use]
    pub fn mission_id(&self) -> &MissionId {
        &self.state.id
    }

    /// Runs the mission to a terminal status and returns its report.
    pub async fn run(mut self) -> Result<MissionReport, TeamError> {
        if self.started {
            return Err(TeamError::InvalidTeam("mission already started".to_owned()));
        }
        self.started = true;
        if self.state.team.requires_human_approval() && !self.interactive {
            return Err(TeamError::InvalidTeam(
                "this team requires @human approval; run it interactively".to_owned(),
            ));
        }
        if let Some(options) = self.resume.take() {
            self.start_resumed(options);
        } else {
            self.start_fresh();
        }

        while !self.state.status.is_terminal() {
            self.round().await;
        }
        let report = self.state.build_report();
        self.emit(TeamEvent::MissionFinished {
            report: report.clone(),
        });
        Ok(report)
    }

    fn start_fresh(&mut self) {
        self.emit(TeamEvent::MissionStarted {
            mission: self.state.id.clone(),
            team: self.state.team.clone(),
            goal: self.state.goal.clone(),
        });
        for channel in self.state.channels.clone() {
            self.emit(TeamEvent::ChannelCreated {
                channel,
                by: SYSTEM.to_owned(),
            });
        }
        let kickoff = format!(
            "Mission started for team \"{}\".\nGoal: {}\nFloor policy: {}. Approval: {}. Lead: @{}. Budget: {} rounds.",
            self.state.team.name,
            self.state.goal.render(),
            self.policy.kind(),
            self.state.team.approval_summary(),
            self.state.team.lead_handle(),
            self.state.team.budget.max_rounds
        );
        self.system_post(crate::conversation::GENERAL, kickoff, Vec::new());
    }

    fn start_resumed(&mut self, options: ResumeOptions) {
        let budget = self.resumed_budget(&options);
        let extra_rounds = budget.max_rounds - self.state.round;
        self.emit(TeamEvent::MissionResumed {
            budget,
            note: options.note.clone(),
        });
        let lead = self.state.team.lead_handle().to_owned();
        self.system_post(
            crate::conversation::GENERAL,
            format!("Mission resumed with {extra_rounds} more round(s). @{lead} pick up where the team left off."),
            vec![lead],
        );
        if let Some(note) = options.note.filter(|note| !note.trim().is_empty()) {
            self.post(
                crate::conversation::GENERAL,
                HUMAN,
                MessageKind::Chat,
                &note,
                None,
                None,
            );
        }
    }

    async fn round(&mut self) {
        if self.check_cancelled() {
            return;
        }
        self.drain_inputs();
        if self.state.status == MissionStatus::Paused {
            self.wait_while_paused().await;
            return;
        }
        let budget = self.state.team.budget.clone();
        if self.state.round >= budget.max_rounds {
            self.finish(
                MissionStatus::OutOfBudget,
                format!("reached {} rounds", budget.max_rounds),
            );
            return;
        }
        let round = self.state.round + 1;
        self.emit(TeamEvent::RoundStarted { round });
        let progress_before = self.state.progress;

        let speakers = if self.state.status == MissionStatus::Reviewing {
            self.state
                .pending_voters()
                .into_iter()
                .map(|voter| Speaker::new(voter, "vote on the completion proposal"))
                .collect()
        } else {
            self.policy.select(&self.state)
        };
        let parallel = self.state.team.parallel_turns
            || self.policy.kind() == crate::FloorPolicyKind::Broadcast;
        if parallel {
            self.parallel_turns(speakers).await;
        } else {
            let reviewing = self.state.status == MissionStatus::Reviewing;
            for speaker in speakers {
                if self.state.status.is_terminal() || self.check_cancelled() {
                    break;
                }
                if reviewing && self.state.proposal.is_none() {
                    break;
                }
                self.sequential_turn(speaker).await;
            }
        }
        if self.state.status == MissionStatus::Reviewing {
            self.resolve_proposal().await;
        }
        if !self.state.status.is_terminal() {
            self.check_budgets();
        }
        if !self.state.status.is_terminal() {
            self.track_stall(progress_before, budget.stall_rounds);
        }
    }

    fn track_stall(&mut self, progress_before: u64, stall_rounds: u32) {
        if self.state.progress > progress_before {
            self.stalled_rounds = 0;
            return;
        }
        self.stalled_rounds += 1;
        if stall_rounds == 0 {
            return;
        }
        if self.stalled_rounds >= stall_rounds {
            self.finish(
                MissionStatus::Stalled,
                format!("no progress for {stall_rounds} consecutive rounds"),
            );
        } else if self.stalled_rounds + 1 == stall_rounds {
            let lead = self.state.team.lead_handle().to_owned();
            self.system_post(
                crate::conversation::GENERAL,
                format!(
                    "@{lead} the team made no progress last round. Re-plan, unblock someone, or propose completion."
                ),
                vec![lead],
            );
        }
    }

    fn check_budgets(&mut self) {
        let budget = &self.state.team.budget;
        if let Some(max_tokens) = budget.max_tokens {
            if self.state.usage.total() >= max_tokens {
                let reason = format!("token budget of {max_tokens} exhausted");
                self.finish(MissionStatus::OutOfBudget, reason);
                return;
            }
        }
        if self.state.turns >= budget.max_turns {
            let reason = format!("reached {} turns", budget.max_turns);
            self.finish(MissionStatus::OutOfBudget, reason);
        } else if self.state.message_count() >= budget.max_messages {
            let reason = format!("reached {} messages", budget.max_messages);
            self.finish(MissionStatus::OutOfBudget, reason);
        }
    }

    fn turn_context(&self, speaker: &Speaker, snapshot: &Arc<MissionState>) -> Option<TurnContext> {
        let bot = self.state.team.member(&speaker.handle)?.clone();
        Some(TurnContext {
            channel: snapshot.reply_channel(&bot.handle),
            activity: ActivityReporter::new(bot.handle.clone(), self.activity_sender.clone()),
            bot,
            reason: speaker.reason.clone(),
            state: Arc::clone(snapshot),
        })
    }

    async fn sequential_turn(&mut self, speaker: Speaker) {
        if self.state.turns >= self.state.team.budget.max_turns {
            return;
        }
        let snapshot = Arc::new(self.state.clone());
        let Some(context) = self.turn_context(&speaker, &snapshot) else {
            return;
        };
        self.start_turn(&speaker);
        let brain = self.brains.brain_for(&speaker.handle);
        let channel = context.channel.clone();
        let result = run_brain(
            brain,
            context,
            self.control.cancellation.child_token(),
            self.state.team.budget.turn_timeout(),
        )
        .await;
        self.complete_turn(&speaker.handle, &channel, result).await;
    }

    async fn parallel_turns(&mut self, speakers: Vec<Speaker>) {
        let remaining = self
            .state
            .team
            .budget
            .max_turns
            .saturating_sub(self.state.turns) as usize;
        let snapshot = Arc::new(self.state.clone());
        let contexts = speakers
            .iter()
            .take(remaining)
            .filter_map(|speaker| {
                self.turn_context(speaker, &snapshot)
                    .map(|ctx| (speaker.clone(), ctx))
            })
            .collect::<Vec<_>>();
        for (speaker, _) in &contexts {
            self.start_turn(speaker);
        }
        let timeout = self.state.team.budget.turn_timeout();
        let futures = contexts.iter().map(|(speaker, context)| {
            run_brain(
                self.brains.brain_for(&speaker.handle),
                context.clone(),
                self.control.cancellation.child_token(),
                timeout,
            )
        });
        let results = join_all(futures).await;
        for ((speaker, context), result) in contexts.into_iter().zip(results) {
            if self.state.status.is_terminal() {
                break;
            }
            self.complete_turn(&speaker.handle, &context.channel, result)
                .await;
        }
    }

    fn start_turn(&mut self, speaker: &Speaker) {
        self.emit(TeamEvent::TurnStarted {
            round: self.state.round,
            bot: speaker.handle.clone(),
            reason: speaker.reason.clone(),
        });
    }

    async fn complete_turn(
        &mut self,
        bot: &str,
        channel: &str,
        result: Result<BotTurn, TeamError>,
    ) {
        self.drain_activity();
        match result {
            Ok(turn) => {
                let labels = turn
                    .actions
                    .iter()
                    .map(|action| action.label().to_owned())
                    .collect();
                for action in turn.actions {
                    if self.state.status.is_terminal() {
                        break;
                    }
                    self.apply_action(bot, channel, action).await;
                }
                self.emit(TeamEvent::TurnCompleted {
                    round: self.state.round,
                    bot: bot.to_owned(),
                    usage: turn.usage,
                    actions: labels,
                });
            }
            Err(TeamError::Cancelled) => {
                self.check_cancelled();
            }
            Err(error) => {
                self.emit(TeamEvent::TurnFailed {
                    round: self.state.round,
                    bot: bot.to_owned(),
                    error: error.to_string(),
                });
                self.system_post(
                    channel,
                    format!("@{bot}'s turn failed: {error}"),
                    Vec::new(),
                );
            }
        }
    }

    fn drain_activity(&mut self) {
        while let Ok((bot, kind, detail)) = self.activity.try_recv() {
            self.emit(TeamEvent::BotActivity { bot, kind, detail });
        }
    }

    fn authorize(&self, bot: &str, action: &BotAction) -> Result<(), String> {
        let request = PermissionRequest::new(action.permission());
        if let Some(global) = &self.global_policy {
            enforce_with_approver(global.as_ref(), self.approver.as_deref(), &request)
                .map_err(|error| error.to_string())?;
        }
        if let Some(policy) = self.bot_policies.get(bot) {
            enforce_with_approver(policy, self.approver.as_deref(), &request)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn deny(&mut self, bot: &str, action: &BotAction, reason: impl Into<String>) {
        let reason = reason.into();
        self.emit(TeamEvent::ActionDenied {
            bot: bot.to_owned(),
            action: action.label().to_owned(),
            reason: reason.clone(),
        });
        self.emit(TeamEvent::NoteRecorded {
            bot: bot.to_owned(),
            text: format!("(engine) your {} was rejected: {reason}", action.label()),
        });
    }

    async fn apply_action(&mut self, bot: &str, channel: &str, action: BotAction) {
        if let Err(reason) = self.authorize(bot, &action) {
            self.deny(bot, &action, reason);
            return;
        }
        let outcome = match action.clone() {
            BotAction::Pass => Ok(()),
            BotAction::Post {
                text,
                channel: target,
                thread,
            } => self.bot_post(
                bot,
                target.as_deref().unwrap_or(channel),
                &text,
                thread.map(MessageId),
            ),
            BotAction::DirectMessage { to, text } => self.direct_message(bot, &to, &text),
            BotAction::CreateTask {
                title,
                description,
                assignee,
                depends_on,
            } => self.create_task(bot, channel, title, description, assignee, depends_on),
            BotAction::AssignTask { task, assignee } => {
                self.assign_task(bot, channel, task, &assignee)
            }
            BotAction::UpdateTask { task, status, note } => {
                self.update_task(bot, channel, task, status, note)
            }
            BotAction::CreateChannel {
                name,
                purpose,
                members,
            } => self.create_channel(bot, &name, purpose, members),
            BotAction::Note { text } => {
                self.emit(TeamEvent::NoteRecorded {
                    bot: bot.to_owned(),
                    text,
                });
                Ok(())
            }
            BotAction::AskHuman { question } => {
                self.ask_human(bot, channel, &question).await;
                Ok(())
            }
            BotAction::Handoff { to, summary } => self.handoff(bot, channel, &to, &summary),
            BotAction::ProposeCompletion { summary } => self.propose(bot, channel, &summary).await,
            BotAction::Vote { approve, reason } => self.vote(bot, channel, approve, &reason),
        };
        match outcome {
            Err(reason) => self.deny(bot, &action, reason),
            Ok(()) if matches!(action, BotAction::Vote { .. }) => self.resolve_proposal().await,
            Ok(()) => {}
        }
    }

    fn bot_post(
        &mut self,
        bot: &str,
        channel: &str,
        text: &str,
        thread: Option<MessageId>,
    ) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("message text is empty".to_owned());
        }
        let target = self
            .state
            .channel(channel)
            .ok_or_else(|| format!("unknown channel #{channel}"))?;
        if !target.includes(bot) {
            return Err(format!("@{bot} is not a member of #{channel}"));
        }
        let kind = if target.direct {
            MessageKind::Direct
        } else {
            MessageKind::Chat
        };
        if let Some(root) = thread {
            let root_message = self
                .state
                .message(root)
                .ok_or_else(|| format!("unknown thread {root}"))?;
            if root_message.channel != channel {
                return Err(format!("thread {root} is not in #{channel}"));
            }
        }
        let thread = thread.map(|root| self.thread_root(root));
        self.post(channel, bot, kind, text, thread, None);
        Ok(())
    }

    fn thread_root(&self, id: MessageId) -> MessageId {
        self.state
            .message(id)
            .and_then(|message| message.thread)
            .unwrap_or(id)
    }

    fn direct_message(&mut self, bot: &str, to: &str, text: &str) -> Result<(), String> {
        if to == bot {
            return Err("cannot message yourself".to_owned());
        }
        if to != HUMAN && !self.state.team.is_member(to) {
            return Err(format!("unknown member @{to}"));
        }
        let name = direct_channel_name(bot, to);
        if self.state.channel(&name).is_none() {
            self.emit(TeamEvent::ChannelCreated {
                channel: Channel::direct(bot, to),
                by: bot.to_owned(),
            });
        }
        let mut mentions = vec![to.to_owned()];
        for mention in self.known_mentions(text) {
            if !mentions.contains(&mention) {
                mentions.push(mention);
            }
        }
        self.post(&name, bot, MessageKind::Direct, text, None, Some(mentions));
        Ok(())
    }

    fn create_task(
        &mut self,
        bot: &str,
        channel: &str,
        title: String,
        description: String,
        assignee: Option<String>,
        depends_on: Vec<crate::TaskId>,
    ) -> Result<(), String> {
        if title.trim().is_empty() {
            return Err("task title is empty".to_owned());
        }
        if let Some(assignee) = &assignee {
            if !self.state.team.is_member(assignee) {
                return Err(format!("unknown assignee @{assignee}"));
            }
        }
        let id = self.state.board.next_id();
        self.state
            .board
            .validate_dependencies(id, &depends_on)
            .map_err(|error| error.to_string())?;
        let task = BoardTask {
            id,
            title,
            description,
            assignee: assignee.clone(),
            status: TaskStatus::Todo,
            depends_on,
            created_by: bot.to_owned(),
            note: None,
        };
        let text = format!(
            "created {} \"{}\"{}",
            task.id,
            task.title,
            assignee
                .as_deref()
                .map(|handle| format!(" → @{handle}"))
                .unwrap_or_default()
        );
        self.emit(TeamEvent::TaskCreated { task });
        let channel = self.public_channel(channel);
        self.post(
            &channel,
            bot,
            MessageKind::TaskUpdate,
            &text,
            None,
            Some(assignee.into_iter().collect()),
        );
        Ok(())
    }

    fn assign_task(
        &mut self,
        bot: &str,
        channel: &str,
        id: crate::TaskId,
        assignee: &str,
    ) -> Result<(), String> {
        if !self.state.team.is_member(assignee) {
            return Err(format!("unknown assignee @{assignee}"));
        }
        let mut task = self
            .state
            .board
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown task {id}"))?;
        if !task.status.is_open() {
            return Err(format!("{id} is already {}", task.status.label()));
        }
        task.assignee = Some(assignee.to_owned());
        let text = format!("assigned {} \"{}\" → @{assignee}", task.id, task.title);
        self.emit(TeamEvent::TaskUpdated {
            task,
            by: bot.to_owned(),
        });
        let channel = self.public_channel(channel);
        self.post(
            &channel,
            bot,
            MessageKind::TaskUpdate,
            &text,
            None,
            Some(vec![assignee.to_owned()]),
        );
        Ok(())
    }

    fn update_task(
        &mut self,
        bot: &str,
        channel: &str,
        id: crate::TaskId,
        status: TaskStatus,
        note: Option<String>,
    ) -> Result<(), String> {
        let mut task = self
            .state
            .board
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown task {id}"))?;
        let lead = self.state.team.lead_handle().to_owned();
        let allowed =
            task.assignee.as_deref() == Some(bot) || task.created_by == bot || lead == bot;
        if !allowed {
            return Err(format!(
                "only the assignee, creator, or lead may update {id}"
            ));
        }
        if matches!(
            status,
            TaskStatus::InProgress | TaskStatus::Done | TaskStatus::Review
        ) && !self.state.board.is_ready(&task)
            && task.status.is_open()
        {
            return Err(format!("{id} is waiting on unfinished dependencies"));
        }
        task.status = status;
        let mut text = format!("{} \"{}\" → {}", task.id, task.title, status.label());
        if let Some(note) = note {
            text.push_str(": ");
            text.push_str(&note);
            task.note = Some(note);
        }
        let mentions = if status == TaskStatus::Blocked && bot != lead {
            vec![lead]
        } else {
            Vec::new()
        };
        self.emit(TeamEvent::TaskUpdated {
            task,
            by: bot.to_owned(),
        });
        let channel = self.public_channel(channel);
        self.post(
            &channel,
            bot,
            MessageKind::TaskUpdate,
            &text,
            None,
            Some(mentions),
        );
        Ok(())
    }

    fn create_channel(
        &mut self,
        bot: &str,
        name: &str,
        purpose: String,
        mut members: Vec<String>,
    ) -> Result<(), String> {
        if !valid_channel_name(name) {
            return Err(format!("invalid channel name `{name}`"));
        }
        if self.state.channel(name).is_some() {
            return Err(format!("#{name} already exists"));
        }
        for member in &members {
            if member != HUMAN && !self.state.team.is_member(member) {
                return Err(format!("unknown member @{member}"));
            }
        }
        if !members.is_empty() && !members.iter().any(|member| member == bot) {
            members.push(bot.to_owned());
        }
        let channel = Channel {
            name: name.to_owned(),
            purpose,
            members,
            direct: false,
        };
        let audience = if channel.members.is_empty() {
            "everyone".to_owned()
        } else {
            channel
                .members
                .iter()
                .map(|m| format!("@{m}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        self.emit(TeamEvent::ChannelCreated {
            channel,
            by: bot.to_owned(),
        });
        self.system_post(
            name,
            format!("@{bot} created #{name} for {audience}"),
            Vec::new(),
        );
        Ok(())
    }

    fn handoff(&mut self, bot: &str, channel: &str, to: &str, summary: &str) -> Result<(), String> {
        if to == bot || !self.state.team.is_member(to) {
            return Err(format!("cannot hand off to @{to}"));
        }
        let channel = self.public_channel(channel);
        self.post(
            &channel,
            bot,
            MessageKind::Handoff,
            &format!("@{to} over to you: {summary}"),
            None,
            Some(vec![to.to_owned()]),
        );
        Ok(())
    }

    async fn propose(&mut self, bot: &str, channel: &str, summary: &str) -> Result<(), String> {
        if self.state.proposal.is_some() {
            return Err("a completion proposal is already pending".to_owned());
        }
        if summary.trim().is_empty() {
            return Err("proposal summary is empty".to_owned());
        }
        self.emit(TeamEvent::ProposalOpened {
            by: bot.to_owned(),
            summary: summary.to_owned(),
        });
        let approvers = self.state.team.approvers.clone();
        let channel = self.public_channel(channel);
        self.post(
            &channel,
            bot,
            MessageKind::Proposal,
            &format!("Proposing completion: {summary}"),
            None,
            Some(approvers),
        );
        self.set_status(
            MissionStatus::Reviewing,
            format!("@{bot} proposed completion"),
        );
        self.resolve_proposal().await;
        Ok(())
    }

    fn vote(
        &mut self,
        bot: &str,
        channel: &str,
        approve: bool,
        reason: &str,
    ) -> Result<(), String> {
        let Some(proposal) = &self.state.proposal else {
            return Err("there is no pending proposal".to_owned());
        };
        if !self
            .state
            .team
            .bot_approvers()
            .any(|approver| approver == bot)
        {
            return Err(format!("@{bot} is not an approver"));
        }
        if proposal.votes.contains_key(bot) {
            return Err(format!("@{bot} already voted"));
        }
        let proposer = proposal.by.clone();
        self.emit(TeamEvent::VoteCast {
            by: bot.to_owned(),
            approve,
            reason: reason.to_owned(),
        });
        let verdict = if approve { "approve" } else { "reject" };
        let channel = self.public_channel(channel);
        self.post(
            &channel,
            bot,
            MessageKind::Vote,
            &format!("{verdict}: {reason}"),
            None,
            Some(vec![proposer]),
        );
        Ok(())
    }

    /// Settles a proposal as soon as the team's approval rule allows (then asks the human, if required).
    async fn resolve_proposal(&mut self) {
        let Some(proposal) = self.state.proposal.clone() else {
            return;
        };
        let voters = self.state.team.bot_approvers().count();
        let approvals = proposal.votes.values().filter(|vote| vote.approve).count();
        let rejections = proposal.votes.len() - approvals;
        let rule = self.state.team.approval;
        let bots_accept = if voters == 0 {
            Some(true)
        } else {
            rule.decide(approvals, rejections, voters)
        };
        match bots_accept {
            None => return,
            Some(false) => {
                let objections = proposal
                    .votes
                    .iter()
                    .filter(|(_, vote)| !vote.approve)
                    .map(|(voter, vote)| format!("@{voter}: {}", vote.reason))
                    .collect::<Vec<_>>()
                    .join("; ");
                let reason = format!(
                    "rejected under the `{rule}` rule ({approvals}/{voters} approved) — {objections}"
                );
                self.reject_proposal(&proposal.by, &reason);
                return;
            }
            Some(true) => {}
        }
        let mut approved_by = proposal
            .votes
            .iter()
            .filter(|(_, vote)| vote.approve)
            .map(|(voter, _)| format!("@{voter}"))
            .collect::<Vec<_>>();
        if self.state.team.requires_human_approval() {
            match self.await_human_decision(&proposal.summary).await {
                Some(Ok(())) => approved_by.push(format!("@{HUMAN}")),
                Some(Err(reason)) => {
                    self.reject_proposal(&proposal.by, &format!("rejected by @human: {reason}"));
                    return;
                }
                None => return,
            }
        }
        let approvers = if approved_by.is_empty() {
            format!("@{} (lead decision)", proposal.by)
        } else {
            approved_by.join(", ")
        };
        self.emit(TeamEvent::ProposalResolved { accepted: true });
        self.system_post(
            crate::conversation::GENERAL,
            format!("Goal accepted by {approvers}. Mission complete."),
            Vec::new(),
        );
        self.finish(MissionStatus::Completed, format!("approved by {approvers}"));
    }

    fn reject_proposal(&mut self, proposer: &str, reason: &str) {
        self.emit(TeamEvent::ProposalResolved { accepted: false });
        self.set_status(MissionStatus::Active, reason.to_owned());
        self.system_post(
            crate::conversation::GENERAL,
            format!("@{proposer} completion {reason}. Keep going."),
            vec![proposer.to_owned()],
        );
    }

    /// Returns `Some(Ok)` on approval, `Some(Err(reason))` on rejection, `None` if cancelled.
    async fn await_human_decision(&mut self, summary: &str) -> Option<Result<(), String>> {
        self.set_status(
            MissionStatus::AwaitingHuman,
            "waiting for @human to approve completion".to_owned(),
        );
        self.system_post(
            crate::conversation::GENERAL,
            format!("@human please approve or reject completion: {summary}"),
            vec![HUMAN.to_owned()],
        );
        let timeout = self.state.team.budget.human_timeout();
        loop {
            match self.wait_for_human(timeout).await {
                HumanReply::Input(ControlInput::Approve) => {
                    self.set_status(MissionStatus::Reviewing, "approved by @human".to_owned());
                    return Some(Ok(()));
                }
                HumanReply::Input(ControlInput::Reject(reason)) => {
                    return Some(Err(reason));
                }
                HumanReply::Input(other) => self.handle_input(other),
                HumanReply::TimedOut => {
                    return Some(Err("no decision before the approval timeout".to_owned()));
                }
                HumanReply::Cancelled => return None,
            }
        }
    }

    async fn ask_human(&mut self, bot: &str, channel: &str, question: &str) {
        let channel = self.public_channel(channel);
        self.emit(TeamEvent::HumanQuestion {
            bot: bot.to_owned(),
            question: question.to_owned(),
        });
        self.post(
            &channel,
            bot,
            MessageKind::Question,
            &format!("@human {question}"),
            None,
            Some(vec![HUMAN.to_owned()]),
        );
        if !self.interactive {
            self.emit(TeamEvent::HumanAnswered { answer: None });
            self.system_post(
                &channel,
                format!("@{bot} no human operator is attached; proceed with your best judgment."),
                vec![bot.to_owned()],
            );
            return;
        }
        let previous = self.state.status;
        self.set_status(
            MissionStatus::AwaitingHuman,
            format!("@{bot} asked the human a question"),
        );
        let timeout = self.state.team.budget.human_timeout();
        let answer = loop {
            match self.wait_for_human(timeout).await {
                HumanReply::Input(ControlInput::Answer(text) | ControlInput::Post { text, .. }) => {
                    break Some(text);
                }
                HumanReply::Input(other) => self.handle_input(other),
                HumanReply::TimedOut | HumanReply::Cancelled => break None,
            }
        };
        self.emit(TeamEvent::HumanAnswered {
            answer: answer.clone(),
        });
        match answer {
            Some(text) => {
                self.post(
                    &channel,
                    HUMAN,
                    MessageKind::Chat,
                    &format!("@{bot} {text}"),
                    None,
                    None,
                );
            }
            None => self.system_post(
                &channel,
                format!("@{bot} no answer from @human; proceed with your best judgment."),
                vec![bot.to_owned()],
            ),
        }
        if !self.check_cancelled() && self.state.status == MissionStatus::AwaitingHuman {
            self.set_status(previous, "human question resolved".to_owned());
        }
    }

    async fn wait_for_human(&mut self, timeout: Option<Duration>) -> HumanReply {
        let cancellation = self.control.cancellation.clone();
        let sleep = async {
            match timeout {
                Some(duration) => tokio::time::sleep(duration).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            () = cancellation.cancelled() => {
                self.check_cancelled();
                HumanReply::Cancelled
            }
            input = self.inputs.recv() => input.map_or(HumanReply::Cancelled, HumanReply::Input),
            () = sleep => HumanReply::TimedOut,
        }
    }

    async fn wait_while_paused(&mut self) {
        while self.state.status == MissionStatus::Paused {
            match self.wait_for_human(None).await {
                HumanReply::Input(input) => self.handle_input(input),
                HumanReply::Cancelled | HumanReply::TimedOut => return,
            }
        }
    }

    fn drain_inputs(&mut self) {
        while let Ok(input) = self.inputs.try_recv() {
            self.handle_input(input);
        }
    }

    fn handle_input(&mut self, input: ControlInput) {
        match input {
            ControlInput::Post {
                channel,
                text,
                thread,
            } => {
                let channel = if self.state.channel(&channel).is_some() {
                    channel
                } else {
                    crate::conversation::GENERAL.to_owned()
                };
                let thread = thread
                    .filter(|root| self.state.message(*root).is_some())
                    .map(|root| self.thread_root(root));
                self.post(&channel, HUMAN, MessageKind::Chat, &text, thread, None);
            }
            ControlInput::Direct { to, text } => self.human_direct(&to, &text),
            ControlInput::Answer(text) => {
                self.post(
                    crate::conversation::GENERAL,
                    HUMAN,
                    MessageKind::Chat,
                    &text,
                    None,
                    None,
                );
            }
            ControlInput::Pause => {
                if !self.state.status.is_terminal() {
                    self.set_status(MissionStatus::Paused, "paused by @human".to_owned());
                }
            }
            ControlInput::Resume => {
                if self.state.status == MissionStatus::Paused {
                    let status = if self.state.proposal.is_some() {
                        MissionStatus::Reviewing
                    } else {
                        MissionStatus::Active
                    };
                    self.set_status(status, "resumed by @human".to_owned());
                }
            }
            ControlInput::Approve | ControlInput::Reject(_) => self.system_post(
                crate::conversation::GENERAL,
                "There is no completion proposal waiting for @human.",
                Vec::new(),
            ),
        }
    }

    fn human_direct(&mut self, to: &str, text: &str) {
        let to = to.trim_start_matches('@');
        if !self.state.team.is_member(to) {
            self.system_post(
                crate::conversation::GENERAL,
                format!("Cannot message @{to}: not on this team."),
                Vec::new(),
            );
            return;
        }
        let name = direct_channel_name(HUMAN, to);
        if self.state.channel(&name).is_none() {
            self.emit(TeamEvent::ChannelCreated {
                channel: Channel::direct(HUMAN, to),
                by: HUMAN.to_owned(),
            });
        }
        let mut mentions = vec![to.to_owned()];
        for mention in self.known_mentions(text) {
            if !mentions.contains(&mention) {
                mentions.push(mention);
            }
        }
        self.post(
            &name,
            HUMAN,
            MessageKind::Direct,
            text,
            None,
            Some(mentions),
        );
    }

    fn check_cancelled(&mut self) -> bool {
        if self.control.cancellation.is_cancelled() {
            if !self.state.status.is_terminal() {
                self.finish(MissionStatus::Cancelled, "cancelled by operator".to_owned());
            }
            return true;
        }
        false
    }

    fn set_status(&mut self, status: MissionStatus, reason: String) {
        if self.state.status != status {
            self.emit(TeamEvent::StatusChanged { status, reason });
        }
    }

    fn finish(&mut self, status: MissionStatus, reason: String) {
        if !self.state.status.is_terminal() {
            self.system_post(
                crate::conversation::GENERAL,
                format!("Mission {status}: {reason}."),
                Vec::new(),
            );
            self.emit(TeamEvent::StatusChanged { status, reason });
        }
    }

    /// Task and proposal narration never goes to a direct channel the audience can't see.
    fn public_channel(&self, channel: &str) -> String {
        match self.state.channel(channel) {
            Some(target) if !target.direct => channel.to_owned(),
            _ => crate::conversation::GENERAL.to_owned(),
        }
    }

    fn known_mentions(&self, text: &str) -> Vec<String> {
        parse_mentions(text)
            .into_iter()
            .filter(|handle| {
                self.state.team.is_member(handle)
                    || matches!(handle.as_str(), HUMAN | "all" | "team")
            })
            .collect()
    }

    fn system_post(&mut self, channel: &str, text: impl AsRef<str>, mentions: Vec<String>) {
        self.post(
            channel,
            SYSTEM,
            MessageKind::System,
            text.as_ref(),
            None,
            Some(mentions),
        );
    }

    fn post(
        &mut self,
        channel: &str,
        author: &str,
        kind: MessageKind,
        text: &str,
        thread: Option<MessageId>,
        mentions: Option<Vec<String>>,
    ) {
        let mentions = mentions.unwrap_or_else(|| self.known_mentions(text));
        let message = Message {
            id: self.state.next_message_id(),
            channel: channel.to_owned(),
            author: author.to_owned(),
            kind,
            text: text.trim().to_owned(),
            thread,
            mentions,
            round: self.state.round,
        };
        self.emit(TeamEvent::MessagePosted { message });
    }

    #[allow(clippy::needless_pass_by_value)] // Events are built inline at every call site.
    fn emit(&mut self, event: TeamEvent) {
        self.state.apply(&event);
        for sink in &self.sinks {
            sink.record(&self.state.id, &event);
        }
    }
}

async fn run_brain(
    brain: Arc<dyn crate::BotBrain>,
    context: TurnContext,
    cancellation: CancellationToken,
    timeout: Option<Duration>,
) -> Result<BotTurn, TeamError> {
    let turn = brain.take_turn(context, cancellation.clone());
    let guarded = async {
        match timeout {
            Some(limit) => tokio::time::timeout(limit, turn)
                .await
                .map_err(|_| TeamError::TurnTimeout(limit))?,
            None => turn.await,
        }
    };
    tokio::select! {
        () = cancellation.cancelled() => Err(TeamError::Cancelled),
        result = guarded => result,
    }
}

/// Default team-action permissions: everything is allowed except proposing
/// completion, which only the lead may do. Profile overrides apply on top.
#[must_use]
pub fn default_bot_policy(team: &TeamSpec, handle: &str) -> RuleBasedPolicy {
    let propose = if team.lead_handle() == handle {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Deny
    };
    let mut policy =
        RuleBasedPolicy::new(PermissionDecision::Allow).with_rule("team.goal.propose", propose);
    if let Some(member) = team.member(handle) {
        for (action, decision) in &member.permissions {
            policy = policy.with_rule(action.clone(), *decision);
        }
    }
    policy
}
