//! Team specifications: members, lead, channels, approvals, floor policy, and budgets.

use std::{fmt, str::FromStr, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    BotProfile, Channel, TeamError,
    conversation::{GENERAL, valid_channel_name},
    roster::HUMAN,
};

/// Strategy deciding which bots take the floor each round.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FloorPolicyKind {
    /// Each round, the next bot in roster order speaks.
    RoundRobin,
    /// Mentioned bots respond; the lead fills silence; assignees keep working.
    MentionDriven,
    /// The lead plans and integrates; assignees work their tasks in between.
    #[default]
    LeadDirected,
    /// Every bot speaks every round, concurrently.
    Broadcast,
    /// The bots whose expertise best matches the latest conversation speak.
    Expertise,
}

impl FloorPolicyKind {
    pub const ALL: [Self; 5] = [
        Self::RoundRobin,
        Self::MentionDriven,
        Self::LeadDirected,
        Self::Broadcast,
        Self::Expertise,
    ];

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::RoundRobin => "round-robin",
            Self::MentionDriven => "mention-driven",
            Self::LeadDirected => "lead-directed",
            Self::Broadcast => "broadcast",
            Self::Expertise => "expertise",
        }
    }
}

impl fmt::Display for FloorPolicyKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.key())
    }
}

impl FromStr for FloorPolicyKind {
    type Err = TeamError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.key() == value)
            .ok_or_else(|| TeamError::UnknownPolicy(value.to_owned()))
    }
}

/// How bot approvers' votes settle a completion proposal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalRule {
    /// Every approver must approve; any rejection sends the team back to work.
    #[default]
    All,
    /// More than half of the approvers must approve.
    Majority,
    /// One approval is enough; the proposal fails only if every approver rejects.
    Any,
}

impl ApprovalRule {
    pub const ALL: [Self; 3] = [Self::All, Self::Majority, Self::Any];

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Majority => "majority",
            Self::Any => "any",
        }
    }

    /// Settles a vote among `voters` approvers.
    ///
    /// Returns `Some(true)` to accept, `Some(false)` to reject, or `None` while
    /// the outcome still depends on votes not yet cast. Decisions are made as
    /// early as the rule allows, so remaining approvers need not speak.
    #[must_use]
    pub const fn decide(self, approvals: usize, rejections: usize, voters: usize) -> Option<bool> {
        let pending = voters.saturating_sub(approvals + rejections);
        match self {
            Self::All => {
                if rejections > 0 {
                    Some(false)
                } else if pending == 0 {
                    Some(true)
                } else {
                    None
                }
            }
            Self::Any => {
                if approvals > 0 {
                    Some(true)
                } else if pending == 0 {
                    Some(false)
                } else {
                    None
                }
            }
            Self::Majority => {
                let needed = voters / 2 + 1;
                if approvals >= needed {
                    Some(true)
                } else if approvals + pending < needed {
                    Some(false)
                } else {
                    None
                }
            }
        }
    }
}

impl fmt::Display for ApprovalRule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.key())
    }
}

impl FromStr for ApprovalRule {
    type Err = TeamError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|rule| rule.key() == value)
            .ok_or_else(|| {
                TeamError::Config(format!(
                    "unknown approval rule `{value}`; use all, majority, or any"
                ))
            })
    }
}

/// Hard limits that bound a mission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    pub max_rounds: u32,
    pub max_turns: u32,
    pub max_messages: u32,
    /// Total input + output tokens across all bots.
    pub max_tokens: Option<u64>,
    /// Consecutive rounds without progress before the mission is declared stalled.
    pub stall_rounds: u32,
    /// Per-turn time limit in seconds.
    pub turn_timeout_secs: Option<u64>,
    /// How long to wait for a human answer before proceeding without one.
    pub human_timeout_secs: Option<u64>,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_rounds: 12,
            max_turns: 64,
            max_messages: 256,
            max_tokens: None,
            stall_rounds: 3,
            turn_timeout_secs: Some(600),
            human_timeout_secs: Some(300),
        }
    }
}

impl Budget {
    #[must_use]
    pub fn turn_timeout(&self) -> Option<Duration> {
        self.turn_timeout_secs.map(Duration::from_secs)
    }

    #[must_use]
    pub fn human_timeout(&self) -> Option<Duration> {
        self.human_timeout_secs.map(Duration::from_secs)
    }
}

/// A resolved team: every member is a concrete [`BotProfile`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Bot who plans and proposes completion; defaults to the first member.
    #[serde(default)]
    pub lead: Option<String>,
    pub members: Vec<BotProfile>,
    /// Extra channels beyond `#general`.
    #[serde(default)]
    pub channels: Vec<Channel>,
    /// Members whose approval completes the mission (`human` allowed). Empty means the lead decides.
    #[serde(default)]
    pub approvers: Vec<String>,
    /// How approver votes are counted.
    #[serde(default)]
    pub approval: ApprovalRule,
    #[serde(default)]
    pub policy: FloorPolicyKind,
    #[serde(default)]
    pub budget: Budget,
    /// Run the bots selected for a round concurrently instead of one after another.
    #[serde(default)]
    pub parallel_turns: bool,
}

