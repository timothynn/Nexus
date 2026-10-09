//! Console state: folds mission events into a Teams-style view model.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nexus_teams::{
    Channel, MissionControl, MissionState, OPERATOR_HELP, OperatorCommand, TeamEvent, TeamSpec,
    conversation::GENERAL, parse_operator_line,
};

const ACTIVITY_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Channels,
    Messages,
    Composer,
}

/// Something the event loop must do outside the view model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    StartMission(String),
}

pub struct App {
    pub team: TeamSpec,
    pub mission: Option<MissionState>,
    pub control: Option<MissionControl>,
    pub focus: Focus,
    pub selected_channel: usize,
    pub input: String,
    /// Lines scrolled up from the newest message.
    pub scroll: usize,
    pub notice: String,
    pub speaking: BTreeSet<String>,
    pub activity: VecDeque<String>,
    pub unread: BTreeMap<String, usize>,
    pub show_help: bool,
    pub read_only: bool,
    pub should_quit: bool,
}

impl App {
    #[must_use]
    pub fn new(team: TeamSpec) -> Self {
        Self {
            team,
            mission: None,
            control: None,
            focus: Focus::Composer,
            selected_channel: 0,
            input: String::new(),
            scroll: 0,
            notice: "Type /goal <what you want> to start a mission · F1 help".to_owned(),
            speaking: BTreeSet::new(),
            activity: VecDeque::new(),
            unread: BTreeMap::new(),
            show_help: false,
            read_only: false,
            should_quit: false,
        }
    }

    /// Replaces the view with a replayed (read-only) mission.
    pub fn load_replay(&mut self, state: MissionState) {
        self.team = state.team.clone();
        self.notice = format!("Replay of {} · {} · read-only", state.id, state.status);
        self.mission = Some(state);
        self.read_only = true;
        self.focus = Focus::Messages;
    }

    pub fn attach(&mut self, control: MissionControl) {
        self.control = Some(control);
        self.mission = None;
        self.read_only = false;
        self.speaking.clear();
        self.unread.clear();
        self.activity.clear();
        self.selected_channel = 0;
        self.scroll = 0;
    }

    /// Attaches to a resumed mission, starting from its persisted state.
    pub fn attach_resumed(&mut self, state: MissionState, control: MissionControl) {
        self.attach(control);
        self.team = state.team.clone();
        self.notice = format!("Resuming {} ({})…", state.id, state.status);
        self.mission = Some(state);
        self.focus = Focus::Composer;
    }

    #[must_use]
    pub fn running(&self) -> bool {
        self.mission
            .as_ref()
            .is_some_and(|mission| !mission.status.is_terminal())
            && self.control.is_some()
    }

    #[must_use]
    pub fn channels(&self) -> Vec<Channel> {
        self.mission.as_ref().map_or_else(
            || self.team.all_channels(),
            |mission| mission.channels.clone(),
        )
    }

    #[must_use]
    pub fn current_channel(&self) -> String {
        self.channels()
            .get(self.selected_channel)
            .map_or_else(|| GENERAL.to_owned(), |channel| channel.name.clone())
    }

    /// Folds one mission event into the view.
    pub fn apply(&mut self, event: &TeamEvent) {
        match event {
            TeamEvent::MissionStarted {
                mission,
                team,
                goal,
            } => {
                self.team = team.clone();
                self.mission = Some(MissionState::new(
                    mission.clone(),
                    team.clone(),
                    goal.clone(),
                ));
                self.notice = format!("Mission {mission} started");
                return;
            }
            TeamEvent::TurnStarted { bot, reason, .. } => {
                self.speaking.insert(bot.clone());
                self.log(format!("@{bot} has the floor — {reason}"));
            }
            TeamEvent::TurnCompleted { bot, actions, .. } => {
                self.speaking.remove(bot);
                if !actions.is_empty() {
                    self.log(format!("@{bot} did {}", actions.join(", ")));
                }
            }
            TeamEvent::TurnFailed { bot, error, .. } => {
                self.speaking.remove(bot);
                self.log(format!("✖ @{bot} failed: {error}"));
            }
            TeamEvent::ActionDenied {
                bot,
                action,
                reason,
            } => {
                self.log(format!("⚠ @{bot} {action} denied: {reason}"));
            }
            TeamEvent::BotActivity { bot, kind, detail } => {
                self.log(format!("⚙ @{bot} {kind} {detail}"));
            }
            TeamEvent::StatusChanged { status, reason } => {
                self.notice = format!("Mission {status}: {reason}");
            }
            TeamEvent::MissionResumed { budget, .. } => {
                self.notice = format!("Mission resumed · budget now {} rounds", budget.max_rounds);
            }
            TeamEvent::HumanQuestion { bot, question } => {
                self.notice = format!("@{bot} asks you: {question} — reply with /answer <text>");
            }
            TeamEvent::MessagePosted { message } => {
                if message.channel != self.current_channel() {
                    *self.unread.entry(message.channel.clone()).or_default() += 1;
                }
            }
            TeamEvent::MissionFinished { report } => {
                self.speaking.clear();
                self.notice = format!(
                    "Mission {} — {} · {} rounds · {}/{} tasks · /goal to start another",
                    report.status,
                    report.reason,
                    report.rounds,
                    report.tasks_done,
                    report.tasks_total
                );
            }
            _ => {}
        }
        if let Some(mission) = &mut self.mission {
            mission.apply(event);
        }
    }

