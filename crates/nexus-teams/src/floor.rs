//! Floor control: deciding which bots take a turn each round.

use std::collections::BTreeSet;

use crate::{BotProfile, FloorPolicyKind, MissionState, conversation::MessageKind};

/// A bot selected to take a turn, with the reason it was given the floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speaker {
    pub handle: String,
    pub reason: String,
}

impl Speaker {
    #[must_use]
    pub fn new(handle: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            handle: handle.into(),
            reason: reason.into(),
        }
    }
}

/// Selects speakers for the next round. Implementations may keep internal cursors.
pub trait FloorPolicy: Send {
    fn kind(&self) -> FloorPolicyKind;

    /// Returns speakers in the order they should act. An empty result ends the round quietly.
    fn select(&mut self, state: &MissionState) -> Vec<Speaker>;
}

/// Builds the policy implementation for a policy kind.
#[must_use]
pub fn policy_for(kind: FloorPolicyKind) -> Box<dyn FloorPolicy> {
    match kind {
        FloorPolicyKind::RoundRobin => Box::new(RoundRobin::default()),
        FloorPolicyKind::MentionDriven => Box::new(MentionDriven),
        FloorPolicyKind::LeadDirected => Box::new(LeadDirected),
        FloorPolicyKind::Broadcast => Box::new(Broadcast),
        FloorPolicyKind::Expertise => Box::new(ExpertiseRouting::default()),
    }
}

/// Collects speakers without duplicates, ordered by roster position.
struct SpeakerSet<'a> {
    state: &'a MissionState,
    chosen: Vec<Speaker>,
}

impl<'a> SpeakerSet<'a> {
    fn new(state: &'a MissionState) -> Self {
        Self {
            state,
            chosen: Vec::new(),
        }
    }

    fn add(&mut self, handle: &str, reason: impl Into<String>) {
        if self.state.team.is_member(handle) && !self.chosen.iter().any(|s| s.handle == handle) {
            self.chosen.push(Speaker::new(handle, reason));
        }
    }

    fn add_mentioned(&mut self) {
        for member in &self.state.team.members {
            let mentions = self.state.unseen_mentions(&member.handle);
            if let Some(message) = mentions.last() {
                self.add(
                    &member.handle,
                    format!("mentioned by @{} in #{}", message.author, message.channel),
                );
            }
        }
    }

    fn add_assignees(&mut self) {
        for member in &self.state.team.members {
            let tasks = self.state.board.actionable_for(&member.handle);
            if let Some(task) = tasks.first() {
                self.add(
                    &member.handle,
                    format!("assigned {} ({})", task.id, task.title),
                );
            }
        }
    }

    fn finish(mut self) -> Vec<Speaker> {
        let order = |handle: &str| {
            self.state
                .team
                .members
                .iter()
                .position(|member| member.handle == handle)
                .unwrap_or(usize::MAX)
        };
        self.chosen.sort_by_key(|speaker| order(&speaker.handle));
        self.chosen
    }
}

/// One bot per round, cycling through the roster.
#[derive(Debug, Default)]
pub struct RoundRobin {
    cursor: usize,
}

impl FloorPolicy for RoundRobin {
    fn kind(&self) -> FloorPolicyKind {
        FloorPolicyKind::RoundRobin
    }

    fn select(&mut self, state: &MissionState) -> Vec<Speaker> {
        let members = &state.team.members;
        if members.is_empty() {
            return Vec::new();
        }
        let member = &members[self.cursor % members.len()];
        self.cursor += 1;
        vec![Speaker::new(&member.handle, "round-robin turn")]
    }
}

/// Mentioned bots answer and assignees keep working; the lead fills silence.
#[derive(Debug, Default)]
pub struct MentionDriven;

impl FloorPolicy for MentionDriven {
    fn kind(&self) -> FloorPolicyKind {
        FloorPolicyKind::MentionDriven
    }

    fn select(&mut self, state: &MissionState) -> Vec<Speaker> {
        let mut set = SpeakerSet::new(state);
        set.add_mentioned();
        set.add_assignees();
        if set.chosen.is_empty() {
            set.add(
                state.team.lead_handle(),
                "nobody has the floor; lead keeps the mission moving",
            );
        }
        set.finish()
    }
}

