//! Event-sourced mission state.
//!
//! Every change to a mission is a [`TeamEvent`]. The engine emits events and
//! [`MissionState::apply`] folds them into state, so replaying a persisted
//! event log reproduces the exact same transcript, board, and outcome.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    BoardTask, Budget, Channel, Message, MessageId, TaskBoard, TeamSpec, TurnUsage,
    conversation::MessageKind, roster::SYSTEM,
};

/// Unique mission identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MissionId(pub String);

impl MissionId {
    #[must_use]
    pub fn generate() -> Self {
        Self(format!("mission-{}", Uuid::new_v4().simple()))
    }
}

impl fmt::Display for MissionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What the user wants the team to achieve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub statement: String,
    #[serde(default)]
    pub success_criteria: Vec<String>,
}

impl Goal {
    #[must_use]
    pub fn new(statement: impl Into<String>) -> Self {
        Self {
            statement: statement.into(),
            success_criteria: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_criteria<I, S>(mut self, criteria: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.success_criteria = criteria.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn render(&self) -> String {
        if self.success_criteria.is_empty() {
            return self.statement.clone();
        }
        format!(
            "{}\nSuccess criteria:\n{}",
            self.statement,
            self.success_criteria
                .iter()
                .map(|criterion| format!("- {criterion}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionStatus {
    Active,
    Paused,
    AwaitingHuman,
    Reviewing,
    Completed,
    Stalled,
    OutOfBudget,
    Failed,
    Cancelled,
}

impl MissionStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Stalled | Self::OutOfBudget | Self::Failed | Self::Cancelled
        )
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::AwaitingHuman => "awaiting human",
            Self::Reviewing => "reviewing",
            Self::Completed => "completed",
            Self::Stalled => "stalled",
            Self::OutOfBudget => "out of budget",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for MissionStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vote {
    pub approve: bool,
    pub reason: String,
}

/// A pending claim that the goal is met.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub by: String,
    pub summary: String,
    pub round: u32,
    pub votes: BTreeMap<String, Vote>,
}

/// Per-member activity counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberStats {
    pub turns: u32,
    pub messages: u32,
    pub actions: u32,
    pub denied: u32,
    pub failures: u32,
    pub usage: TurnUsage,
}

/// The mission's immutable, append-only history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TeamEvent {
    MissionStarted {
        mission: MissionId,
        team: TeamSpec,
        goal: Goal,
    },
    RoundStarted {
        round: u32,
    },
    ChannelCreated {
        channel: Channel,
        by: String,
    },
    MessagePosted {
        message: Message,
    },
    TurnStarted {
        round: u32,
        bot: String,
        reason: String,
    },
    TurnCompleted {
        round: u32,
        bot: String,
        usage: TurnUsage,
        actions: Vec<String>,
    },
    TurnFailed {
        round: u32,
        bot: String,
        error: String,
    },
    ActionDenied {
        bot: String,
        action: String,
        reason: String,
    },
    TaskCreated {
        task: BoardTask,
    },
    TaskUpdated {
        task: BoardTask,
        by: String,
    },
    NoteRecorded {
        bot: String,
        text: String,
    },
    HumanQuestion {
        bot: String,
        question: String,
    },
    HumanAnswered {
        answer: Option<String>,
    },
    ProposalOpened {
        by: String,
        summary: String,
    },
    VoteCast {
        by: String,
        approve: bool,
        reason: String,
    },
    ProposalResolved {
        accepted: bool,
    },
    StatusChanged {
        status: MissionStatus,
        reason: String,
    },
    /// Workspace activity from inside a turn (tool calls, model requests). Informational.
    BotActivity {
        bot: String,
        kind: String,
        detail: String,
    },
    MissionFinished {
        report: MissionReport,
    },
    /// A finished or interrupted mission continues with a fresh budget.
    MissionResumed {
        budget: Budget,
        note: Option<String>,
    },
}

impl TeamEvent {
    /// Stable event kind used for persistence and filtering.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::MissionStarted { .. } => "mission_started",
            Self::RoundStarted { .. } => "round_started",
            Self::ChannelCreated { .. } => "channel_created",
            Self::MessagePosted { .. } => "message_posted",
            Self::TurnStarted { .. } => "turn_started",
            Self::TurnCompleted { .. } => "turn_completed",
            Self::TurnFailed { .. } => "turn_failed",
            Self::ActionDenied { .. } => "action_denied",
            Self::TaskCreated { .. } => "task_created",
            Self::TaskUpdated { .. } => "task_updated",
            Self::NoteRecorded { .. } => "note_recorded",
            Self::HumanQuestion { .. } => "human_question",
            Self::HumanAnswered { .. } => "human_answered",
            Self::ProposalOpened { .. } => "proposal_opened",
            Self::VoteCast { .. } => "vote_cast",
            Self::ProposalResolved { .. } => "proposal_resolved",
            Self::StatusChanged { .. } => "status_changed",
            Self::BotActivity { .. } => "bot_activity",
            Self::MissionFinished { .. } => "mission_finished",
            Self::MissionResumed { .. } => "mission_resumed",
        }
    }

    /// Whether the event moves the mission forward (used for stall detection).
    #[must_use]
    pub fn is_progress(&self) -> bool {
        match self {
            Self::MessagePosted { message } => message.author != SYSTEM,
            Self::TaskCreated { .. }
            | Self::TaskUpdated { .. }
            | Self::ProposalOpened { .. }
            | Self::VoteCast { .. }
            | Self::ChannelCreated { .. }
            | Self::HumanAnswered { answer: Some(_) } => true,
            _ => false,
        }
    }
}

/// Summary of a finished mission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionReport {
    pub mission: MissionId,
    pub team: String,
    pub goal: String,
    pub status: MissionStatus,
    pub reason: String,
    pub rounds: u32,
    pub turns: u32,
    pub messages: u32,
    pub tasks_total: u32,
    pub tasks_done: u32,
    pub usage: TurnUsage,
    pub members: BTreeMap<String, MemberStats>,
    pub summary: Option<String>,
}