    fn log(&mut self, line: String) {
        if self.activity.len() == ACTIVITY_LIMIT {
            self.activity.pop_front();
        }
        self.activity.push_back(line);
    }

    fn select_channel(&mut self, index: usize) {
        let count = self.channels().len();
        if count == 0 {
            return;
        }
        self.selected_channel = index.min(count - 1);
        self.scroll = 0;
        self.unread.remove(&self.current_channel());
    }

    /// Handles a key press; returns an action for the event loop when needed.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit();
            return None;
        }
        match key.code {
            KeyCode::F(1) => self.show_help = !self.show_help,
            KeyCode::Tab => self.cycle_focus(true),
            KeyCode::BackTab => self.cycle_focus(false),
            KeyCode::Esc => {
                if self.show_help {
                    self.show_help = false;
                } else {
                    self.focus = Focus::Channels;
                }
            }
            _ if self.focus == Focus::Composer => return self.composer_key(key.code),
            KeyCode::Char('q') => self.quit(),
            KeyCode::Char('i' | '/') if !self.read_only => {
                self.focus = Focus::Composer;
                if key.code == KeyCode::Char('/') {
                    self.input.push('/');
                }
            }
            KeyCode::Char('p') => self.toggle_pause(),
            KeyCode::Char('a') => self.send_command("/approve"),
            KeyCode::Char('x') => self.send_command("/cancel"),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::End => self.scroll = 0,
            _ => {}
        }
        None
    }

    fn composer_key(&mut self, code: KeyCode) -> Option<Action> {
        match code {
            KeyCode::Enter => {
                let line = std::mem::take(&mut self.input);
                return self.submit(&line);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(character) => self.input.push(character),
            KeyCode::Up => self.move_channel(-1),
            KeyCode::Down => self.move_channel(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            _ => {}
        }
        None
    }

    fn cycle_focus(&mut self, forward: bool) {
        let order = [Focus::Channels, Focus::Messages, Focus::Composer];
        let index = order
            .iter()
            .position(|focus| *focus == self.focus)
            .unwrap_or(0);
        let step = if forward { 1 } else { order.len() - 1 };
        let next = (index + step) % order.len();
        self.focus = order[next];
        if self.read_only && self.focus == Focus::Composer {
            self.cycle_focus(forward);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Focus::Channels => self.move_channel(delta),
            Focus::Messages => {
                self.scroll = if delta < 0 {
                    self.scroll.saturating_add(1)
                } else {
                    self.scroll.saturating_sub(1)
                };
            }
            Focus::Composer => {}
        }
    }

    fn move_channel(&mut self, delta: isize) {
        let next = self.selected_channel.saturating_add_signed(delta);
        self.select_channel(next);
    }

    fn toggle_pause(&mut self) {
        let paused = self
            .mission
            .as_ref()
            .is_some_and(|mission| mission.status == nexus_teams::MissionStatus::Paused);
        self.send_command(if paused { "/resume" } else { "/pause" });
    }

    fn send_command(&mut self, line: &str) {
        let _ = self.submit(line);
    }

    fn quit(&mut self) {
        if let Some(control) = &self.control {
            if self.running() {
                control.cancel();
            }
        }
        self.should_quit = true;
    }

    /// Interprets a composer line.
    pub fn submit(&mut self, line: &str) -> Option<Action> {
        if self.read_only {
            "Replays are read-only".clone_into(&mut self.notice);
            return None;
        }
        match parse_operator_line(line, &self.current_channel())? {
            OperatorCommand::Goal(goal) => {
                if self.running() {
                    "A mission is already running — /cancel it first".clone_into(&mut self.notice);
                    return None;
                }
                return Some(Action::StartMission(goal));
            }
            OperatorCommand::Help => self.show_help = true,
            OperatorCommand::Invalid(message) => self.notice = message,
            OperatorCommand::Cancel => match &self.control {
                Some(control) if self.running() => {
                    control.cancel();
                    "Cancellation requested".clone_into(&mut self.notice);
                }
                _ => "No mission is running".clone_into(&mut self.notice),
            },
            OperatorCommand::Control(input) => match &self.control {
                Some(control) if self.running() => {
                    if control.send(input).is_err() {
                        "Mission is no longer accepting input".clone_into(&mut self.notice);
                    }
                }
                _ => "Start a mission first: /goal <what you want>".clone_into(&mut self.notice),
            },
        }
        None
    }

    #[must_use]
    pub fn help_text() -> String {
        format!(
            "{OPERATOR_HELP}\n\nkeys:\n  Tab / Shift+Tab     move focus (channels, messages, composer)\n  ↑/↓ (composer)      switch channel\n  ↑/↓ j/k             select channel or scroll messages\n  PgUp/PgDn End       scroll messages\n  i or /              focus the composer\n  p                   pause / resume\n  a                   approve completion\n  x                   cancel mission\n  F1                  toggle help\n  q or Ctrl+C         quit (cancels a running mission)"
        )
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::Arc;

    use nexus_teams::{
        BotProfile, BrainRouter, Budget, Channel, Goal, Message, MessageId, MessageKind,
        MissionEngine, MissionId, MissionState, MissionStatus, SimulatedBrain, TeamEvent, TeamSpec,
    };

    use super::{Action, App, Focus};

    fn team() -> TeamSpec {
        TeamSpec::new(
            "t",
            vec![
                BotProfile::from_archetype("lead", "lead").expect("lead"),
                BotProfile::from_archetype("ada", "engineer").expect("eng"),
            ],
        )
        .with_channel(Channel::open("design", "UX"))
    }

    fn started(app: &mut App) {
        app.apply(&TeamEvent::MissionStarted {
            mission: MissionId("mission-x".to_owned()),
            team: team(),
            goal: Goal::new("g"),
        });
    }

    fn type_line(app: &mut App, text: &str) -> Option<Action> {
        app.focus = Focus::Composer;
        for character in text.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    #[test]
    fn goal_command_starts_missions() {
        let mut app = App::new(team());
        assert_eq!(
            type_line(&mut app, "/goal Ship it"),
            Some(Action::StartMission("Ship it".to_owned()))
        );
        assert!(app.input.is_empty());
    }

    #[test]
    fn posts_without_a_mission_explain_themselves() {
        let mut app = App::new(team());
        assert_eq!(type_line(&mut app, "hello"), None);
        assert!(app.notice.contains("/goal"));
    }

    #[test]
    fn events_fold_into_mission_state_and_unread_counts() {
        let mut app = App::new(team());
        started(&mut app);
        app.apply(&TeamEvent::TurnStarted {
            round: 1,
            bot: "ada".to_owned(),
            reason: "r".to_owned(),
        });
        assert!(app.speaking.contains("ada"));
        app.apply(&TeamEvent::MessagePosted {
            message: Message {
                id: MessageId(1),
                channel: "design".to_owned(),
                author: "ada".to_owned(),
                kind: MessageKind::Chat,
                text: "mock".to_owned(),
                thread: None,
                mentions: Vec::new(),
                round: 1,
            },
        });
        assert_eq!(app.unread.get("design"), Some(&1));
        assert_eq!(app.mission.as_ref().expect("mission").messages.len(), 1);
        app.focus = Focus::Channels;
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.current_channel(), "design");
        assert!(!app.unread.contains_key("design"));
    }

    #[test]
    fn replays_are_read_only() {
        let mut app = App::new(team());
        let state =
            nexus_teams::MissionState::new(MissionId("m".to_owned()), team(), Goal::new("g"));
        app.load_replay(state);
        assert_eq!(type_line(&mut app, "/goal again"), None);
        assert!(app.notice.contains("read-only"));
    }

    #[test]
    fn resumed_missions_continue_from_persisted_state() {
        let mut app = App::new(team());
        let mut state =
            MissionState::new(MissionId("mission-r".to_owned()), team(), Goal::new("g"));
        state.apply(&TeamEvent::StatusChanged {
            status: MissionStatus::OutOfBudget,
            reason: "reached 1 rounds".to_owned(),
        });
        let engine = MissionEngine::new(
            team(),
            Goal::new("g"),
            BrainRouter::new(Arc::new(SimulatedBrain)),
        )
        .expect("engine");
        app.attach_resumed(state, engine.control());
        assert!(!app.running(), "terminal until the resume event arrives");
        app.apply(&TeamEvent::MissionResumed {
            budget: Budget::default(),
            note: None,
        });
        assert!(app.running());
        assert!(app.notice.contains("resumed"));
        assert_eq!(app.team.name, "t");
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = App::new(team());
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }
}
