//! `nexus team run` and `nexus team resume`: drive missions from the terminal.

use std::{
    collections::BTreeMap,
    io::{self, BufRead, IsTerminal},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use nexus_config::Config;
use nexus_teams::{
    ApprovalRule, BotProfile, BrainRouter, ChannelEventSink, FloorPolicyKind, Goal, MissionControl,
    MissionEngine, MissionId, MissionReport, OPERATOR_HELP, OperatorCommand, ResumeOptions,
    RuntimeBrain, SimulatedBrain, SqliteTeamSink, TeamError, TeamLibrary, TeamOverrides, TeamSpec,
    WorkspaceMode, load_mission, parse_operator_line, resolve_mission_id,
    transcript::render_report,
};
use nexus_workspace::GitWorktreeManager;
use tokio::sync::mpsc;

use crate::{
    chat::ChatPrinter,
    support::{
        Approval, ProviderArgs, WORKSPACE_TOOLS, build_provider, build_tool_executor, session_store,
    },
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum BrainChoice {
    /// Simulated for the mock provider, model-backed otherwise.
    #[default]
    Auto,
    /// Deterministic offline collaboration.
    Simulated,
    /// Model-backed bots through the Nexus runtime.
    Runtime,
}

/// How a mission is executed and presented; shared by `run` and `resume`.
#[derive(Debug, Args)]
pub struct ExecutionArgs {
    #[command(flatten)]
    provider: ProviderArgs,
    #[arg(long, value_enum, default_value_t = BrainChoice::Auto)]
    brain: BrainChoice,
    /// Auto-approve `ask` tool permissions.
    #[arg(long)]
    yes: bool,
    /// Attach an operator console: post, DM, answer, approve, pause, cancel.
    #[arg(long, short)]
    interactive: bool,
    /// Show bot activity (turn reasons, tool calls).
    #[arg(long, short)]
    verbose: bool,
    /// Only print the final report.
    #[arg(long, short)]
    quiet: bool,
    /// Print the final report as JSON.
    #[arg(long)]
    json: bool,
    /// Do not persist the mission to `.nexus/nexus.db`.
    #[arg(long)]
    no_persist: bool,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Saved team name (omit to use `--template` or assemble from `--member`).
    team: Option<String>,
    /// Built-in team template (see `nexus team templates`).
    #[arg(long)]
    template: Option<String>,
    /// The goal the team works toward.
    #[arg(long, short)]
    goal: String,
    /// Success criteria (repeatable).
    #[arg(long = "criterion")]
    criteria: Vec<String>,
    /// Members to add: handles, archetypes, or `handle:archetype`.
    #[arg(long = "member")]
    members: Vec<String>,
    #[arg(long)]
    lead: Option<String>,
    #[arg(long = "approver")]
    approvers: Vec<String>,
    /// How approver votes are counted: all, majority, any.
    #[arg(long)]
    approval: Option<ApprovalRule>,
    #[arg(long)]
    policy: Option<FloorPolicyKind>,
    #[arg(long)]
    max_rounds: Option<u32>,
    #[arg(long)]
    max_turns: Option<u32>,
    #[arg(long)]
    max_tokens: Option<u64>,
    #[arg(long)]
    stall_rounds: Option<u32>,
    /// Per-turn timeout in seconds.
    #[arg(long)]
    turn_timeout: Option<u64>,
    /// Run selected bots concurrently each round.
    #[arg(long)]
    parallel: bool,
    /// Grant a workspace tool to every bot (repeatable).
    #[arg(long = "grant-tool")]
    grant_tools: Vec<String>,
    #[command(flatten)]
    execution: ExecutionArgs,
}

#[derive(Debug, Args)]
pub struct ResumeArgs {
    /// Mission id or unique prefix.
    mission: String,
    /// Additional rounds (defaults to the team's round budget).
    #[arg(long)]
    rounds: Option<u32>,
    /// Additional turns (defaults to the team's turn budget).
    #[arg(long)]
    turns: Option<u32>,
    /// Additional tokens.
    #[arg(long)]
    tokens: Option<u64>,
    /// Feedback for the team, posted to #general as @human.
    #[arg(long)]
    note: Option<String>,
    #[command(flatten)]
    execution: ExecutionArgs,
}

pub fn assemble_team(library: &TeamLibrary, args: &RunArgs) -> Result<TeamSpec> {
    for tool in &args.grant_tools {
        if !WORKSPACE_TOOLS.contains(&tool.as_str()) {
            bail!(
                "unknown tool `{tool}`; available: {}",
                WORKSPACE_TOOLS.join(", ")
            );
        }
    }
    if args.team.is_some() && args.template.is_some() {
        bail!("pass either a saved team or --template, not both");
    }
    let overrides = TeamOverrides {
        members: args.members.clone(),
        lead: args.lead.clone(),
        approvers: args.approvers.clone(),
        approval: args.approval,
        policy: args.policy,
        template: args.template.clone(),
        max_rounds: args.max_rounds,
        max_turns: args.max_turns,
        max_tokens: args.max_tokens,
        stall_rounds: args.stall_rounds,
        turn_timeout_secs: args.turn_timeout,
        parallel_turns: args.parallel,
        grant_tools: args.grant_tools.clone(),
    };
    Ok(library.assemble(args.team.as_deref(), &overrides)?)
}

pub async fn run(root: &Path, library: &TeamLibrary, args: RunArgs) -> Result<()> {
    let team = assemble_team(library, &args)?;
    let goal = Goal::new(args.goal.clone()).with_criteria(args.criteria.clone());
    let mission_id = MissionId::generate();
    let (brains, workspaces) = build_brains(root, &args.execution, &mission_id.0, &team)?;
    let engine = MissionEngine::new(team, goal, brains)?.with_mission_id(mission_id);
    drive(root, engine, &args.execution, workspaces).await
}

pub async fn resume(root: &Path, args: ResumeArgs) -> Result<()> {
    let store = session_store(root)?;
    let id = resolve_mission_id(&store, &args.mission)?;
    let state = load_mission(&store, &id)?;
    let (brains, workspaces) = build_brains(root, &args.execution, &id, &state.team)?;
    let options = ResumeOptions {
        rounds: args.rounds,
        turns: args.turns,
        tokens: args.tokens,
        note: args.note.clone(),
    };
    let engine = MissionEngine::resume(state, brains, options)?;
    drive(root, engine, &args.execution, workspaces).await
}

/// Runs a mission with live output, persistence, Ctrl+C, and an optional operator console.
async fn drive(
    root: &Path,
    engine: MissionEngine,
    execution: &ExecutionArgs,
    workspaces: Option<Arc<BotWorkspaces>>,
) -> Result<()> {
    let mission_id = engine.mission_id().clone();
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let mut engine = engine
        .with_sink(Arc::new(ChannelEventSink::new(events_tx)))
        .interactive(execution.interactive);
    let persistence = if execution.no_persist {
        None
    } else {
        let sink = Arc::new(SqliteTeamSink::new(session_store(root)?));
        engine = engine.with_sink(sink.clone());
        Some(sink)
    };
    let control = engine.control();

    let ctrl_c = control.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\n[nexus] cancelling mission…");
            ctrl_c.cancel();
        }
    });
    if execution.interactive {
        spawn_operator_console(control.clone());
        eprintln!("[nexus] operator console attached — type /help for commands");
    }

    let quiet = execution.quiet || execution.json;
    let verbose = execution.verbose;
    let printer = tokio::spawn(async move {
        let mut printer = ChatPrinter::new(io::stdout().is_terminal(), verbose);
        while let Some(event) = events_rx.recv().await {
            if quiet {
                continue;
            }
            if let Some(line) = printer.format(&event) {
                println!("{line}");
            }
        }
    });

    let report = engine.run().await?;
    let _ = printer.await;
    print_report(&report, execution.json)?;
    if execution.json {
        return Ok(());
    }
    if let Some(workspaces) = workspaces {
        let allocated = workspaces.allocated();
        if !allocated.is_empty() {
            println!("\nBot worktrees (nothing is merged automatically):");
            for (handle, name) in allocated {
                println!("  @{handle:<12} nexus worktree review {name}");
            }
        }
    }
    if let Some(sink) = persistence {
        for error in sink.errors() {
            eprintln!("[nexus] persistence error: {error}");
        }
        let short = short_id(&mission_id.0);
        println!("\nReplay: nexus team transcript {short}");
        if report.status == nexus_teams::MissionStatus::Completed {
            println!("Follow up: nexus team resume {short} --note \"<more work>\"");
        } else {
            println!("Continue: nexus team resume {short} --rounds 6 --note \"...\"");
        }
    }
    Ok(())
}

