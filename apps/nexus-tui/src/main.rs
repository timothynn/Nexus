//! Nexus Teams operator console: watch bots collaborate, chat with them, and steer missions.

// Command-line flag structs are naturally bool-heavy.
#![allow(clippy::struct_excessive_bools)]

mod app;
mod ui;

use std::{
    env, fs,
    io::{self, Stdout},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use nexus_config::Config;
use nexus_models::provider_from_name;
use nexus_storage::SqliteStore;
use nexus_teams::{
    ApprovalRule, BotProfile, BrainRouter, ChannelEventSink, FloorPolicyKind, Goal, MissionControl,
    MissionEngine, PacedBrain, ResumeOptions, RuntimeBrain, SimulatedBrain, SqliteTeamSink,
    TeamError, TeamEvent, TeamLibrary, TeamOverrides, TeamSpec, load_mission, resolve_mission_id,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::{runtime::Runtime, sync::mpsc::UnboundedReceiver};

use crate::app::{Action, App};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
enum BrainChoice {
    /// Simulated for the mock provider, model-backed otherwise.
    #[default]
    Auto,
    Simulated,
    Runtime,
}

#[derive(Debug, Parser)]
#[command(name = "nexus-tui", version, about = "Nexus Teams operator console")]
struct Args {
    /// Saved team (`.nexus/teams/<name>.toml`); defaults to a demo squad.
    team: Option<String>,
    /// Built-in team template (software, research, content, incident, debate).
    #[arg(long, conflicts_with = "team")]
    template: Option<String>,
    /// Members: bot handles, archetypes, or `handle:archetype` (repeatable).
    #[arg(long = "member")]
    members: Vec<String>,
    #[arg(long)]
    lead: Option<String>,
    /// Completion approvers (bot handles or `human`).
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
    max_tokens: Option<u64>,
    #[arg(long)]
    parallel: bool,
    /// Start a mission immediately.
    #[arg(long, short)]
    goal: Option<String>,
    #[arg(long, default_value = "mock-1")]
    model: String,
    #[arg(long, default_value = "mock")]
    provider: String,
    #[arg(long, default_value = "https://api.openai.com/v1")]
    base_url: String,
    #[arg(long, default_value = "OPENAI_API_KEY")]
    api_key_env: String,
    #[arg(long, value_enum, default_value_t = BrainChoice::Auto)]
    brain: BrainChoice,
    /// Delay before each simulated turn, so missions can be followed live.
    #[arg(long, default_value_t = 700)]
    pace_ms: u64,
    /// Open a persisted mission read-only (accepts a unique id prefix).
    #[arg(long, conflicts_with = "resume")]
    replay: Option<String>,
    /// Continue a persisted mission live (accepts a unique id prefix).
    #[arg(long, conflicts_with = "goal")]
    resume: Option<String>,
    /// Extra rounds granted to a resumed mission.
    #[arg(long, requires = "resume")]
    rounds: Option<u32>,
    /// Feedback for the team when resuming, posted to #general as @human.
    #[arg(long, requires = "resume")]
    note: Option<String>,
    /// Do not persist missions to `.nexus/nexus.db`.
    #[arg(long)]
    no_persist: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let root = env::current_dir()?;
    let library = TeamLibrary::new(&root);
    let mut app = App::new(assemble_team(&library, &args)?);
    if let Some(prefix) = &args.replay {
        let store = open_store(&root)?;
        let id = resolve_mission_id(&store, prefix)?;
        app.load_replay(load_mission(&store, &id)?);
    }

    let runtime = Runtime::new().context("failed to start the async runtime")?;
    let mut events = None;
    if let Some(prefix) = &args.resume {
        events = Some(resume_mission(&runtime, &mut app, &args, &root, prefix)?);
    } else if let (Some(goal), None) = (args.goal.clone(), &args.replay) {
        events = Some(start_mission(&runtime, &mut app, &args, &root, goal)?);
    }

    install_panic_hook();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let result = event_loop(&mut terminal, &runtime, &mut app, &args, &root, events);
    restore_terminal(&mut terminal);
    runtime.shutdown_timeout(Duration::from_secs(2));
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    runtime: &Runtime,
    app: &mut App,
    args: &Args,
    root: &Path,
    mut events: Option<UnboundedReceiver<TeamEvent>>,
) -> Result<()> {
    while !app.should_quit {
        if let Some(receiver) = &mut events {
            while let Ok(event) = receiver.try_recv() {
                app.apply(&event);
            }
        }
        terminal.draw(|frame| ui::render(frame, app))?;
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if let Some(Action::StartMission(goal)) = app.handle_key(key) {
                match start_mission(runtime, app, args, root, goal) {
                    Ok(receiver) => events = Some(receiver),
                    Err(error) => app.notice = format!("Could not start mission: {error:#}"),
                }
            }
        }
    }
    Ok(())
}

fn assemble_team(library: &TeamLibrary, args: &Args) -> Result<TeamSpec> {
    let mut overrides = TeamOverrides {
        members: args.members.clone(),
        lead: args.lead.clone(),
        approvers: args.approvers.clone(),
        approval: args.approval,
        policy: args.policy,
        template: args.template.clone(),
        max_rounds: args.max_rounds,
        max_tokens: args.max_tokens,
        parallel_turns: args.parallel,
        ..TeamOverrides::default()
    };
    let demo = args.team.is_none() && args.template.is_none() && args.members.is_empty();
    if demo {
        // A demo squad that exercises planning, building, and review.
        overrides.members = [
            "lead:lead",
            "arch:architect",
            "ada:engineer",
            "rev:reviewer",
        ]
        .map(str::to_owned)
        .to_vec();
        if overrides.approvers.is_empty() {
            overrides.approvers = vec!["rev".to_owned()];
        }
    }
    let mut team = library.assemble(args.team.as_deref(), &overrides)?;
    if demo {
        "demo-squad".clone_into(&mut team.name);
    }
    Ok(team)
}

fn start_mission(
    runtime: &Runtime,
    app: &mut App,
    args: &Args,
    root: &Path,
    goal: String,
) -> Result<UnboundedReceiver<TeamEvent>> {
    let team = app.team.clone();
    let brains = build_brains(args, root, &team)?;
    let engine = MissionEngine::new(team, Goal::new(goal), brains)?;
    let (control, receiver) = launch(runtime, engine, args, root)?;
    app.attach(control);
    Ok(receiver)
}

fn resume_mission(
    runtime: &Runtime,
    app: &mut App,
    args: &Args,
    root: &Path,
    prefix: &str,
) -> Result<UnboundedReceiver<TeamEvent>> {
    let store = open_store(root)?;
    let state = load_mission(&store, &resolve_mission_id(&store, prefix)?)?;
    let brains = build_brains(args, root, &state.team)?;
    let options = ResumeOptions {
        rounds: args.rounds,
        note: args.note.clone(),
        ..ResumeOptions::default()
    };
    let engine = MissionEngine::resume(state.clone(), brains, options)?;
    let (control, receiver) = launch(runtime, engine, args, root)?;
    app.attach_resumed(state, control);
    Ok(receiver)
}

/// Wires the live feed and persistence into an engine and runs it in the background.
fn launch(
    runtime: &Runtime,
    engine: MissionEngine,
    args: &Args,
    root: &Path,
) -> Result<(MissionControl, UnboundedReceiver<TeamEvent>)> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = engine
        .with_sink(Arc::new(ChannelEventSink::new(sender)))
        .interactive(true);
    if !args.no_persist {
        engine = engine.with_sink(Arc::new(SqliteTeamSink::new(Arc::new(open_store(root)?))));
    }
    let control = engine.control();
    runtime.spawn(engine.run());
    Ok((control, receiver))
}

