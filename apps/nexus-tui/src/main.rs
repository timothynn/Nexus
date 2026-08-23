use std::io::{self, stdout};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap},
    Frame, Terminal,
};

const TABS: [&str; 6] = ["Run", "Agents", "Context", "Sessions", "Config", "Help"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus { Tabs, Command, Timeline }

struct App { tab: usize, focus: Focus, command: String, events: Vec<String>, should_quit: bool, status: String }
impl Default for App {
    fn default() -> Self {
        Self { tab: 0, focus: Focus::Tabs, command: String::new(), should_quit: false, status: "Ready · local-first operator console".to_owned(), events: vec!["run.started · operator console initialized".to_owned(), "system.ready · event timeline online".to_owned()] }
    }
}

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout(); execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out); let mut terminal = Terminal::new(backend)?;
    let result = run(&mut terminal);
    disable_raw_mode()?; execute!(terminal.backend_mut(), LeaveAlternateScreen)?; terminal.show_cursor()?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::default();
    while !app.should_quit {
        terminal.draw(|frame| render(frame, &app))?;
        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press { handle_key(&mut app, key.code); }
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') if app.focus != Focus::Command => app.should_quit = true,
        KeyCode::Tab => app.focus = match app.focus { Focus::Tabs => Focus::Command, Focus::Command => Focus::Timeline, Focus::Timeline => Focus::Tabs },
        KeyCode::Left if app.focus == Focus::Tabs => app.tab = app.tab.saturating_sub(1),
        KeyCode::Right if app.focus == Focus::Tabs => app.tab = (app.tab + 1).min(TABS.len() - 1),
        KeyCode::Char(c) if app.focus == Focus::Command => app.command.push(c),
        KeyCode::Backspace if app.focus == Focus::Command => { app.command.pop(); }
        KeyCode::Enter if app.focus == Focus::Command => {
            let command = app.command.trim().to_owned();
            if !command.is_empty() { app.events.push(format!("command.queued · {command}")); app.status = format!("Queued: {command}"); app.command.clear(); }
        }
        KeyCode::Esc => { app.command.clear(); app.focus = Focus::Tabs; }
        _ => {}
    }
}

fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Min(8), Constraint::Length(3)]).split(frame.area());
    frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(" ◈ NEXUS", Style::default().add_modifier(Modifier::BOLD)), Span::raw("   programmable AI harness                         "), Span::styled("● READY", Style::default().add_modifier(Modifier::BOLD))])).block(Block::default().borders(Borders::ALL)), chunks[0]);
    frame.render_widget(Tabs::new(TABS).select(app.tab).block(Block::default().borders(Borders::ALL).title("Workspace")).highlight_style(Style::default().add_modifier(Modifier::BOLD)), chunks[1]);
    let body = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(32), Constraint::Percentage(68)]).split(chunks[2]);
    render_sidebar(frame, body[0]); render_main(frame, body[1], app);
    let prompt = if app.focus == Focus::Command { format!("> {}▌", app.command) } else { format!("> {}", app.command) };
    frame.render_widget(Paragraph::new(prompt).block(Block::default().borders(Borders::ALL).title(format!("Command · {} · Tab switches focus · q quits", app.status))).wrap(Wrap { trim: true }), chunks[3]);
}

fn render_sidebar(frame: &mut Frame, area: Rect) {
    let agents = ["● Supervisor · waiting", "● Worker-1 · idle", "● Worker-2 · idle", "○ Reviewer · waiting"];
    frame.render_widget(List::new(agents.iter().map(|item| ListItem::new(*item)).collect::<Vec<_>>()).block(Block::default().borders(Borders::ALL).title("Agents")), area);
}

fn render_main(frame: &mut Frame, area: Rect, app: &App) {
    let content = match app.tab {
        0 => "Run a task from the command bar.\n\nThis foundation is an operator surface over Nexus runtime primitives.\n\nCommands are queued locally; live runtime binding is the next increment.",
        1 => "Dependency graph, worker state, handoffs, supervisor, reviewer.\n\nLive events will plug into AgentEventSink.",
        2 => "Repository context, instructions, skills, token budgets, and Git-aware priorities.",
        3 => "Durable SQLite sessions, replay, run history, and review artifacts.",
        4 => "Layered configuration, provider/model selection, permissions, plugins, and sandbox policy.",
        _ => "Tab: change focus\n←/→: switch workspace tab\nEnter: queue command\nEsc: leave command input\nq: quit",
    };
    let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);
    frame.render_widget(Paragraph::new(content).block(Block::default().borders(Borders::ALL).title(TABS[app.tab])).wrap(Wrap { trim: true }), chunks[0]);
    let timeline = app.events.iter().rev().take(8).rev().map(|event| ListItem::new(event.as_str())).collect::<Vec<_>>();
    frame.render_widget(List::new(timeline).block(Block::default().borders(Borders::ALL).title("Live event timeline")), chunks[1]);
}