/// Folded mission state.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionState {
    pub id: MissionId,
    pub team: TeamSpec,
    pub goal: Goal,
    pub status: MissionStatus,
    pub status_reason: String,
    pub round: u32,
    pub turns: u32,
    pub messages: Vec<Message>,
    pub channels: Vec<Channel>,
    pub board: TaskBoard,
    pub notes: BTreeMap<String, Vec<String>>,
    pub proposal: Option<Proposal>,
    pub stats: BTreeMap<String, MemberStats>,
    pub usage: TurnUsage,
    /// Last message id each bot had seen when its latest turn started.
    pub last_seen: BTreeMap<String, MessageId>,
    pub pending_question: Option<(String, String)>,
    pub accepted_summary: Option<String>,
    /// Count of progress events, for stall detection.
    pub progress: u64,
    pub activity: Vec<(String, String, String)>,
    pub report: Option<MissionReport>,
}

impl MissionState {
    /// Rebuilds state from a full event log. The first event must be `MissionStarted`.
    pub fn replay<'a, I>(events: I) -> Option<Self>
    where
        I: IntoIterator<Item = &'a TeamEvent>,
    {
        let mut events = events.into_iter();
        let mut state = match events.next()? {
            TeamEvent::MissionStarted {
                mission,
                team,
                goal,
            } => Self::new(mission.clone(), team.clone(), goal.clone()),
            _ => return None,
        };
        for event in events {
            state.apply(event);
        }
        Some(state)
    }

    #[must_use]
    pub fn new(id: MissionId, team: TeamSpec, goal: Goal) -> Self {
        let channels = team.all_channels();
        let stats = team
            .members
            .iter()
            .map(|member| (member.handle.clone(), MemberStats::default()))
            .collect();
        Self {
            id,
            team,
            goal,
            status: MissionStatus::Active,
            status_reason: String::new(),
            round: 0,
            turns: 0,
            messages: Vec::new(),
            channels,
            board: TaskBoard::default(),
            notes: BTreeMap::new(),
            proposal: None,
            stats,
            usage: TurnUsage::default(),
            last_seen: BTreeMap::new(),
            pending_question: None,
            accepted_summary: None,
            progress: 0,
            activity: Vec::new(),
            report: None,
        }
    }