pub fn short_id(id: &str) -> &str {
    let rest = id.strip_prefix("mission-").unwrap_or(id);
    &rest[..rest.len().min(8)]
}

fn print_report(report: &MissionReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!("\n{}", render_report(report));
    }
    Ok(())
}

/// Lazily allocates one Git worktree per bot that asks for isolation.
pub struct BotWorkspaces {
    root: PathBuf,
    mission: String,
    manager: Option<GitWorktreeManager>,
    allocated: Mutex<BTreeMap<String, (String, PathBuf)>>,
}

impl BotWorkspaces {
    fn path_for(&self, bot: &BotProfile) -> Result<PathBuf, TeamError> {
        if bot.workspace == WorkspaceMode::Shared {
            return Ok(self.root.clone());
        }
        let manager = self.manager.as_ref().ok_or_else(|| {
            TeamError::Config("worktree workspaces need a Git repository".to_owned())
        })?;
        let mut allocated = self
            .allocated
            .lock()
            .map_err(|_| TeamError::Brain("workspace lock poisoned".to_owned()))?;
        if let Some((_, path)) = allocated.get(&bot.handle) {
            return Ok(path.clone());
        }
        let name = format!("{}-{}", self.mission, bot.handle);
        let worktree = match manager.create(&name, None) {
            Ok(worktree) => worktree,
            // A resumed mission reuses the worktree its bot already owns.
            Err(nexus_workspace::WorkspaceError::AlreadyExists(_)) => manager
                .list()
                .map_err(|error| TeamError::Config(error.to_string()))?
                .into_iter()
                .find(|worktree| worktree.name == name)
                .ok_or_else(|| TeamError::NotFound(format!("worktree `{name}`")))?,
            Err(error) => return Err(TeamError::Config(error.to_string())),
        };
        allocated.insert(bot.handle.clone(), (name, worktree.path.clone()));
        Ok(worktree.path)
    }