/// The lead plans and integrates; assignees work in between.
#[derive(Debug, Default)]
pub struct LeadDirected;

impl FloorPolicy for LeadDirected {
    fn kind(&self) -> FloorPolicyKind {
        FloorPolicyKind::LeadDirected
    }

    fn select(&mut self, state: &MissionState) -> Vec<Speaker> {
        let lead = state.team.lead_handle();
        let mut set = SpeakerSet::new(state);
        if state.board.is_empty() && state.proposal.is_none() {
            set.add(lead, "plan the mission and assign work");
            return set.finish();
        }
        set.add_mentioned();
        set.add_assignees();
        let lead_has_news = !state.unseen_mentions(lead).is_empty();
        let others_working = set.chosen.iter().any(|speaker| speaker.handle != lead);
        if !others_working && !lead_has_news {
            let reason = if state.board.all_done() {
                "all tasks are closed; integrate results and decide on completion"
            } else {
                "no one is working; re-plan, unblock, or reassign"
            };
            set.add(lead, reason);
        }
        set.finish()
    }
}

/// Every bot speaks every round.
#[derive(Debug, Default)]
pub struct Broadcast;

impl FloorPolicy for Broadcast {
    fn kind(&self) -> FloorPolicyKind {
        FloorPolicyKind::Broadcast
    }

    fn select(&mut self, state: &MissionState) -> Vec<Speaker> {
        state
            .team
            .members
            .iter()
            .map(|member| Speaker::new(&member.handle, "broadcast round"))
            .collect()
    }
}

/// Routes the floor to the bots whose expertise best matches recent conversation.
#[derive(Debug)]
pub struct ExpertiseRouting {
    /// Maximum bots selected by relevance (mentions and assignees are always added).
    pub max_speakers: usize,
    /// How many recent messages are scored.
    pub window: usize,
}

impl Default for ExpertiseRouting {
    fn default() -> Self {
        Self {
            max_speakers: 2,
            window: 6,
        }
    }
}

impl ExpertiseRouting {
    /// Scores each member by overlap between its expertise/role words and recent text.
    #[must_use]
    pub fn scores(&self, state: &MissionState) -> Vec<(String, usize)> {
        let mut recent = state
            .messages
            .iter()
            .rev()
            .filter(|message| message.kind != MessageKind::System)
            .take(self.window)
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>();
        if recent.is_empty() {
            recent.push(&state.goal.statement);
        }
        let words = recent
            .iter()
            .flat_map(|text| tokenize(text))
            .collect::<BTreeSet<_>>();
        state
            .team
            .members
            .iter()
            .map(|member| (member.handle.clone(), expertise_overlap(member, &words)))
            .collect()
    }
}

/// How many of `words` match a member's expertise tags or role.
pub(crate) fn expertise_overlap(member: &BotProfile, words: &BTreeSet<String>) -> usize {
    member
        .expertise
        .iter()
        .flat_map(|tag| tokenize(tag))
        .chain(tokenize(&member.role))
        .collect::<BTreeSet<_>>()
        .intersection(words)
        .count()
}

impl FloorPolicy for ExpertiseRouting {
    fn kind(&self) -> FloorPolicyKind {
        FloorPolicyKind::Expertise
    }

    fn select(&mut self, state: &MissionState) -> Vec<Speaker> {
        let mut set = SpeakerSet::new(state);
        set.add_mentioned();
        set.add_assignees();
        let mut scores = self
            .scores(state)
            .into_iter()
            .filter(|(_, score)| *score > 0)
            .collect::<Vec<_>>();
        scores.sort_by(|left, right| right.1.cmp(&left.1));
        for (handle, score) in scores.into_iter().take(self.max_speakers) {
            set.add(&handle, format!("expertise match (score {score})"));
        }
        if set.chosen.is_empty() {
            set.add(
                state.team.lead_handle(),
                "no expertise match; lead keeps the mission moving",
            );
        }
        set.finish()
    }
}