impl TeamSpec {
    #[must_use]
    pub fn new(name: impl Into<String>, members: Vec<BotProfile>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            lead: None,
            members,
            channels: Vec::new(),
            approvers: Vec::new(),
            approval: ApprovalRule::default(),
            policy: FloorPolicyKind::default(),
            budget: Budget::default(),
            parallel_turns: false,
        }
    }

    #[must_use]
    pub fn with_approval(mut self, approval: ApprovalRule) -> Self {
        self.approval = approval;
        self
    }

    #[must_use]
    pub fn with_lead(mut self, lead: impl Into<String>) -> Self {
        self.lead = Some(lead.into());
        self
    }

    #[must_use]
    pub fn with_policy(mut self, policy: FloorPolicyKind) -> Self {
        self.policy = policy;
        self
    }

    #[must_use]
    pub fn with_approvers<I, S>(mut self, approvers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.approvers = approvers.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    #[must_use]
    pub fn with_channel(mut self, channel: Channel) -> Self {
        self.channels.push(channel);
        self
    }

    /// The lead's handle (explicit or first member).
    #[must_use]
    pub fn lead_handle(&self) -> &str {
        self.lead
            .as_deref()
            .or_else(|| self.members.first().map(|member| member.handle.as_str()))
            .unwrap_or_default()
    }

    #[must_use]
    pub fn member(&self, handle: &str) -> Option<&BotProfile> {
        self.members.iter().find(|member| member.handle == handle)
    }

    #[must_use]
    pub fn is_member(&self, handle: &str) -> bool {
        self.member(handle).is_some()
    }

    /// All channels including `#general`, which always comes first.
    #[must_use]
    pub fn all_channels(&self) -> Vec<Channel> {
        let mut channels = vec![Channel::open(
            GENERAL,
            if self.description.is_empty() {
                "Team-wide coordination".to_owned()
            } else {
                self.description.clone()
            },
        )];
        channels.extend(
            self.channels
                .iter()
                .filter(|channel| channel.name != GENERAL)
                .cloned(),
        );
        channels
    }

    pub fn validate(&self) -> Result<(), TeamError> {
        if self.name.trim().is_empty() {
            return Err(TeamError::InvalidTeam("team name is required".to_owned()));
        }
        if self.members.is_empty() {
            return Err(TeamError::InvalidTeam(format!(
                "team `{}` needs at least one bot",
                self.name
            )));
        }
        let mut seen = Vec::new();
        for member in &self.members {
            member.validate()?;
            if seen.contains(&member.handle.as_str()) {
                return Err(TeamError::DuplicateMember(member.handle.clone()));
            }
            seen.push(member.handle.as_str());
        }
        if let Some(lead) = &self.lead {
            if !self.is_member(lead) {
                return Err(TeamError::UnknownMember(lead.clone()));
            }
        }
        for approver in &self.approvers {
            if approver != HUMAN && !self.is_member(approver) {
                return Err(TeamError::UnknownMember(approver.clone()));
            }
        }
        for channel in &self.channels {
            if !valid_channel_name(&channel.name) {
                return Err(TeamError::InvalidChannel(channel.name.clone()));
            }
            for member in &channel.members {
                if member != HUMAN && !self.is_member(member) {
                    return Err(TeamError::UnknownMember(member.clone()));
                }
            }
        }
        if self.budget.max_rounds == 0 || self.budget.max_turns == 0 {
            return Err(TeamError::InvalidTeam(
                "budget max_rounds and max_turns must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }

    /// Whether completion needs the human operator's approval.
    #[must_use]
    pub fn requires_human_approval(&self) -> bool {
        self.approvers.iter().any(|approver| approver == HUMAN)
    }

    /// Bot approvers (excluding the human).
    pub fn bot_approvers(&self) -> impl Iterator<Item = &str> {
        self.approvers
            .iter()
            .map(String::as_str)
            .filter(|approver| *approver != HUMAN)
    }

    /// Who settles completion and how, e.g. `@rev, @qa (all must approve), then @human`.
    #[must_use]
    pub fn approval_summary(&self) -> String {
        let bots = self
            .bot_approvers()
            .map(|approver| format!("@{approver}"))
            .collect::<Vec<_>>();
        let mut summary = match bots.len() {
            0 => String::new(),
            1 => bots[0].clone(),
            _ => {
                let rule = match self.approval {
                    ApprovalRule::All => "all must approve",
                    ApprovalRule::Majority => "majority vote",
                    ApprovalRule::Any => "any one approval",
                };
                format!("{} ({rule})", bots.join(", "))
            }
        };
        if self.requires_human_approval() {
            if !summary.is_empty() {
                summary.push_str(", then ");
            }
            summary.push_str("@human");
        }
        if summary.is_empty() {
            format!("lead decides (@{})", self.lead_handle())
        } else {
            summary
        }
    }
}

/// Parses quick member specs such as `ada:engineer`, `reviewer`, or `ops:analyst`.
///
/// `handle:archetype` builds from the archetype; a bare word must itself be an archetype key.
pub fn parse_member_spec(spec: &str) -> Result<BotProfile, TeamError> {
    let (handle, archetype) = spec
        .split_once(':')
        .map_or((spec, spec), |(handle, archetype)| (handle, archetype));
    let profile = BotProfile::from_archetype(handle.trim(), archetype.trim())?;
    profile.validate()?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::{ApprovalRule, Budget, FloorPolicyKind, TeamSpec, parse_member_spec};
    use crate::{BotProfile, Channel};

    fn team() -> TeamSpec {
        TeamSpec::new(
            "launch",
            vec![
                BotProfile::from_archetype("pm", "lead").expect("lead"),
                BotProfile::from_archetype("ada", "engineer").expect("engineer"),
            ],
        )
    }

    #[test]
    fn lead_defaults_to_first_member() {
        assert_eq!(team().lead_handle(), "pm");
        assert_eq!(team().with_lead("ada").lead_handle(), "ada");
    }

    #[test]
    fn validation_catches_bad_references() {
        assert!(team().validate().is_ok());
        assert!(team().with_lead("ghost").validate().is_err());
        assert!(team().with_approvers(["ghost"]).validate().is_err());
        assert!(team().with_approvers(["human", "ada"]).validate().is_ok());
        let mut duplicate = team();
        duplicate.members.push(BotProfile::new("ada", "copy"));
        assert!(duplicate.validate().is_err());
        assert!(
            team()
                .with_channel(Channel::open("Bad Name", ""))
                .validate()
                .is_err()
        );
        assert!(
            team()
                .with_budget(Budget {
                    max_rounds: 0,
                    ..Budget::default()
                })
                .validate()
                .is_err()
        );
    }

    #[test]
    fn general_channel_is_always_first() {
        let channels = team()
            .with_channel(Channel::open("design", "UX"))
            .all_channels();
        assert_eq!(channels[0].name, "general");
        assert_eq!(channels[1].name, "design");
    }

    #[test]
    fn member_specs_resolve_archetypes() {
        let profile = parse_member_spec("ada:engineer").expect("spec");
        assert_eq!(profile.handle, "ada");
        assert_eq!(
            parse_member_spec("reviewer").expect("spec").handle,
            "reviewer"
        );
        assert!(parse_member_spec("ada:wizard").is_err());
    }

    #[test]
    fn approval_rules_decide_as_early_as_possible() {
        use ApprovalRule::{All, Any, Majority};
        // (approvals, rejections, voters) → decision
        assert_eq!(All.decide(1, 0, 2), None);
        assert_eq!(All.decide(2, 0, 2), Some(true));
        assert_eq!(All.decide(1, 1, 3), Some(false));
        assert_eq!(Any.decide(0, 1, 2), None);
        assert_eq!(Any.decide(1, 0, 3), Some(true));
        assert_eq!(Any.decide(0, 2, 2), Some(false));
        assert_eq!(Majority.decide(2, 0, 3), Some(true));
        assert_eq!(Majority.decide(1, 1, 3), None);
        assert_eq!(Majority.decide(0, 2, 3), Some(false));
        assert_eq!(Majority.decide(1, 1, 2), Some(false), "ties reject");
        for rule in ApprovalRule::ALL {
            assert_eq!(rule.key().parse::<ApprovalRule>().expect("parses"), rule);
        }
        assert!("vibes".parse::<ApprovalRule>().is_err());
    }

    #[test]
    fn approval_summaries_describe_who_decides() {
        assert_eq!(team().approval_summary(), "lead decides (@pm)");
        assert_eq!(team().with_approvers(["ada"]).approval_summary(), "@ada");
        assert_eq!(
            team().with_approvers(["human"]).approval_summary(),
            "@human"
        );
        assert_eq!(
            team()
                .with_approvers(["pm", "ada", "human"])
                .with_approval(ApprovalRule::Majority)
                .approval_summary(),
            "@pm, @ada (majority vote), then @human"
        );
    }

    #[test]
    fn policies_round_trip_through_strings() {
        for kind in FloorPolicyKind::ALL {
            assert_eq!(kind.key().parse::<FloorPolicyKind>().expect("parses"), kind);
        }
        assert!("chaos".parse::<FloorPolicyKind>().is_err());
    }

    #[test]
    fn team_specs_deserialize_from_toml() {
        let spec: TeamSpec = toml::from_str(
            r#"
            name = "docs"
            policy = "mention-driven"
            approvers = ["human"]
            [budget]
            max_rounds = 4
            [[members]]
            handle = "scribe"
            archetype = "writer"
            "#,
        )
        .expect("toml");
        assert_eq!(spec.policy, FloorPolicyKind::MentionDriven);
        assert_eq!(spec.budget.max_rounds, 4);
        assert_eq!(spec.budget.max_turns, Budget::default().max_turns);
        assert!(spec.requires_human_approval());
    }
}
