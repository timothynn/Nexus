use std::{io::{self, stdout}, sync::{mpsc::{self, Receiver, Sender}, Arc}, thread};

use anyhow::Result;
use crossterm::{event::{self, Event, KeyCode, KeyEventKind}, execute, terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen}};
use nexus_agents::{AgentEvent, AgentEventSink};
use ratatui::{backend::CrosstermBackend, layout::{Constraint, Direction, Layout, Rect}, style::{Modifier, Style}, text::{Line, Span}, widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap}, Frame, Terminal};

const TABS: [&str; 6] = ["Run", "Agents", "Context", "Sessions", "Config", "Help"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus { Tabs, Command, Timeline }

struct TuiEventSink { sender: Sender<AgentEvent> }
impl AgentEventSink for TuiEventSink { fn record(&self, event: AgentEvent) { let _ = self.sender.send(event); } }

enum RuntimeCommand { Run(String), Cancel, Shutdown }

struct App {
    tab: usize, focus: Focus, command: String, events: Vec<String>, should_quit: bool,
    status: String, receiver: Receiver<AgentEvent>, dispatcher: Sender<RuntimeCommand>, selected_event: usize,
}
impl App {
    fn new(receiver: Receiver<AgentEvent>, dispatcher: Sender<RuntimeCommand>) -> Self {
        Self { tab: 0, focus: Focus::Tabs, command: String::new(), should_quit: false, selected_event: 0,
            status: "Ready · runtime command bridge online".to_owned(), receiver, dispatcher,
            events: vec!["system.ready · runtime command and event bridges online".to_owned()] }
    }
    fn drain_runtime_events(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            let line = format_event(&event);
            self.status = match &event {
                AgentEvent::RunStarted { run_name } => format!("Running: {run_name}"),
                AgentEvent::RunCompleted { .. } => "Run completed".to_owned(),
                AgentEvent::RunCancelled { .. } => "Run cancelled".to_owned(),
                AgentEvent::WorkerFailed { error, .. } => format!("Worker failed: {error}"),
                _ => self.status.clone(),
            };
            self.events.push(line); self.selected_event = self.events.len().saturating_sub(1);
        }
    }
}

fn format_event(event: &AgentEvent) -> String {
    match event {
        AgentEvent::RunStarted { run_name } => format!("run.started · {run_name}"),
        AgentEvent::LayerStarted { tasks } => format!("layer.started · {}", tasks.join(", ")),
        AgentEvent::WorkerStarted { task_id, agent_index, workspace } => format!("worker.started · {task_id} · worker {} · {workspace}", agent_index + 1),
        AgentEvent::WorkerCompleted { task_id, agent_index } => format!("worker.completed · {task_id} · worker {}", agent_index + 1),
        AgentEvent::WorkerFailed { task_id, agent_index, error } => format!("worker.failed · {task_id} · worker {} · {error}", agent_index + 1),
        AgentEvent::RoleStarted { role, task_id } => format!("role.started · {role:?} · {task_id}"),
        AgentEvent::RoleCompleted { role, task_id } => format!("role.completed · {role:?} · {task_id}"),
        AgentEvent::RunCompleted { run_name } => format!("run.completed · {run_name}"),
        AgentEvent::RunCancelled { run_name } => format!("run.cancelled · {run_name}"),
    }
}

fn main() -> Result<()> {
    let (event_sender, event_receiver) = mpsc::channel();
    let event_sink: Arc<dyn AgentEventSink> = Arc::new(TuiEventSink { sender: event_sender });
    let (dispatcher, commands) = mpsc::channel();
    let runtime = spawn_runtime_bridge(event_sink, commands);
    enable_raw_mode()?; let mut out = stdout(); execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out); let mut terminal = Terminal::new(backend)?;
    let result = run(&mut terminal, event_receiver, dispatcher.clone());
    let _ = dispatcher.send(RuntimeCommand::Shutdown); let _ = runtime.join();
    disable_raw_mode()?; execute!(terminal.backend_mut(), LeaveAlternateScreen)?; terminal.show_cursor()?; result
}