    /// Folds one event into state.
    pub fn apply(&mut self, event: &TeamEvent) {
        if event.is_progress() {
            self.progress += 1;
        }
        match event {
            TeamEvent::MissionStarted { .. } => {}
            TeamEvent::RoundStarted { round } => self.round = *round,
            TeamEvent::ChannelCreated { channel, .. } => {
                if self.channel(&channel.name).is_none() {
                    self.channels.push(channel.clone());
                }
            }
            TeamEvent::MessagePosted { message } => {
                if let Some(stats) = self.stats.get_mut(&message.author) {
                    stats.messages += 1;
                }
                self.messages.push(message.clone());
            }
            TeamEvent::TurnStarted { bot, .. } => {
                self.turns += 1;
                let latest = self.latest_message_id();
                self.last_seen.insert(bot.clone(), latest);
                self.stats.entry(bot.clone()).or_default().turns += 1;
            }
            TeamEvent::TurnCompleted {
                bot,
                usage,
                actions,
                ..
            } => {
                let stats = self.stats.entry(bot.clone()).or_default();
                stats.usage.add(*usage);
                stats.actions += u32::try_from(actions.len()).unwrap_or(u32::MAX);
                self.usage.add(*usage);
            }
            TeamEvent::TurnFailed { bot, .. } => {
                self.stats.entry(bot.clone()).or_default().failures += 1;
            }
            TeamEvent::ActionDenied { bot, .. } => {
                self.stats.entry(bot.clone()).or_default().denied += 1;
            }
            TeamEvent::TaskCreated { task } | TeamEvent::TaskUpdated { task, .. } => {
                self.board.upsert(task.clone());
            }
            TeamEvent::NoteRecorded { bot, text } => {
                self.notes
                    .entry(bot.clone())
                    .or_default()
                    .push(text.clone());
            }
            TeamEvent::HumanQuestion { bot, question } => {
                self.pending_question = Some((bot.clone(), question.clone()));
            }
            TeamEvent::HumanAnswered { .. } => self.pending_question = None,
            TeamEvent::ProposalOpened { .. }
            | TeamEvent::VoteCast { .. }
            | TeamEvent::ProposalResolved { .. } => self.apply_proposal_event(event),
            TeamEvent::StatusChanged { status, reason } => {
                self.status = *status;
                self.status_reason.clone_from(reason);
            }
            TeamEvent::BotActivity { bot, kind, detail } => {
                self.activity
                    .push((bot.clone(), kind.clone(), detail.clone()));
            }
            TeamEvent::MissionFinished { report } => self.report = Some(report.clone()),
            TeamEvent::MissionResumed { budget, .. } => {
                self.team.budget = budget.clone();
                self.status = if self.proposal.is_some() {
                    MissionStatus::Reviewing
                } else {
                    MissionStatus::Active
                };
                "resumed".clone_into(&mut self.status_reason);
                self.pending_question = None;
                self.report = None;
            }
        }
    }

    /// Completion governance: proposals, votes, and their resolution.
    fn apply_proposal_event(&mut self, event: &TeamEvent) {
        match event {
            TeamEvent::ProposalOpened { by, summary } => {
                self.proposal = Some(Proposal {
                    by: by.clone(),
                    summary: summary.clone(),
                    round: self.round,
                    votes: BTreeMap::new(),
                });
            }
            TeamEvent::VoteCast {
                by,
                approve,
                reason,
            } => {
                if let Some(proposal) = &mut self.proposal {
                    proposal.votes.insert(
                        by.clone(),
                        Vote {
                            approve: *approve,
                            reason: reason.clone(),
                        },
                    );
                }
            }
            TeamEvent::ProposalResolved { accepted } => {
                if let Some(proposal) = self.proposal.take() {
                    if *accepted {
                        self.accepted_summary = Some(proposal.summary);
                    }
                }
            }
            _ => {}
        }
    }

    #[must_use]
    pub fn channel(&self, name: &str) -> Option<&Channel> {
        self.channels.iter().find(|channel| channel.name == name)
    }

    #[must_use]
    pub fn message(&self, id: MessageId) -> Option<&Message> {
        self.messages.iter().find(|message| message.id == id)
    }

    #[must_use]
    pub fn latest_message_id(&self) -> MessageId {
        self.messages
            .last()
            .map_or(MessageId(0), |message| message.id)
    }

    #[must_use]
    pub fn next_message_id(&self) -> MessageId {
        MessageId(self.latest_message_id().0 + 1)
    }

