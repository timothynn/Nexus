//! Bot profiles, built-in archetypes, and member handles.

use std::collections::BTreeMap;

use nexus_permissions::PermissionDecision;
use serde::{Deserialize, Serialize};

use crate::TeamError;

/// Handle used by the human operator in channels and mentions.
pub const HUMAN: &str = "human";
/// Handle used for engine-authored notices.
pub const SYSTEM: &str = "system";

/// How a bot's workspace tools are rooted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceMode {
    /// Tools run against the operator's repository root (read-only tools are recommended).
    #[default]
    Shared,
    /// Tools run inside a dedicated Git worktree owned by the bot.
    Worktree,
}

/// A bot participating in a team.
///
/// Profiles are plain data: they say who the bot is and what it may do, while
/// [`crate::BotBrain`] implementations decide how it thinks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotProfile {
    /// Unique, mention-able handle (`@handle`).
    pub handle: String,
    /// Human-friendly display name.
    #[serde(default)]
    pub name: String,
    /// Job title shown in the roster.
    #[serde(default)]
    pub role: String,
    /// Built-in archetype this profile extends, if any.
    #[serde(default)]
    pub archetype: Option<String>,
    /// Persona and working instructions.
    #[serde(default)]
    pub instructions: String,
    /// Topics the bot is strong at; used by expertise routing.
    #[serde(default)]
    pub expertise: Vec<String>,
    /// Optional per-bot provider override.
    #[serde(default)]
    pub provider: Option<String>,
    /// Optional per-bot model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Workspace tools the bot may call (for example `filesystem.read`).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Workspace isolation for tool execution.
    #[serde(default)]
    pub workspace: WorkspaceMode,
    /// Permission overrides for team actions, keyed by action (`team.goal.propose`).
    #[serde(default)]
    pub permissions: BTreeMap<String, PermissionDecision>,
    /// Display color hint for interfaces (named or `#rrggbb`).
    #[serde(default)]
    pub color: Option<String>,
}

impl BotProfile {
    #[must_use]
    pub fn new(handle: impl Into<String>, role: impl Into<String>) -> Self {
        let handle = handle.into();
        Self {
            name: display_name(&handle),
            handle,
            role: role.into(),
            archetype: None,
            instructions: String::new(),
            expertise: Vec::new(),
            provider: None,
            model: None,
            tools: Vec::new(),
            workspace: WorkspaceMode::Shared,
            permissions: BTreeMap::new(),
            color: None,
        }
    }

    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = instructions.into();
        self
    }

    #[must_use]
    pub fn with_expertise<I, S>(mut self, expertise: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.expertise = expertise.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn with_permission(
        mut self,
        action: impl Into<String>,
        decision: PermissionDecision,
    ) -> Self {
        self.permissions.insert(action.into(), decision);
        self
    }

    /// Builds a profile from a built-in archetype.
    pub fn from_archetype(handle: impl Into<String>, archetype: &str) -> Result<Self, TeamError> {
        let template = Archetype::find(archetype)
            .ok_or_else(|| TeamError::UnknownArchetype(archetype.to_owned()))?;
        let mut profile = Self::new(handle, template.role)
            .with_instructions(template.instructions)
            .with_expertise(template.expertise.iter().copied());
        profile.archetype = Some(template.key.to_owned());
        profile.color = Some(template.color.to_owned());
        Ok(profile)
    }

    /// Fills unset fields from the profile's archetype, keeping explicit values.
    pub fn resolve_archetype(mut self) -> Result<Self, TeamError> {
        let Some(key) = self.archetype.clone() else {
            return Ok(self);
        };
        let template =
            Archetype::find(&key).ok_or_else(|| TeamError::UnknownArchetype(key.clone()))?;
        if self.role.trim().is_empty() {
            template.role.clone_into(&mut self.role);
        }
        if self.instructions.trim().is_empty() {
            template.instructions.clone_into(&mut self.instructions);
        } else {
            self.instructions = format!("{}\n\n{}", template.instructions, self.instructions);
        }
        if self.expertise.is_empty() {
            self.expertise = template
                .expertise
                .iter()
                .map(|&tag| tag.to_owned())
                .collect();
        }
        if self.color.is_none() {
            self.color = Some(template.color.to_owned());
        }
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), TeamError> {
        validate_handle(&self.handle)?;
        if matches!(self.handle.as_str(), HUMAN | SYSTEM) {
            return Err(TeamError::ReservedHandle(self.handle.clone()));
        }
        Ok(())
    }

    /// One-line roster description: `@handle (Name, Role)`.
    #[must_use]
    pub fn label(&self) -> String {
        if self.role.is_empty() {
            format!("@{} ({})", self.handle, self.name)
        } else {
            format!("@{} ({}, {})", self.handle, self.name, self.role)
        }
    }
}