/// Lowercased words longer than two characters, with a trailing plural `s` removed.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() > 2)
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            lower
                .strip_suffix('s')
                .filter(|stem| stem.len() > 2)
                .map_or_else(|| lower.clone(), str::to_owned)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        Broadcast, ExpertiseRouting, FloorPolicy, LeadDirected, MentionDriven, RoundRobin,
    };
    use crate::{
        BoardTask, BotProfile, Goal, Message, MissionId, MissionState, TaskId, TaskStatus,
        TeamEvent, TeamSpec, conversation::MessageKind, parse_mentions,
    };

    fn state() -> MissionState {
        MissionState::new(
            MissionId("m".to_owned()),
            TeamSpec::new(
                "t",
                vec![
                    BotProfile::from_archetype("lead", "lead").expect("lead"),
                    BotProfile::from_archetype("ada", "engineer").expect("engineer"),
                    BotProfile::from_archetype("qa", "tester").expect("tester"),
                ],
            ),
            Goal::new("Write regression tests for the parser"),
        )
    }

    fn post(state: &mut MissionState, author: &str, text: &str) {
        let id = state.next_message_id();
        state.apply(&TeamEvent::MessagePosted {
            message: Message {
                id,
                channel: "general".to_owned(),
                author: author.to_owned(),
                kind: MessageKind::Chat,
                text: text.to_owned(),
                thread: None,
                mentions: parse_mentions(text),
                round: 1,
            },
        });
    }

    fn assign(state: &mut MissionState, id: u32, assignee: &str, status: TaskStatus) {
        state.apply(&TeamEvent::TaskCreated {
            task: BoardTask {
                id: TaskId(id),
                title: format!("task {id}"),
                description: String::new(),
                assignee: Some(assignee.to_owned()),
                status,
                depends_on: Vec::new(),
                created_by: "lead".to_owned(),
                note: None,
            },
        });
    }

    fn handles(speakers: &[super::Speaker]) -> Vec<&str> {
        speakers.iter().map(|s| s.handle.as_str()).collect()
    }

    #[test]
    fn round_robin_cycles() {
        let state = state();
        let mut policy = RoundRobin::default();
        let picks = (0..4)
            .map(|_| policy.select(&state)[0].handle.clone())
            .collect::<Vec<_>>();
        assert_eq!(picks, vec!["lead", "ada", "qa", "lead"]);
    }

    #[test]
    fn lead_directed_starts_with_lead_then_assignees() {
        let mut state = state();
        assert_eq!(handles(&LeadDirected.select(&state)), vec!["lead"]);
        assign(&mut state, 1, "ada", TaskStatus::Todo);
        assign(&mut state, 2, "qa", TaskStatus::InProgress);
        assert_eq!(handles(&LeadDirected.select(&state)), vec!["ada", "qa"]);
    }

    #[test]
    fn lead_directed_returns_to_lead_when_work_is_done() {
        let mut state = state();
        assign(&mut state, 1, "ada", TaskStatus::Done);
        let speakers = LeadDirected.select(&state);
        assert_eq!(handles(&speakers), vec!["lead"]);
        assert!(speakers[0].reason.contains("all tasks are closed"));
    }

    #[test]
    fn mention_driven_answers_mentions_in_roster_order() {
        let mut state = state();
        post(&mut state, "lead", "@qa and @ada, thoughts?");
        assert_eq!(handles(&MentionDriven.select(&state)), vec!["ada", "qa"]);
    }

    #[test]
    fn mention_driven_falls_back_to_lead() {
        assert_eq!(handles(&MentionDriven.select(&state())), vec!["lead"]);
    }

    #[test]
    fn broadcast_selects_everyone() {
        assert_eq!(Broadcast.select(&state()).len(), 3);
    }

    #[test]
    fn expertise_routes_to_matching_bots() {
        let mut state = state();
        post(
            &mut state,
            "lead",
            "We need regression tests and edge-cases verified",
        );
        let mut policy = ExpertiseRouting::default();
        let speakers = policy.select(&state);
        assert_eq!(speakers[0].handle, "qa");
    }

    #[test]
    fn mentions_of_all_reach_everyone() {
        let mut state = state();
        post(&mut state, "lead", "@all standup please");
        assert_eq!(MentionDriven.select(&state).len(), 2);
    }
}