    /// Messages a member can see, in order.
    pub fn visible_to<'s, 'h>(
        &'s self,
        handle: &'h str,
    ) -> impl Iterator<Item = &'s Message> + use<'s, 'h> {
        self.messages.iter().filter(move |message| {
            self.channel(&message.channel)
                .is_none_or(|channel| channel.includes(handle))
        })
    }

    /// Visible messages posted after the member's previous turn began, excluding its own.
    #[must_use]
    pub fn unseen_by(&self, handle: &str) -> Vec<&Message> {
        let since = self.last_seen.get(handle).copied().unwrap_or(MessageId(0));
        self.visible_to(handle)
            .filter(|message| message.id > since && message.author != handle)
            .collect()
    }

    /// Unseen messages that mention the member (or `@all`).
    #[must_use]
    pub fn unseen_mentions(&self, handle: &str) -> Vec<&Message> {
        self.unseen_by(handle)
            .into_iter()
            .filter(|message| message.mentions(handle))
            .collect()
    }

    /// Bots that still owe a vote on the open proposal.
    #[must_use]
    pub fn pending_voters(&self) -> Vec<String> {
        let Some(proposal) = &self.proposal else {
            return Vec::new();
        };
        self.team
            .bot_approvers()
            .filter(|approver| !proposal.votes.contains_key(*approver))
            .map(str::to_owned)
            .collect()
    }

    #[must_use]
    pub fn message_count(&self) -> u32 {
        u32::try_from(self.messages.len()).unwrap_or(u32::MAX)
    }

    /// The most useful channel to answer a member in: where it was last mentioned, else `#general`.
    #[must_use]
    pub fn reply_channel(&self, handle: &str) -> String {
        self.unseen_mentions(handle)
            .last()
            .map(|message| message.channel.clone())
            .or_else(|| {
                self.unseen_by(handle)
                    .iter()
                    .rev()
                    .find(|message| message.kind != MessageKind::System)
                    .map(|message| message.channel.clone())
            })
            .unwrap_or_else(|| crate::conversation::GENERAL.to_owned())
    }

    #[must_use]
    pub fn build_report(&self) -> MissionReport {
        MissionReport {
            mission: self.id.clone(),
            team: self.team.name.clone(),
            goal: self.goal.statement.clone(),
            status: self.status,
            reason: self.status_reason.clone(),
            rounds: self.round,
            turns: self.turns,
            messages: self.message_count(),
            tasks_total: u32::try_from(self.board.len()).unwrap_or(u32::MAX),
            tasks_done: u32::try_from(self.board.count(crate::TaskStatus::Done))
                .unwrap_or(u32::MAX),
            usage: self.usage,
            members: self.stats.clone(),
            summary: self.accepted_summary.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Goal, MissionId, MissionState, MissionStatus, TeamEvent};
    use crate::{BotProfile, Message, MessageId, TeamSpec, conversation::MessageKind};

    fn started() -> TeamEvent {
        TeamEvent::MissionStarted {
            mission: MissionId("m".to_owned()),
            team: TeamSpec::new(
                "t",
                vec![
                    BotProfile::from_archetype("lead", "lead").expect("lead"),
                    BotProfile::from_archetype("ada", "engineer").expect("eng"),
                ],
            ),
            goal: Goal::new("ship"),
        }
    }

    fn post(id: u64, author: &str, text: &str) -> TeamEvent {
        TeamEvent::MessagePosted {
            message: Message {
                id: MessageId(id),
                channel: "general".to_owned(),
                author: author.to_owned(),
                kind: MessageKind::Chat,
                text: text.to_owned(),
                thread: None,
                mentions: crate::parse_mentions(text),
                round: 1,
            },
        }
    }

    #[test]
    fn replay_requires_mission_start() {
        assert!(MissionState::replay(&[post(1, "lead", "hi")]).is_none());
        assert!(MissionState::replay(&[started()]).is_some());
    }

    #[test]
    fn unseen_mentions_reset_after_turn() {
        let events = vec![
            started(),
            post(1, "lead", "@ada please start"),
            TeamEvent::TurnStarted {
                round: 1,
                bot: "ada".to_owned(),
                reason: "mentioned".to_owned(),
            },
        ];
        let before = MissionState::replay(&events[..2]).expect("state");
        assert_eq!(before.unseen_mentions("ada").len(), 1);
        let after = MissionState::replay(&events).expect("state");
        assert!(after.unseen_mentions("ada").is_empty());
        assert_eq!(after.stats["ada"].turns, 1);
    }

    #[test]
    fn progress_ignores_system_messages() {
        let state =
            MissionState::replay(&[started(), post(1, "system", "note"), post(2, "ada", "x")])
                .expect("state");
        assert_eq!(state.progress, 1);
    }

    #[test]
    fn proposals_track_votes_and_acceptance() {
        let state = MissionState::replay(&[
            started(),
            TeamEvent::ProposalOpened {
                by: "lead".to_owned(),
                summary: "done".to_owned(),
            },
            TeamEvent::VoteCast {
                by: "ada".to_owned(),
                approve: true,
                reason: String::new(),
            },
            TeamEvent::ProposalResolved { accepted: true },
            TeamEvent::StatusChanged {
                status: MissionStatus::Completed,
                reason: "approved".to_owned(),
            },
        ])
        .expect("state");
        assert!(state.proposal.is_none());
        assert_eq!(state.accepted_summary.as_deref(), Some("done"));
        assert!(state.status.is_terminal());
        assert_eq!(state.build_report().summary.as_deref(), Some("done"));
    }

    #[test]
    fn events_round_trip_through_json() {
        let event = post(1, "ada", "hello @lead");
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"event\":\"message_posted\""));
        let back: TeamEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, event);
    }

    #[test]
    fn goals_render_criteria() {
        let goal = Goal::new("Ship").with_criteria(["tests pass"]);
        assert_eq!(goal.render(), "Ship\nSuccess criteria:\n- tests pass");
    }
}