/// Mention handles are lowercase ASCII words so they can be parsed unambiguously.
pub fn validate_handle(handle: &str) -> Result<(), TeamError> {
    let valid = !handle.is_empty()
        && handle.len() <= 32
        && handle.starts_with(|c: char| c.is_ascii_lowercase())
        && handle
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(TeamError::InvalidHandle(handle.to_owned()))
    }
}

fn display_name(handle: &str) -> String {
    handle
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            // Short handles read as acronyms: `pm` → `PM`, `qa` → `QA`.
            if part.len() <= 2 {
                return part.to_ascii_uppercase();
            }
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A reusable bot template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Archetype {
    pub key: &'static str,
    pub role: &'static str,
    pub summary: &'static str,
    pub instructions: &'static str,
    pub expertise: &'static [&'static str],
    pub color: &'static str,
}

impl Archetype {
    #[must_use]
    pub fn all() -> &'static [Archetype] {
        ARCHETYPES
    }

    #[must_use]
    pub fn find(key: &str) -> Option<&'static Archetype> {
        ARCHETYPES.iter().find(|archetype| archetype.key == key)
    }
}

const ARCHETYPES: &[Archetype] = &[
    Archetype {
        key: "lead",
        role: "Team lead",
        summary: "Plans the mission, assigns tasks, integrates results, and proposes completion.",
        instructions: "You lead this team. Break the goal into concrete tasks, assign each to the best-suited teammate, keep everyone unblocked, integrate their results, and propose completion only when the success criteria are demonstrably met.",
        expertise: &[
            "planning",
            "coordination",
            "prioritization",
            "scope",
            "delivery",
        ],
        color: "magenta",
    },
    Archetype {
        key: "architect",
        role: "Software architect",
        summary: "Designs module boundaries, interfaces, and trade-offs.",
        instructions: "You own the design. Propose module boundaries, interfaces, data flow, and trade-offs. Flag risks early and keep designs as simple as the goal allows.",
        expertise: &[
            "architecture",
            "design",
            "interfaces",
            "api",
            "modules",
            "trade-offs",
        ],
        color: "blue",
    },
    Archetype {
        key: "engineer",
        role: "Software engineer",
        summary: "Implements tasks and reports concrete results.",
        instructions: "You implement. Take assigned tasks, do the work with the tools you have, and report concrete results: what changed, where, and how it was verified.",
        expertise: &["implementation", "code", "rust", "refactor", "bug", "build"],
        color: "green",
    },
    Archetype {
        key: "researcher",
        role: "Researcher",
        summary: "Gathers facts, prior art, and constraints.",
        instructions: "You investigate. Gather facts, prior art, and constraints relevant to the goal. Cite where each finding came from and separate facts from assumptions.",
        expertise: &[
            "research",
            "investigation",
            "docs",
            "requirements",
            "analysis",
            "sources",
        ],
        color: "cyan",
    },
    Archetype {
        key: "reviewer",
        role: "Reviewer",
        summary: "Checks work against the goal and approves or rejects completion.",
        instructions: "You review. Check every result against the goal and success criteria. Be specific about defects. Vote to approve completion only when the work is genuinely done.",
        expertise: &["review", "quality", "correctness", "security", "standards"],
        color: "yellow",
    },
    Archetype {
        key: "tester",
        role: "QA engineer",
        summary: "Designs and runs tests, reports regressions.",
        instructions: "You test. Design test cases for the goal, run them when tools allow, and report failures with reproduction steps.",
        expertise: &[
            "testing",
            "tests",
            "qa",
            "regression",
            "edge-cases",
            "verification",
        ],
        color: "red",
    },
    Archetype {
        key: "writer",
        role: "Technical writer",
        summary: "Produces documentation, summaries, and release notes.",
        instructions: "You write. Turn the team's work into clear documentation, summaries, and release notes for the intended audience.",
        expertise: &[
            "documentation",
            "docs",
            "writing",
            "readme",
            "summary",
            "release-notes",
        ],
        color: "white",
    },
    Archetype {
        key: "critic",
        role: "Devil's advocate",
        summary: "Challenges assumptions and stress-tests plans.",
        instructions: "You challenge. Question assumptions, look for failure modes, and argue the strongest alternative. Be constructive: every objection comes with a proposed fix.",
        expertise: &[
            "risk",
            "assumptions",
            "failure-modes",
            "alternatives",
            "critique",
        ],
        color: "lightred",
    },
    Archetype {
        key: "designer",
        role: "Product designer",
        summary: "Shapes user experience, flows, and copy.",
        instructions: "You design the experience. Describe user flows, interface layouts, states, and copy. Advocate for the user's needs.",
        expertise: &["ux", "ui", "design", "flows", "copy", "accessibility"],
        color: "lightmagenta",
    },
    Archetype {
        key: "analyst",
        role: "Data analyst",
        summary: "Quantifies, measures, and validates with data.",
        instructions: "You quantify. Define metrics, analyze data, and validate claims with numbers. State your confidence and the limits of the data.",
        expertise: &[
            "data",
            "metrics",
            "analysis",
            "sql",
            "statistics",
            "measurement",
        ],
        color: "lightblue",
    },
];

