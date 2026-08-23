use std::{
    io::{self, stdout},
    sync::{mpsc::{self, Receiver, Sender}, Arc},
};

use anyhow::Result;
use crossterm::{event::{self, Event, KeyCode, KeyEventKind}, execute, terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen}};
use nexus_agents::{AgentEvent, AgentEventSink};
use ratatui::{backend::CrosstermBackend, layout::{Constraint, Direction, Layout, Rect}, style::{Modifier, Style}, text::{Line, Span}, widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap}, Frame, Terminal};

const TABS: [&str; 6] = ["Run", "Agents", "Context", "Sessions", "Config", "Help"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus { Tabs, Command, Timeline }

struct TuiEventSink { sender: Sender<AgentEvent> }
impl AgentEventSink for TuiEventSink { fn record(&self, event: AgentEvent) { let _ = self.sender.send(event); } }

struct App {
    tab: usize, focus: Focus, command: String, events: Vec<String>, should_quit: bool,
    status: String, receiver: Receiver<AgentEvent>, selected_event: usize,
}
impl App {
    fn new(receiver: Receiver<AgentEvent>) -> Self {
        Self { tab: 0, focus: Focus::Tabs, command: String::new(), should_quit: false, selected_event: 0,
            status: "Ready · connected to Nexus event stream".to_owned(), receiver,
            events: vec!["system.ready · runtime event bridge online".to_owned()] }
    }
    fn drain_runtime_events(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            let line = format_event(&event);
            if matches!(event, AgentEvent::RunCancelled { .. }) { self.status = "Run cancelled".to_owned(); }
            if matches!(event, AgentEvent::RunCompleted { .. }) { self.status = "Run completed".to_owned(); }
            self.events.push(line);
            self.selected_event = self.events.len().saturating_sub(1);
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
    let (sender, receiver) = mpsc::channel();
    let _event_sink: Arc<dyn AgentEventSink> = Arc::new(TuiEventSink { sender });
    enable_raw_mode()?; let mut out = stdout(); execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out); let mut terminal = Terminal::new(backend)?;
    let result = run(&mut terminal, receiver);
    disable_raw_mode()?; execute!(terminal.backend_mut(), LeaveAlternateScreen)?; terminal.show_cursor()?; result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, receiver: Receiver<AgentEvent>) -> Result<()> {
    let mut app = App::new(receiver);
    while !app.should_quit {
        app.drain_runtime_events(); terminal.draw(|frame| render(frame, &app))?;
        if event::poll(std::time::Duration::from_millis(100))? { if let Event::Key(key) = event::read()? { if key.kind == KeyEventKind::Press { handle_key(&mut app, key.code); } } }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') if app.focus != Focus::Command => app.should_quit = true,
        KeyCode::Tab => app.focus = match app.focus { Focus::Tabs => Focus::Command, Focus::Command => Focus::Timeline, Focus::Timeline => Focus::Tabs },
        KeyCode::Left if app.focus == Focus::Tabs => app.tab = app.tab.saturating_sub(1),
        KeyCode::Right if app.focus == Focus::Tabs => app.tab = (app.tab + 1).min(TABS.len() - 1),
        KeyCode::Up if app.focus == Focus::Timeline => app.selected_event = app.selected_event.saturating_sub(1),
        KeyCode::Down if app.focus == Focus::Timeline => app.selected_event = (app.selected_event + 1).min(app.events.len().saturating_sub(1)),
        KeyCode::Char(c) if app.focus == Focus::Command => app.command.push(c),
        KeyCode::Backspace if app.focus == Focus::Command => { app.command.pop(); }
        KeyCode::Enter if app.focus == Focus::Command => {
            let command = app.command.trim().to_owned();
            if !command.is_empty() { app.events.push(format!("command.queued · {command}")); app.status = format!("Queued: {command} · runtime dispatch adapter next"); app.command.clear(); app.selected_event = app.events.len().saturating_sub(1); }
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
    frame.render_widget(Paragraph::new(prompt).block(Block::default().borders(Borders::ALL).title(format!("Command · {} · Tab focus · q quit", app.status))).wrap(Wrap { trim: true }), chunks[3]);
}

fn render_sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let workers = app.events.iter().filter(|event| event.starts_with("worker.")).count();
    let agents = ["Supervisor · event-driven", "Workers · live stream", "Reviewer · awaiting handoff", "Cancel · runtime token boundary"];
    let title = format!("Agents · {workers} worker events");
    frame.render_widget(List::new(agents.iter().map(|item| ListItem::new(*item)).collect::<Vec<_>>()).block(Block::default().borders(Borders::ALL).title(title)), area);
}

fn render_main(frame: &mut Frame, area: Rect, app: &App) {
    let content = match app.tab {
        0 => "Run tasks from the command bar.\n\nThe TUI now consumes the same AgentEvent stream used by the multi-agent runtime.\n\nCommand dispatch is the next adapter.",
        1 => "Dependency graph, worker state, handoffs, supervisor, and reviewer.\n\n↑/↓ scroll the live event timeline when Timeline focus is active.",
        2 => "Repository context, instructions, skills, token budgets, and Git-aware priorities.",
        3 => "Durable SQLite sessions, replay, run history, and review artifacts.",
        4 => "Layered configuration, provider/model selection, permissions, plugins, and sandbox policy.",
        _ => "Tab: change focus\n←/→: switch workspace tab\n↑/↓: inspect timeline\nEnter: queue command\nEsc: leave command input\nq: quit",
    };
    let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(42), Constraint::Percentage(58)]).split(area);
    frame.render_widget(Paragraph::new(content).block(Block::default().borders(Borders::ALL).title(TABS[app.tab])).wrap(Wrap { trim: true }), chunks[0]);
    let timeline = app.events.iter().enumerate().rev().take(10).rev().map(|(index, event)| { let prefix = if index == app.selected_event { "▶ " } else { "  " }; ListItem::new(format!("{prefix}{event}")) }).collect::<Vec<_>>();
    frame.render_widget(List::new(timeline).block(Block::default().borders(Borders::ALL).title("Live runtime event timeline")), chunks[1]);
}
