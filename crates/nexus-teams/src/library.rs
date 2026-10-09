//! Project-local bot and team definitions under `.nexus/bots` and `.nexus/teams`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    ApprovalRule, BotProfile, Budget, Channel, FloorPolicyKind, TeamError, TeamSpec, TeamTemplate,
    roster::validate_handle, team::parse_member_spec,
};

/// A team member as written in a team file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MemberEntry {
    /// `"ada"` (a bot file or archetype key) or `"ada:engineer"`.
    Reference(String),
    /// A full inline profile.
    Inline(Box<BotProfile>),
}

/// On-disk team definition; members are resolved into profiles when loaded.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TeamFile {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    pub members: Vec<MemberEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<Channel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvers: Vec<String>,
    #[serde(default)]
    pub approval: ApprovalRule,
    #[serde(default)]
    pub policy: FloorPolicyKind,
    #[serde(default)]
    pub budget: Budget,
    #[serde(default)]
    pub parallel_turns: bool,
}

/// Adjustments applied on top of a saved team (or an empty ad-hoc team).
///
/// Interfaces map their flags onto this so every surface assembles teams the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamOverrides {
    /// Extra members: bot file handles, archetype keys, or `handle:archetype`.
    pub members: Vec<String>,
    pub lead: Option<String>,
    /// Replaces the approvers when non-empty.
    pub approvers: Vec<String>,
    pub approval: Option<ApprovalRule>,
    pub policy: Option<FloorPolicyKind>,
    /// Built-in template used when no saved team is named.
    pub template: Option<String>,
    pub max_rounds: Option<u32>,
    pub max_turns: Option<u32>,
    pub max_tokens: Option<u64>,
    pub stall_rounds: Option<u32>,
    pub turn_timeout_secs: Option<u64>,
    pub parallel_turns: bool,
    /// Workspace tools granted to every member.
    pub grant_tools: Vec<String>,
}

/// Loads and saves bots and teams for a project root.
#[derive(Debug, Clone)]
pub struct TeamLibrary {
    root: PathBuf,
}

impl TeamLibrary {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn bots_dir(&self) -> PathBuf {
        self.root.join(".nexus").join("bots")
    }

    #[must_use]
    pub fn teams_dir(&self) -> PathBuf {
        self.root.join(".nexus").join("teams")
    }

    #[must_use]
    pub fn bot_path(&self, handle: &str) -> PathBuf {
        self.bots_dir().join(format!("{handle}.toml"))
    }

    #[must_use]
    pub fn team_path(&self, name: &str) -> PathBuf {
        self.teams_dir().join(format!("{name}.toml"))
    }

    pub fn list_bots(&self) -> Result<Vec<BotProfile>, TeamError> {
        toml_stems(&self.bots_dir())?
            .into_iter()
            .map(|handle| self.load_bot(&handle))
            .collect()
    }

    pub fn load_bot(&self, handle: &str) -> Result<BotProfile, TeamError> {
        validate_handle(handle)?;
        let path = self.bot_path(handle);
        if !path.is_file() {
            return Err(TeamError::NotFound(format!("bot `{handle}`")));
        }
        let mut profile: BotProfile = read_toml(&path)?;
        if profile.handle.is_empty() {
            handle.clone_into(&mut profile.handle);
        }
        let profile = profile.resolve_archetype()?;
        profile.validate()?;
        Ok(profile)
    }

    /// Resolves a member reference: a bot file wins over an archetype of the same name.
    pub fn resolve_member(&self, reference: &str) -> Result<BotProfile, TeamError> {
        if !reference.contains(':') && self.bot_path(reference).is_file() {
            return self.load_bot(reference);
        }
        parse_member_spec(reference)
    }

    pub fn save_bot(&self, profile: &BotProfile, overwrite: bool) -> Result<PathBuf, TeamError> {
        profile.validate()?;
        let path = self.bot_path(&profile.handle);
        write_toml(&path, profile, overwrite)?;
        Ok(path)
    }

    pub fn list_teams(&self) -> Result<Vec<String>, TeamError> {
        toml_stems(&self.teams_dir())
    }