    /// `(handle, worktree name)` pairs allocated so far.
    fn allocated(&self) -> Vec<(String, String)> {
        self.allocated
            .lock()
            .map(|allocated| {
                allocated
                    .iter()
                    .map(|(handle, (name, _))| (handle.clone(), name.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn build_brains(
    root: &Path,
    execution: &ExecutionArgs,
    mission: &str,
    team: &TeamSpec,
) -> Result<(BrainRouter, Option<Arc<BotWorkspaces>>)> {
    let simulated = match execution.brain {
        BrainChoice::Simulated => true,
        BrainChoice::Runtime => false,
        BrainChoice::Auto => {
            execution.provider.provider == "mock"
                && team.members.iter().all(|member| member.provider.is_none())
        }
    };
    if simulated {
        return Ok((BrainRouter::new(Arc::new(SimulatedBrain)), None));
    }
    let config = Config::load_from(root)?.config;
    let defaults = execution.provider.clone();
    let providers: nexus_teams::ProviderFactory = Arc::new(move |bot: &BotProfile| {
        let provider_name = bot.provider.as_deref().unwrap_or(&defaults.provider);
        let provider = build_provider(provider_name, &defaults.base_url, &defaults.api_key_env)
            .map_err(|error| TeamError::Brain(error.to_string()))?;
        Ok((
            provider,
            bot.model.clone().unwrap_or_else(|| defaults.model.clone()),
        ))
    });
    let approval = if execution.yes {
        Approval::Always
    } else if execution.interactive {
        // Stdin belongs to the operator console during interactive missions.
        Approval::Never
    } else {
        Approval::Prompt
    };
    let workspaces = Arc::new(BotWorkspaces {
        root: root.to_path_buf(),
        mission: mission.chars().take(20).collect(),
        manager: GitWorktreeManager::new(root).ok(),
        allocated: Mutex::new(BTreeMap::new()),
    });
    let factory_workspaces = Arc::clone(&workspaces);
    let executors: nexus_teams::ExecutorFactory = Arc::new(move |bot: &BotProfile| {
        let path = factory_workspaces.path_for(bot)?;
        let tools = bot.tools.iter().map(String::as_str).collect::<Vec<_>>();
        build_tool_executor(&path, &tools, approval)
            .map_err(|error| TeamError::Brain(error.to_string()))
    });
    Ok((
        BrainRouter::new(Arc::new(
            RuntimeBrain::new(config, providers).with_executors(executors),
        )),
        Some(workspaces),
    ))
}

fn spawn_operator_console(control: MissionControl) {
    // A detached thread: blocking stdin reads must not hold up runtime shutdown.
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match parse_operator_line(&line, nexus_teams::conversation::GENERAL) {
                Some(OperatorCommand::Control(input)) => {
                    if control.send(input).is_err() {
                        break;
                    }
                }
                Some(OperatorCommand::Cancel) => {
                    control.cancel();
                    break;
                }
                Some(OperatorCommand::Help) => eprintln!("{OPERATOR_HELP}"),
                Some(OperatorCommand::Invalid(message)) => eprintln!("[nexus] {message}"),
                Some(OperatorCommand::Goal(_)) => {
                    eprintln!(
                        "[nexus] a mission is already running; start another with `nexus team run`"
                    );
                }
                None => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::Path, process::Command, sync::Mutex};

    use clap::Parser;
    use nexus_teams::{BotProfile, TeamLibrary, WorkspaceMode};
    use nexus_workspace::GitWorktreeManager;

    use super::{BotWorkspaces, ResumeArgs, RunArgs, assemble_team, short_id};

    #[test]
    fn bot_worktrees_are_allocated_once_and_reused_on_resume() {
        let root = std::env::temp_dir().join(format!("nexus-cli-bots-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp repo");
        let git = |args: &[&str]| {
            Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .is_ok_and(|output| output.status.success())
        };
        let ready = git(&["init", "--quiet"])
            && git(&[
                "-c",
                "user.name=Nexus",
                "-c",
                "user.email=nexus@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "init",
            ]);
        if !ready {
            eprintln!("skipping: git is unavailable");
            std::fs::remove_dir_all(&root).ok();
            return;
        }
        let workspaces = |root: &Path| BotWorkspaces {
            root: root.to_path_buf(),
            mission: "mission-0123456789ab".to_owned(),
            manager: GitWorktreeManager::new(root).ok(),
            allocated: Mutex::new(BTreeMap::new()),
        };
        let shared = BotProfile::from_archetype("lead", "lead").expect("lead");
        let mut isolated = BotProfile::from_archetype("ada", "engineer").expect("ada");
        isolated.workspace = WorkspaceMode::Worktree;

        let first = workspaces(&root);
        assert_eq!(first.path_for(&shared).expect("shared"), root);
        let path = first.path_for(&isolated).expect("worktree");
        assert!(path.is_dir() && path != root);
        assert_eq!(first.path_for(&isolated).expect("cached"), path);
        assert_eq!(
            first.allocated(),
            vec![("ada".to_owned(), "mission-0123456789ab-ada".to_owned())]
        );

        // A resumed mission allocates afresh and must find the bot's existing worktree.
        let resumed = workspaces(&root);
        assert_eq!(resumed.path_for(&isolated).expect("reused"), path);

        GitWorktreeManager::new(&root)
            .expect("manager")
            .remove("mission-0123456789ab-ada", true)
            .expect("remove");
        std::fs::remove_dir_all(&root).ok();
    }

    #[derive(Parser)]
    struct RunHarness {
        #[command(flatten)]
        run: RunArgs,
    }

    #[derive(Parser)]
    struct ResumeHarness {
        #[command(flatten)]
        resume: ResumeArgs,
    }

    fn library() -> TeamLibrary {
        TeamLibrary::new(std::env::temp_dir().join("nexus-cli-no-such-root"))
    }

    #[test]
    fn ad_hoc_teams_are_assembled_from_flags() {
        let harness = RunHarness::parse_from([
            "test",
            "--goal",
            "Ship it",
            "--member",
            "pm:lead",
            "--member",
            "ada:engineer",
            "--approver",
            "pm",
            "--approval",
            "majority",
            "--policy",
            "mention-driven",
            "--max-rounds",
            "5",
            "--grant-tool",
            "filesystem.read",
        ]);
        let team = assemble_team(&library(), &harness.run).expect("team");
        assert_eq!(team.members.len(), 2);
        assert_eq!(team.budget.max_rounds, 5);
        assert_eq!(team.policy.key(), "mention-driven");
        assert_eq!(team.approval.key(), "majority");
        assert!(
            team.members
                .iter()
                .all(|m| m.tools == vec!["filesystem.read".to_owned()])
        );
    }

    #[test]
    fn templates_assemble_and_accept_extra_members() {
        let harness = RunHarness::parse_from([
            "test",
            "--goal",
            "x",
            "--template",
            "software",
            "--member",
            "docs:writer",
        ]);
        let team = assemble_team(&library(), &harness.run).expect("team");
        assert_eq!(team.name, "software");
        assert!(team.is_member("arch") && team.is_member("docs"));
    }

    #[test]
    fn invalid_team_sources_are_rejected() {
        let library = library();
        let no_members = RunHarness::parse_from(["test", "--goal", "x"]);
        assert!(assemble_team(&library, &no_members.run).is_err());
        let both =
            RunHarness::parse_from(["test", "saved", "--template", "software", "--goal", "x"]);
        assert!(assemble_team(&library, &both.run).is_err());
        let bad_tool = RunHarness::parse_from([
            "test",
            "--goal",
            "x",
            "--member",
            "pm:lead",
            "--grant-tool",
            "rm.rf",
        ]);
        assert!(assemble_team(&library, &bad_tool.run).is_err());
    }

    #[test]
    fn resume_flags_parse() {
        let harness = ResumeHarness::parse_from([
            "test",
            "c2f7f079",
            "--rounds",
            "4",
            "--note",
            "focus on tests",
            "-i",
        ]);
        assert_eq!(harness.resume.rounds, Some(4));
        assert_eq!(harness.resume.note.as_deref(), Some("focus on tests"));
        assert!(harness.resume.execution.interactive);
    }

    #[test]
    fn short_ids_strip_the_prefix() {
        assert_eq!(short_id("mission-0123456789abcdef"), "01234567");
        assert_eq!(short_id("abc"), "abc");
    }
}
