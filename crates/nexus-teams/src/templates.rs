//! Built-in team templates: ready-made squads for common kinds of goals.

use crate::{ApprovalRule, Budget, Channel, FloorPolicyKind, MemberEntry, TeamFile};

/// A reusable team blueprint. Members are member specs (`handle:archetype`),
/// so project bot files with the same handle override the archetype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamTemplate {
    pub key: &'static str,
    pub summary: &'static str,
    pub description: &'static str,
    pub members: &'static [&'static str],
    pub lead: &'static str,
    pub approvers: &'static [&'static str],
    pub approval: ApprovalRule,
    pub policy: FloorPolicyKind,
    pub parallel_turns: bool,
    pub channels: &'static [(&'static str, &'static str)],
}

impl TeamTemplate {
    #[must_use]
    pub fn all() -> &'static [TeamTemplate] {
        TEMPLATES
    }

    #[must_use]
    pub fn find(key: &str) -> Option<&'static TeamTemplate> {
        TEMPLATES.iter().find(|template| template.key == key)
    }

    /// The template as an editable team file named `name`.
    #[must_use]
    pub fn team_file(&self, name: &str) -> TeamFile {
        TeamFile {
            name: name.to_owned(),
            description: self.description.to_owned(),
            lead: Some(self.lead.to_owned()),
            members: self
                .members
                .iter()
                .map(|member| MemberEntry::Reference((*member).to_owned()))
                .collect(),
            channels: self
                .channels
                .iter()
                .map(|(channel, purpose)| Channel::open(*channel, *purpose))
                .collect(),
            approvers: self
                .approvers
                .iter()
                .map(|&approver| approver.to_owned())
                .collect(),
            approval: self.approval,
            policy: self.policy,
            budget: Budget::default(),
            parallel_turns: self.parallel_turns,
        }
    }
}

const TEMPLATES: &[TeamTemplate] = &[
    TeamTemplate {
        key: "software",
        summary: "Plan, build, test, and review a code change",
        description: "Software squad: design, implementation, verification, and review",
        members: &[
            "lead:lead",
            "arch:architect",
            "ada:engineer",
            "qa:tester",
            "rev:reviewer",
        ],
        lead: "lead",
        approvers: &["rev", "qa"],
        approval: ApprovalRule::All,
        policy: FloorPolicyKind::LeadDirected,
        parallel_turns: false,
        channels: &[
            ("design", "Architecture and interface decisions"),
            ("review", "Code review and verification"),
        ],
    },
    TeamTemplate {
        key: "research",
        summary: "Investigate a question and write up the findings",
        description: "Research cell: gather sources, quantify, challenge, and summarize",
        members: &[
            "lead:lead",
            "scout:researcher",
            "quant:analyst",
            "skeptic:critic",
            "scribe:writer",
        ],
        lead: "lead",
        approvers: &["skeptic", "scribe", "quant"],
        approval: ApprovalRule::Majority,
        policy: FloorPolicyKind::MentionDriven,
        parallel_turns: false,
        channels: &[("sources", "Evidence, citations, and data")],
    },
    TeamTemplate {
        key: "content",
        summary: "Draft, design, and edit a piece of content",
        description: "Content studio: write, shape the experience, and edit",
        members: &[
            "lead:lead",
            "scribe:writer",
            "ux:designer",
            "editor:reviewer",
        ],
        lead: "lead",
        approvers: &["editor"],
        approval: ApprovalRule::All,
        policy: FloorPolicyKind::LeadDirected,
        parallel_turns: false,
        channels: &[("drafts", "Work-in-progress drafts")],
    },
    TeamTemplate {
        key: "incident",
        summary: "Coordinate an incident response with human sign-off",
        description: "Incident room: triage, mitigate, measure, and communicate",
        members: &[
            "lead:lead",
            "sre:engineer",
            "quant:analyst",
            "scribe:writer",
        ],
        lead: "lead",
        approvers: &["human"],
        approval: ApprovalRule::All,
        policy: FloorPolicyKind::MentionDriven,
        parallel_turns: false,
        channels: &[("timeline", "Incident timeline and decisions")],
    },
    TeamTemplate {
        key: "debate",
        summary: "Argue both sides of a decision, then judge it",
        description: "Design debate: advocate, challenge, and judge",
        members: &["lead:lead", "pro:architect", "con:critic", "judge:reviewer"],
        lead: "lead",
        approvers: &["judge"],
        approval: ApprovalRule::Any,
        policy: FloorPolicyKind::Broadcast,
        parallel_turns: true,
        channels: &[],
    },
];

#[cfg(test)]
mod tests {
    use super::TeamTemplate;
    use crate::TeamLibrary;

    #[test]
    fn every_template_resolves_to_a_valid_team() {
        let library = TeamLibrary::new(std::env::temp_dir().join("nexus-templates-no-such-root"));
        let mut keys = Vec::new();
        for template in TeamTemplate::all() {
            let team = library
                .resolve_team(template.team_file(template.key))
                .unwrap_or_else(|error| panic!("{}: {error}", template.key));
            assert_eq!(team.lead_handle(), template.lead);
            assert_eq!(team.members.len(), template.members.len());
            keys.push(template.key);
        }
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(
            keys.len(),
            TeamTemplate::all().len(),
            "template keys are unique"
        );
        assert!(TeamTemplate::find("software").is_some());
        assert!(TeamTemplate::find("nope").is_none());
    }
}