#[cfg(test)]
mod tests {
    use super::{Archetype, BotProfile, validate_handle};

    #[test]
    fn handles_are_strict() {
        assert!(validate_handle("coder").is_ok());
        assert!(validate_handle("qa-2").is_ok());
        assert!(validate_handle("Coder").is_err());
        assert!(validate_handle("2fast").is_err());
        assert!(validate_handle("a b").is_err());
        assert!(validate_handle("").is_err());
    }

    #[test]
    fn reserved_handles_are_rejected() {
        assert!(BotProfile::new("human", "x").validate().is_err());
        assert!(BotProfile::new("system", "x").validate().is_err());
    }

    #[test]
    fn archetype_profiles_inherit_template() {
        let profile = BotProfile::from_archetype("ada", "engineer").expect("archetype exists");
        assert_eq!(profile.role, "Software engineer");
        assert_eq!(profile.name, "Ada");
        assert!(profile.expertise.iter().any(|tag| tag == "rust"));
    }

    #[test]
    fn explicit_fields_survive_archetype_resolution() {
        let mut profile =
            BotProfile::new("rev", "Security reviewer").with_instructions("Focus on auth.");
        profile.archetype = Some("reviewer".to_owned());
        let resolved = profile.resolve_archetype().expect("resolves");
        assert_eq!(resolved.role, "Security reviewer");
        assert!(resolved.instructions.contains("You review."));
        assert!(resolved.instructions.ends_with("Focus on auth."));
    }

    #[test]
    fn archetype_keys_are_unique_and_valid_handles() {
        let mut keys = Archetype::all().iter().map(|a| a.key).collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Archetype::all().len());
        assert!(keys.iter().all(|key| validate_handle(key).is_ok()));
    }

    #[test]
    fn display_names_are_derived_from_handles() {
        assert_eq!(BotProfile::new("data-wiz", "x").name, "Data Wiz");
        assert_eq!(BotProfile::new("pm", "x").name, "PM");
    }
}