fn spawn_runtime_bridge(event_sink: Arc<dyn AgentEventSink>, commands: Receiver<RuntimeCommand>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut active_run: Option<String> = None;
        while let Ok(command) = commands.recv() {
            match command {
                RuntimeCommand::Run(task) => {
                    if active_run.is_some() {
                        event_sink.record(AgentEvent::WorkerFailed { task_id: "dispatch".to_owned(), agent_index: 0, error: "a run is already active; cancel it before starting another".to_owned() });
                        continue;
                    }
                    let run_name = format!("tui-{}", task.split_whitespace().next().unwrap_or("run"));
                    active_run = Some(run_name.clone());
                    event_sink.record(AgentEvent::RunStarted { run_name: run_name.clone() });
                    event_sink.record(AgentEvent::LayerStarted { tasks: vec![task.clone()] });
                    event_sink.record(AgentEvent::WorkerStarted { task_id: task.clone(), agent_index: 0, workspace: "runtime-dispatch".to_owned() });
                    event_sink.record(AgentEvent::WorkerCompleted { task_id: task.clone(), agent_index: 0 });
                    event_sink.record(AgentEvent::RunCompleted { run_name });
                    active_run = None;
                }
                RuntimeCommand::Cancel => {
                    if let Some(run_name) = active_run.take() { event_sink.record(AgentEvent::RunCancelled { run_name }); }
                }
                RuntimeCommand::Shutdown => break,
            }
        }
    })
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, receiver: Receiver<AgentEvent>, dispatcher: Sender<RuntimeCommand>) -> Result<()> {
    let mut app = App::new(receiver, dispatcher);
    while !app.should_quit {
        app.drain_runtime_events(); terminal.draw(|frame| render(frame, &app))?;
        if event::poll(std::time::Duration::from_millis(100))? { if let Event::Key(key) = event::read()? { if key.kind == KeyEventKind::Press { handle_key(&mut app, key.code); } } }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') if app.focus != Focus::Command => app.should_quit = true,
        KeyCode::Char('c') if app.focus != Focus::Command => { let _ = app.dispatcher.send(RuntimeCommand::Cancel); app.status = "Cancellation requested".to_owned(); },
        KeyCode::Tab => app.focus = match app.focus { Focus::Tabs => Focus::Command, Focus::Command => Focus::Timeline, Focus::Timeline => Focus::Tabs },
        KeyCode::Left if app.focus == Focus::Tabs => app.tab = app.tab.saturating_sub(1),
        KeyCode::Right if app.focus == Focus::Tabs => app.tab = (app.tab + 1).min(TABS.len() - 1),
        KeyCode::Up if app.focus == Focus::Timeline => app.selected_event = app.selected_event.saturating_sub(1),
        KeyCode::Down if app.focus == Focus::Timeline => app.selected_event = (app.selected_event + 1).min(app.events.len().saturating_sub(1)),
        KeyCode::Char(c) if app.focus == Focus::Command => app.command.push(c),
        KeyCode::Backspace if app.focus == Focus::Command => { app.command.pop(); }
        KeyCode::Enter if app.focus == Focus::Command => {
            let command = app.command.trim().to_owned();
            if !command.is_empty() { match app.dispatcher.send(RuntimeCommand::Run(command.clone())) { Ok(()) => { app.events.push(format!("command.dispatched · {command}")); app.status = format!("Dispatching: {command}"); }, Err(_) => { app.events.push("command.failed · runtime bridge offline".to_owned()); app.status = "Runtime bridge offline".to_owned(); } } app.command.clear(); app.selected_event = app.events.len().saturating_sub(1); }
        }
        KeyCode::Esc => { app.command.clear(); app.focus = Focus::Tabs; }
        _ => {}
    }
}

fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Min(8), Constraint::Length(3)]).split(frame.area());
    frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(" ◈ NEXUS", Style::default().add_modifier(Modifier::BOLD)), Span::raw("   programmable AI harness                         "), Span::styled("● LIVE", Style::default().add_modifier(Modifier::BOLD))])).block(Block::default().borders(Borders::ALL)), chunks[0]);
    frame.render_widget(Tabs::new(TABS).select(app.tab).block(Block::default().borders(Borders::ALL).title("Workspace")).highlight_style(Style::default().add_modifier(Modifier::BOLD)), chunks[1]);
    let body = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(32), Constraint::Percentage(68)]).split(chunks[2]);
    render_sidebar(frame, body[0], app); render_main(frame, body[1], app);
    let prompt = if app.focus == Focus::Command { format!("> {}▌", app.command) } else { format!("> {}", app.command) };
    frame.render_widget(Paragraph::new(prompt).block(Block::default().borders(Borders::ALL).title(format!("Command · {} · Tab focus · c cancel · q quit", app.status))).wrap(Wrap { trim: true }), chunks[3]);
}

fn render_sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let workers = app.events.iter().filter(|event| event.starts_with("worker.")).count();
    let agents = ["Supervisor · runtime handoff", "Workers · live stream", "Reviewer · explicit review", "Cancel · shared dispatch boundary"];
    let title = format!("Agents · {workers} worker events");
    frame.render_widget(List::new(agents.iter().map(|item| ListItem::new(*item)).collect::<Vec<_>>()).block(Block::default().borders(Borders::ALL).title(title)), area);
}

fn render_main(frame: &mut Frame, area: Rect, app: &App) {
    let content = match app.tab {
        0 => "Enter a task in the command bar to dispatch it through the Nexus runtime bridge.\n\nPress c outside command mode to request cancellation.",
        1 => "Dependency graph, worker state, handoffs, supervisor, and reviewer.\n\n↑/↓ scroll the live event timeline when Timeline focus is active.",
        2 => "Repository context, instructions, skills, token budgets, and Git-aware priorities.",
        3 => "Durable SQLite sessions, replay, run history, and review artifacts.",
        4 => "Layered configuration, provider/model selection, permissions, plugins, and sandbox policy.",
        _ => "Tab: change focus\n←/→: switch workspace tab\n↑/↓: inspect timeline\nEnter: dispatch command\nc: cancel active run\nEsc: leave command input\nq: quit",
    };
    let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(42), Constraint::Percentage(58)]).split(area);
    frame.render_widget(Paragraph::new(content).block(Block::default().borders(Borders::ALL).title(TABS[app.tab])).wrap(Wrap { trim: true }), chunks[0]);
    let timeline = app.events.iter().enumerate().rev().take(10).rev().map(|(index, event)| { let prefix = if index == app.selected_event { "▶ " } else { "  " }; ListItem::new(format!("{prefix}{event}")) }).collect::<Vec<_>>();
    frame.render_widget(List::new(timeline).block(Block::default().borders(Borders::ALL).title("Live runtime event timeline")), chunks[1]);
}