fn build_brains(args: &Args, root: &Path, team: &TeamSpec) -> Result<BrainRouter> {
    let simulated = match args.brain {
        BrainChoice::Simulated => true,
        BrainChoice::Runtime => false,
        BrainChoice::Auto => {
            args.provider == "mock" && team.members.iter().all(|member| member.provider.is_none())
        }
    };
    if simulated {
        return Ok(BrainRouter::new(Arc::new(PacedBrain::new(
            Arc::new(SimulatedBrain),
            Duration::from_millis(args.pace_ms),
        ))));
    }
    let config = Config::load_from(root)?.config;
    let (provider, model, base_url, api_key_env) = (
        args.provider.clone(),
        args.model.clone(),
        args.base_url.clone(),
        args.api_key_env.clone(),
    );
    let providers: nexus_teams::ProviderFactory = Arc::new(move |bot: &BotProfile| {
        let name = bot.provider.as_deref().unwrap_or(&provider);
        let built = provider_from_name(name, &base_url, &api_key_env)
            .map_err(|error| TeamError::Brain(error.to_string()))?;
        Ok((built, bot.model.clone().unwrap_or_else(|| model.clone())))
    });
    Ok(BrainRouter::new(Arc::new(RuntimeBrain::new(
        config, providers,
    ))))
}

fn open_store(root: &Path) -> Result<SqliteStore> {
    let directory = root.join(".nexus");
    fs::create_dir_all(&directory)?;
    Ok(SqliteStore::open(directory.join("nexus.db"))?)
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original(info);
    }));
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use nexus_teams::TeamLibrary;

    use super::{Args, assemble_team};

    fn library() -> TeamLibrary {
        TeamLibrary::new(std::env::temp_dir().join("nexus-tui-no-such-root"))
    }

    #[test]
    fn default_team_is_a_demo_squad() {
        let team = assemble_team(&library(), &Args::parse_from(["nexus-tui"])).expect("team");
        assert_eq!(team.name, "demo-squad");
        assert_eq!(team.members.len(), 4);
        assert_eq!(team.approvers, vec!["rev".to_owned()]);
    }

    #[test]
    fn templates_seed_the_console_team() {
        let args = Args::parse_from(["nexus-tui", "--template", "research", "--approval", "any"]);
        let team = assemble_team(&library(), &args).expect("team");
        assert_eq!(team.name, "research");
        assert_eq!(team.members.len(), 5);
        assert_eq!(team.approval, nexus_teams::ApprovalRule::Any);
    }

    #[test]
    fn resume_flags_require_a_mission() {
        assert!(Args::try_parse_from(["nexus-tui", "--rounds", "3"]).is_err());
        assert!(Args::try_parse_from(["nexus-tui", "--resume", "ab12", "--goal", "x"]).is_err());
        let args = Args::parse_from(["nexus-tui", "--resume", "ab12", "--rounds", "3"]);
        assert_eq!(args.rounds, Some(3));
    }

    #[test]
    fn members_replace_the_demo_squad() {
        let args = Args::parse_from(["nexus-tui", "--member", "pm:lead", "--member", "qa:tester"]);
        let team = assemble_team(&library(), &args).expect("team");
        assert_eq!(team.name, "ad-hoc");
        assert_eq!(team.members.len(), 2);
    }
}
