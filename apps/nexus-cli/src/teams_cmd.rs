//! `nexus bots` and `nexus team`: manage bots and teams, and inspect missions.

use std::{
    fmt::Write as _,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow, bail};
use clap::{Args, Subcommand, ValueEnum};
use nexus_teams::{
    ApprovalRule, Archetype, BotProfile, Channel, FloorPolicyKind, MemberEntry, TeamError,
    TeamFile, TeamLibrary, TeamSpec, TeamTemplate, WorkspaceMode, list_missions, load_mission,
    persistence::load_events,
    resolve_mission_id,
    transcript::{render_markdown, render_report},
};

use crate::{
    chat::ChatPrinter,
    mission_cmd::{self, ResumeArgs, RunArgs, short_id},
    support::{WORKSPACE_TOOLS, session_store},
};

#[derive(Debug, Subcommand)]
pub enum BotsCommand {
    /// List built-in bot archetypes.
    Archetypes,
    /// List project bots in `.nexus/bots`.
    List,
    /// Show a project bot or archetype-based profile.
    Show { handle: String },
    /// Create a project bot in `.nexus/bots/<handle>.toml`.
    New(NewBotArgs),
}

#[derive(Debug, Args)]
pub struct NewBotArgs {
    /// Mention handle (`@handle`).
    handle: String,
    /// Built-in archetype to extend.
    #[arg(long, short)]
    archetype: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    role: Option<String>,
    /// Extra persona instructions (appended to the archetype's).
    #[arg(long)]
    instructions: Option<String>,
    /// Comma-separated expertise tags.
    #[arg(long, value_delimiter = ',')]
    expertise: Vec<String>,
    /// Workspace tools the bot may call (filesystem.read, shell.execute).
    #[arg(long = "tool")]
    tools: Vec<String>,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    /// Give the bot its own Git worktree for tool execution.
    #[arg(long)]
    worktree: bool,
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Subcommand)]
pub enum TeamCommand {
    /// List built-in team templates.
    Templates,
    /// List project teams in `.nexus/teams`.
    List,
    /// Show a saved team or built-in template: roster, channels, policy, approval, budget.
    Show { name: String },
    /// Create a team definition in `.nexus/teams/<name>.toml`.
    New(NewTeamArgs),
    /// Run a team toward a goal.
    Run(Box<RunArgs>),
    /// Continue a persisted mission with a fresh budget and optional feedback.
    Resume(Box<ResumeArgs>),
    /// List persisted missions, newest first.
    Missions,
    /// Print a mission transcript (accepts a unique id prefix).
    Transcript {
        mission: String,
        #[arg(long, value_enum, default_value_t = TranscriptFormat::Chat)]
        format: TranscriptFormat,
        /// Write to a file instead of stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Print a mission's task board and working notes.
    Board { mission: String },
    /// Print a mission report.
    Report { mission: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum TranscriptFormat {
    Chat,
    Markdown,
    Json,
}

#[derive(Debug, Args)]
pub struct NewTeamArgs {
    name: String,
    /// Start from a built-in template (see `nexus team templates`).
    #[arg(long)]
    template: Option<String>,
    /// Members: bot file handles, archetypes, or `handle:archetype` (added to a template's).
    #[arg(long = "member")]
    members: Vec<String>,
    #[arg(long)]
    lead: Option<String>,
    /// Completion approvers (bot handles or `human`); replaces a template's.
    #[arg(long = "approver")]
    approvers: Vec<String>,
    /// How approver votes are counted: all, majority, any.
    #[arg(long)]
    approval: Option<ApprovalRule>,
    #[arg(long)]
    policy: Option<FloorPolicyKind>,
    #[arg(long)]
    description: Option<String>,
    /// Extra channels as `name` or `name:purpose`.
    #[arg(long = "channel")]
    channels: Vec<String>,
    #[arg(long)]
    parallel: bool,
    #[arg(long)]
    force: bool,
}

pub fn run_bots(root: &Path, command: BotsCommand) -> Result<()> {
    let library = TeamLibrary::new(root);
    match command {
        BotsCommand::Archetypes => {
            for archetype in Archetype::all() {
                println!(
                    "{:<11} {:<20} {}",
                    archetype.key, archetype.role, archetype.summary
                );
            }
        }
        BotsCommand::List => {
            let bots = library.list_bots()?;
            if bots.is_empty() {
                println!(
                    "No project bots yet. Create one with `nexus bots new <handle> --archetype engineer`."
                );
            }
            for bot in bots {
                println!(
                    "{:<14} {:<22} {}",
                    format!("@{}", bot.handle),
                    bot.role,
                    bot.expertise.join(", ")
                );
            }
        }
        BotsCommand::Show { handle } => {
            let bot = library.resolve_member(&handle)?;
            print!("{}", describe_bot(&bot));
        }
        BotsCommand::New(args) => {
            let force = args.force;
            let path = library.save_bot(&new_bot(args)?, force);
            println!("created {}", explain_exists(path)?.display());
        }
    }
    Ok(())
}

/// Turns "already exists" into a hint about `--force`.
fn explain_exists(result: Result<PathBuf, TeamError>) -> Result<PathBuf> {
    result.or_else(|error| match error {
        TeamError::AlreadyExists(path) => bail!("{path} already exists; pass --force to overwrite"),
        other => Err(other.into()),
    })
}

fn new_bot(args: NewBotArgs) -> Result<BotProfile> {
    let mut bot = match &args.archetype {
        Some(archetype) => BotProfile::from_archetype(&args.handle, archetype)?,
        None => BotProfile::new(&args.handle, args.role.clone().unwrap_or_default()),
    };
    // Persist only the overrides so archetype improvements flow through on load.
    if bot.archetype.is_some() {
        bot.instructions.clear();
        bot.expertise.clear();
        bot.role.clear();
        bot.color = None;
    }
    if let Some(name) = args.name {
        bot.name = name;
    }
    if let Some(role) = args.role {
        bot.role = role;
    }
    if let Some(instructions) = args.instructions {
        bot.instructions = instructions;
    }
    if !args.expertise.is_empty() {
        bot.expertise = args.expertise;
    }
    for tool in &args.tools {
        if !WORKSPACE_TOOLS.contains(&tool.as_str()) {
            bail!(
                "unknown tool `{tool}`; available: {}",
                WORKSPACE_TOOLS.join(", ")
            );
        }
    }
    bot.tools = args.tools;
    bot.provider = args.provider;
    bot.model = args.model;
    if args.worktree {
        bot.workspace = WorkspaceMode::Worktree;
    }
    bot.validate()?;
    Ok(bot)
}

fn describe_bot(bot: &BotProfile) -> String {
    let mut out = format!("{}\n", bot.label());
    if let Some(archetype) = &bot.archetype {
        let _ = writeln!(out, "archetype: {archetype}");
    }
    if !bot.expertise.is_empty() {
        let _ = writeln!(out, "expertise: {}", bot.expertise.join(", "));
    }
    if !bot.tools.is_empty() {
        let _ = writeln!(
            out,
            "tools: {} ({:?} workspace)",
            bot.tools.join(", "),
            bot.workspace
        );
    }
    if let Some(model) = &bot.model {
        let _ = writeln!(
            out,
            "model: {} via {}",
            model,
            bot.provider.as_deref().unwrap_or("default provider")
        );
    }
    for (action, decision) in &bot.permissions {
        let _ = writeln!(out, "permission: {action} = {decision:?}");
    }
    let _ = writeln!(out, "\n{}", bot.instructions);
    out
}

pub async fn run_team(root: &Path, command: TeamCommand) -> Result<()> {
    let library = TeamLibrary::new(root);
    match command {
        TeamCommand::Templates => list_templates(&library),
        TeamCommand::List => list_teams(&library)?,
        TeamCommand::Show { name } => print!("{}", describe_team(&find_team(&library, &name)?)),
        TeamCommand::New(args) => {
            let force = args.force;
            let path = library.save_team(&new_team_file(args)?, force);
            println!("created {}", explain_exists(path)?.display());
        }
        TeamCommand::Run(args) => mission_cmd::run(root, &library, *args).await?,
        TeamCommand::Resume(args) => mission_cmd::resume(root, *args).await?,
        TeamCommand::Missions => list_persisted_missions(root)?,
        TeamCommand::Transcript {
            mission,
            format,
            output,
        } => {
            let rendered = transcript(root, &mission, format, output.is_none())?;
            match output {
                Some(path) => {
                    std::fs::write(&path, rendered)?;
                    println!("wrote {}", path.display());
                }
                None => println!("{rendered}"),
            }
        }
        TeamCommand::Board { mission } => {
            let store = session_store(root)?;
            let state = load_mission(&store, &resolve_mission_id(&store, &mission)?)?;
            println!("{}", state.board.render());
            for (bot, notes) in &state.notes {
                println!("\nNotes from @{bot}:");
                for note in notes {
                    println!("  - {note}");
                }
            }
        }
        TeamCommand::Report { mission } => {
            let store = session_store(root)?;
            let state = load_mission(&store, &resolve_mission_id(&store, &mission)?)?;
            let report = state.report.clone().unwrap_or_else(|| state.build_report());
            print!("{}", render_report(&report));
        }
    }
    Ok(())
}

fn list_templates(library: &TeamLibrary) {
    for template in TeamTemplate::all() {
        println!("{:<10} {}", template.key, template.summary);
        match library.resolve_team(template.team_file(template.key)) {
            Ok(team) => println!(
                "{:<10} {} bots · {} · approval: {}",
                "",
                team.members.len(),
                team.policy,
                team.approval_summary()
            ),
            Err(error) => println!("{:<10} (unavailable: {error})", ""),
        }
    }
    println!(
        "\nRun one:  nexus team run --template software --goal \"...\"\nSave one: nexus team new my-team --template software"
    );
}

fn list_teams(library: &TeamLibrary) -> Result<()> {
    let teams = library.list_teams()?;
    if teams.is_empty() {
        println!(
            "No teams yet. Create one with `nexus team new <name> --template software` (see `nexus team templates`)."
        );
    }
    for name in teams {
        match library.load_team(&name) {
            Ok(team) => println!(
                "{name:<16} {} bots, lead @{}, {} — {}",
                team.members.len(),
                team.lead_handle(),
                team.policy,
                team.description
            ),
            Err(error) => println!("{name:<16} (invalid: {error})"),
        }
    }
    Ok(())
}

/// A saved team, falling back to a built-in template of the same name.
fn find_team(library: &TeamLibrary, name: &str) -> Result<TeamSpec> {
    match library.load_team(name) {
        Err(TeamError::NotFound(_)) if TeamTemplate::find(name).is_some() => {
            let template = TeamTemplate::find(name).expect("checked above");
            Ok(library.resolve_team(template.team_file(name))?)
        }
        result => Ok(result?),
    }
}

fn list_persisted_missions(root: &Path) -> Result<()> {
    let store = session_store(root)?;
    let missions = list_missions(&store)?;
    if missions.is_empty() {
        println!(
            "No missions yet. Start one with `nexus team run --template software --goal \"...\"`."
        );
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    for mission in missions {
        println!(
            "{}  {:<14} {:>8}  {:<12} {}",
            short_id(&mission.id),
            mission.status.label(),
            ago(now - mission.last_at_ms),
            mission.team,
            mission.goal
        );
    }
    Ok(())
}

/// Compact relative age: `42s ago`, `5m ago`, `3h ago`, `2d ago`.
fn ago(elapsed_ms: i64) -> String {
    let seconds = elapsed_ms.max(0) / 1000;
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn transcript(
    root: &Path,
    mission: &str,
    format: TranscriptFormat,
    to_stdout: bool,
) -> Result<String> {
    let store = session_store(root)?;
    let id = resolve_mission_id(&store, mission)?;
    Ok(match format {
        TranscriptFormat::Markdown => render_markdown(&load_mission(&store, &id)?),
        TranscriptFormat::Json => load_events(&store, &id)?
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?
            .join("\n"),
        TranscriptFormat::Chat => {
            let mut printer = ChatPrinter::new(to_stdout && io::stdout().is_terminal(), false);
            load_events(&store, &id)?
                .iter()
                .filter_map(|event| printer.format(event))
                .collect::<Vec<_>>()
                .join("\n")
        }
    })
}

fn new_team_file(args: NewTeamArgs) -> Result<TeamFile> {
    let mut file = match args.template.as_deref() {
        Some(key) => TeamTemplate::find(key)
            .ok_or_else(|| anyhow!("unknown team template `{key}`; see `nexus team templates`"))?
            .team_file(&args.name),
        None if args.members.is_empty() => {
            bail!(
                "add members with --member, or start from --template (see `nexus team templates`)"
            )
        }
        None => TeamFile {
            name: args.name.clone(),
            ..TeamFile::default()
        },
    };
    file.members
        .extend(args.members.into_iter().map(MemberEntry::Reference));
    if let Some(description) = args.description {
        file.description = description;
    }
    if args.lead.is_some() {
        file.lead = args.lead;
    }
    if !args.approvers.is_empty() {
        file.approvers = args.approvers;
    }
    if let Some(approval) = args.approval {
        file.approval = approval;
    }
    if let Some(policy) = args.policy {
        file.policy = policy;
    }
    file.parallel_turns |= args.parallel;
    file.channels.extend(args.channels.iter().map(|spec| {
        let (name, purpose) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
        Channel::open(name.trim(), purpose.trim())
    }));
    Ok(file)
}

fn describe_team(team: &TeamSpec) -> String {
    let mut out = format!("Team {}", team.name);
    if !team.description.is_empty() {
        let _ = write!(out, " — {}", team.description);
    }
    let _ = writeln!(
        out,
        "\nlead: @{}   policy: {}   parallel turns: {}",
        team.lead_handle(),
        team.policy,
        team.parallel_turns
    );
    let _ = writeln!(out, "approval: {}", team.approval_summary());
    let budget = &team.budget;
    let _ = writeln!(
        out,
        "budget: {} rounds, {} turns, {} messages, tokens {}, stall after {} idle rounds",
        budget.max_rounds,
        budget.max_turns,
        budget.max_messages,
        budget
            .max_tokens
            .map_or_else(|| "unlimited".to_owned(), |t| t.to_string()),
        budget.stall_rounds
    );
    let _ = writeln!(out, "\nmembers:");
    for member in &team.members {
        let _ = writeln!(
            out,
            "  {}  [{}]",
            member.label(),
            member.expertise.join(", ")
        );
    }
    let _ = writeln!(out, "\nchannels:");
    for channel in team.all_channels() {
        let audience = if channel.members.is_empty() {
            "everyone".to_owned()
        } else {
            channel.members.join(", ")
        };
        let _ = writeln!(
            out,
            "  #{} — {} ({audience})",
            channel.name, channel.purpose
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use nexus_teams::{ApprovalRule, MemberEntry, TeamLibrary};

    use super::{NewTeamArgs, ago, describe_team, find_team, new_team_file};

    #[derive(Parser)]
    struct NewTeamHarness {
        #[command(flatten)]
        new: NewTeamArgs,
    }

    fn library() -> TeamLibrary {
        TeamLibrary::new(std::env::temp_dir().join("nexus-cli-no-such-root"))
    }

    #[test]
    fn team_files_start_from_templates_and_apply_overrides() {
        let harness = NewTeamHarness::parse_from([
            "test",
            "squad",
            "--template",
            "software",
            "--member",
            "docs:writer",
            "--approval",
            "majority",
            "--channel",
            "ops:Deploys and alerts",
        ]);
        let file = new_team_file(harness.new).expect("file");
        assert_eq!(file.name, "squad");
        assert_eq!(file.approval, ApprovalRule::Majority);
        assert!(
            file.members
                .contains(&MemberEntry::Reference("docs:writer".to_owned()))
        );
        assert!(file.channels.iter().any(|channel| channel.name == "ops"));
        let team = library().resolve_team(file).expect("valid team");
        assert_eq!(team.members.len(), 6);
    }

    #[test]
    fn team_files_need_members_or_a_known_template() {
        let empty = NewTeamHarness::parse_from(["test", "squad"]);
        assert!(new_team_file(empty.new).is_err());
        let unknown = NewTeamHarness::parse_from(["test", "squad", "--template", "nope"]);
        assert!(new_team_file(unknown.new).is_err());
    }

    #[test]
    fn templates_are_shown_when_no_saved_team_matches() {
        let team = find_team(&library(), "research").expect("template");
        let described = describe_team(&team);
        assert!(described.contains("approval: @skeptic, @scribe, @quant (majority vote)"));
        assert!(find_team(&library(), "ghost").is_err());
    }

    #[test]
    fn ages_are_compact() {
        assert_eq!(ago(-5), "0s ago");
        assert_eq!(ago(42_000), "42s ago");
        assert_eq!(ago(5 * 60_000), "5m ago");
        assert_eq!(ago(3 * 3_600_000), "3h ago");
        assert_eq!(ago(2 * 86_400_000), "2d ago");
    }
}