    pub fn load_team_file(&self, name: &str) -> Result<TeamFile, TeamError> {
        let path = self.team_path(name);
        if !path.is_file() {
            return Err(TeamError::NotFound(format!("team `{name}`")));
        }
        read_toml(&path)
    }

    pub fn load_team(&self, name: &str) -> Result<TeamSpec, TeamError> {
        self.resolve_team(self.load_team_file(name)?)
    }

    pub fn resolve_team(&self, file: TeamFile) -> Result<TeamSpec, TeamError> {
        let members = file
            .members
            .into_iter()
            .map(|entry| match entry {
                MemberEntry::Reference(reference) => self.resolve_member(&reference),
                MemberEntry::Inline(profile) => profile.resolve_archetype(),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let spec = TeamSpec {
            name: file.name,
            description: file.description,
            lead: file.lead,
            members,
            channels: file.channels,
            approvers: file.approvers,
            approval: file.approval,
            policy: file.policy,
            budget: file.budget,
            parallel_turns: file.parallel_turns,
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Builds a validated team from an optional saved team plus overrides.
    ///
    /// Without a saved team, the overrides must name at least one member and an
    /// ad-hoc team is assembled from them.
    pub fn assemble(
        &self,
        base: Option<&str>,
        overrides: &TeamOverrides,
    ) -> Result<TeamSpec, TeamError> {
        let template = overrides
            .template
            .as_deref()
            .map(|key| {
                TeamTemplate::find(key)
                    .ok_or_else(|| TeamError::NotFound(format!("team template `{key}`")))
            })
            .transpose()?;
        let mut team = match (base, template) {
            (Some(name), _) => self.load_team(name)?,
            (None, Some(template)) => self.resolve_team(template.team_file(template.key))?,
            (None, None) if overrides.members.is_empty() => {
                return Err(TeamError::InvalidTeam(
                    "name a saved team or add members (for example pm:lead and ada:engineer)"
                        .to_owned(),
                ));
            }
            (None, None) => TeamSpec::new("ad-hoc", Vec::new()),
        };
        for reference in &overrides.members {
            team.members.push(self.resolve_member(reference)?);
        }
        if let Some(lead) = &overrides.lead {
            team.lead = Some(lead.clone());
        }
        if !overrides.approvers.is_empty() {
            team.approvers.clone_from(&overrides.approvers);
        }
        if let Some(approval) = overrides.approval {
            team.approval = approval;
        }
        if let Some(policy) = overrides.policy {
            team.policy = policy;
        }
        team.parallel_turns |= overrides.parallel_turns;
        let budget = &mut team.budget;
        if let Some(value) = overrides.max_rounds {
            budget.max_rounds = value;
        }
        if let Some(value) = overrides.max_turns {
            budget.max_turns = value;
        }
        if overrides.max_tokens.is_some() {
            budget.max_tokens = overrides.max_tokens;
        }
        if let Some(value) = overrides.stall_rounds {
            budget.stall_rounds = value;
        }
        if overrides.turn_timeout_secs.is_some() {
            budget.turn_timeout_secs = overrides.turn_timeout_secs;
        }
        for tool in &overrides.grant_tools {
            for member in &mut team.members {
                if !member.tools.contains(tool) {
                    member.tools.push(tool.clone());
                }
            }
        }
        team.validate()?;
        Ok(team)
    }

    pub fn save_team(&self, file: &TeamFile, overwrite: bool) -> Result<PathBuf, TeamError> {
        self.resolve_team(file.clone())?;
        let path = self.team_path(&file.name);
        write_toml(&path, file, overwrite)?;
        Ok(path)
    }
}

fn toml_stems(directory: &Path) -> Result<Vec<String>, TeamError> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut stems = Vec::new();
    for entry in fs::read_dir(directory).map_err(TeamError::Io)? {
        let path = entry.map_err(TeamError::Io)?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                stems.push(stem.to_owned());
            }
        }
    }
    stems.sort();
    Ok(stems)
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, TeamError> {
    let raw = fs::read_to_string(path).map_err(TeamError::Io)?;
    toml::from_str(&raw).map_err(|error| TeamError::Config(format!("{}: {error}", path.display())))
}

fn write_toml<T: Serialize>(path: &Path, value: &T, overwrite: bool) -> Result<(), TeamError> {
    if path.exists() && !overwrite {
        return Err(TeamError::AlreadyExists(path.display().to_string()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(TeamError::Io)?;
    }
    let raw =
        toml::to_string_pretty(value).map_err(|error| TeamError::Config(error.to_string()))?;
    fs::write(path, raw).map_err(TeamError::Io)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{MemberEntry, TeamFile, TeamLibrary, TeamOverrides};
    use crate::{BotProfile, Budget, FloorPolicyKind};

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "nexus-teams-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).expect("temp dir");
        root
    }

    #[test]
    fn bots_and_teams_round_trip() {
        let root = temp_root("roundtrip");
        let library = TeamLibrary::new(&root);
        let mut bot =
            BotProfile::new("ada", "Rust engineer").with_instructions("Prefer small diffs.");
        bot.archetype = Some("engineer".to_owned());
        library.save_bot(&bot, false).expect("save bot");
        assert!(
            library.save_bot(&bot, false).is_err(),
            "no silent overwrite"
        );

        let file = TeamFile {
            name: "core".to_owned(),
            description: "Core runtime".to_owned(),
            lead: Some("pm".to_owned()),
            members: vec![
                MemberEntry::Reference("pm:lead".to_owned()),
                MemberEntry::Reference("ada".to_owned()),
                MemberEntry::Reference("reviewer".to_owned()),
            ],
            channels: Vec::new(),
            approvers: vec!["reviewer".to_owned()],
            approval: crate::ApprovalRule::All,
            policy: FloorPolicyKind::MentionDriven,
            budget: Budget::default(),
            parallel_turns: false,
        };
        library.save_team(&file, false).expect("save team");
        assert_eq!(library.list_teams().expect("list"), vec!["core"]);
        let team = library.load_team("core").expect("load team");
        assert_eq!(team.members.len(), 3);
        let ada = team.member("ada").expect("ada");
        assert_eq!(ada.role, "Rust engineer");
        assert!(ada.instructions.ends_with("Prefer small diffs."));
        assert_eq!(library.list_bots().expect("bots").len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn overrides_assemble_ad_hoc_and_saved_teams() {
        let root = temp_root("assemble");
        let library = TeamLibrary::new(&root);
        assert!(library.assemble(None, &TeamOverrides::default()).is_err());
        let overrides = TeamOverrides {
            members: vec!["pm:lead".to_owned(), "ada:engineer".to_owned()],
            approvers: vec!["pm".to_owned()],
            policy: Some(FloorPolicyKind::Broadcast),
            max_rounds: Some(3),
            max_tokens: Some(500),
            grant_tools: vec!["filesystem.read".to_owned()],
            ..TeamOverrides::default()
        };
        let team = library.assemble(None, &overrides).expect("ad-hoc");
        assert_eq!(team.name, "ad-hoc");
        assert_eq!(team.policy, FloorPolicyKind::Broadcast);
        assert_eq!(team.budget.max_rounds, 3);
        assert_eq!(team.budget.max_tokens, Some(500));
        assert!(
            team.members
                .iter()
                .all(|member| member.tools == vec!["filesystem.read".to_owned()])
        );
        let bad_lead = TeamOverrides {
            lead: Some("ghost".to_owned()),
            ..overrides
        };
        assert!(library.assemble(None, &bad_lead).is_err());
        assert!(
            library
                .assemble(Some("missing"), &TeamOverrides::default())
                .is_err()
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn inline_members_and_missing_teams() {
        let root = temp_root("inline");
        let library = TeamLibrary::new(&root);
        std::fs::create_dir_all(library.teams_dir()).expect("dir");
        std::fs::write(
            library.team_path("solo"),
            "name = \"solo\"\n[[members]]\nhandle = \"scribe\"\narchetype = \"writer\"\n",
        )
        .expect("write");
        let team = library.load_team("solo").expect("load");
        assert_eq!(team.members[0].role, "Technical writer");
        assert!(library.load_team("ghost").is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
