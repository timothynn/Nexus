//! Teams of collaborating bots.
//!
//! Nexus Teams models a chat workspace — think Microsoft Teams — where the
//! members are bots. The user assembles a [`TeamSpec`] of [`BotProfile`]s,
//! states a [`Goal`], and a [`MissionEngine`] runs the team toward it:
//!
//! ```text
//! Goal ─▶ Floor policy picks speakers ─▶ BotBrain turns ─▶ permissioned actions
//!   ▲                                                         │
//!   └──── votes / human approval ◀── completion proposal ◀────┘
//! ```
//!
//! * Bots talk in channels, threads, and direct messages, and `@mention` each other.
//! * A Planner-style [`TaskBoard`] tracks tasks, assignees, and dependencies.
//! * [`floor`] policies decide who speaks: round-robin, mention-driven,
//!   lead-directed, broadcast, or expertise routing.
//! * Every bot action is authorized through `nexus-permissions`.
//! * Mission state is event-sourced ([`TeamEvent`]), so persisted logs replay exactly.
//! * Missions are bounded by budgets, stall detection, cancellation, and
//!   optional human approval of completion.

pub mod board;
pub mod brain;
pub mod context;
pub mod conversation;
pub mod engine;
pub mod floor;
pub mod library;
pub mod mission;
pub mod operator;
pub mod persistence;
pub mod protocol;
pub mod roster;
pub mod runtime_brain;
pub mod team;
pub mod templates;
pub mod transcript;

use std::time::Duration;

pub use board::{BoardTask, TaskBoard, TaskId, TaskStatus};
pub use brain::{
    ActivityReporter, BotBrain, BrainRouter, PacedBrain, ScriptedBrain, SimulatedBrain, TurnContext,
};
pub use conversation::{Channel, Message, MessageId, MessageKind, parse_mentions};
pub use engine::{
    ChannelEventSink, ControlInput, MemoryEventSink, MissionControl, MissionEngine, ResumeOptions,
    TeamEventSink,
};
pub use floor::{FloorPolicy, Speaker};
pub use library::{MemberEntry, TeamFile, TeamLibrary, TeamOverrides};
pub use mission::{
    Goal, MemberStats, MissionId, MissionReport, MissionState, MissionStatus, Proposal, TeamEvent,
};
pub use operator::{OPERATOR_HELP, OperatorCommand, parse_operator_line};
pub use persistence::{
    MissionSummary, SqliteTeamSink, list_missions, load_mission, resolve_mission_id,
};
pub use protocol::{BotAction, BotTurn, TurnUsage, parse_reply};
pub use roster::{Archetype, BotProfile, HUMAN, SYSTEM, WorkspaceMode};
pub use runtime_brain::{ExecutorFactory, ProviderFactory, RuntimeBrain};
pub use team::{ApprovalRule, Budget, FloorPolicyKind, TeamSpec, parse_member_spec};
pub use templates::TeamTemplate;

#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    #[error("unknown bot archetype `{0}`; run `nexus bots archetypes` to list them")]
    UnknownArchetype(String),
    #[error(
        "invalid handle `{0}`: use lowercase letters, digits, `-` or `_`, starting with a letter"
    )]
    InvalidHandle(String),
    #[error("handle `{0}` is reserved")]
    ReservedHandle(String),
    #[error("invalid team: {0}")]
    InvalidTeam(String),
    #[error("duplicate team member `{0}`")]
    DuplicateMember(String),
    #[error("unknown team member `{0}`")]
    UnknownMember(String),
    #[error("invalid channel `{0}`")]
    InvalidChannel(String),
    #[error("unknown floor policy `{0}`")]
    UnknownPolicy(String),
    #[error("invalid task id `{0}`")]
    InvalidTaskId(String),
    #[error("unknown task {0}")]
    UnknownTask(TaskId),
    #[error("task {0} would create a dependency cycle")]
    TaskCycle(TaskId),
    #[error("bot brain failed: {0}")]
    Brain(String),
    #[error("bot turn timed out after {0:?}")]
    TurnTimeout(Duration),
    #[error("mission was cancelled")]
    Cancelled,
    #[error("mission is no longer accepting input")]
    MissionClosed,
    #[error("mission already completed; resume it with a note describing the follow-up work")]
    AlreadyCompleted,
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0} already exists")]
    AlreadyExists(String),
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("teams I/O failed: {0}")]
    Io(std::io::Error),
}
