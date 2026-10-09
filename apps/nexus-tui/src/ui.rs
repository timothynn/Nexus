//! Rendering: a Teams-style layout of channels, conversation, roster, and board.

use nexus_teams::{Message, MessageKind, MissionState, MissionStatus, TaskStatus, TeamSpec};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::app::{App, Focus};

const PALETTE: [Color; 6] = [
    Color::Cyan,
    Color::Green,
    Color::Magenta,
    Color::Yellow,
    Color::Blue,
    Color::LightRed,
];

pub fn render(frame: &mut Frame, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());
    render_header(frame, rows[0], app);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(28),
            Constraint::Min(30),
            Constraint::Length(42),
        ])
        .split(rows[1]);
    render_sidebar(frame, columns[0], app);
    render_messages(frame, columns[1], app);
    render_inspector(frame, columns[2], app);
    render_composer(frame, rows[2], app);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {}", app.notice), Style::default().fg(Color::Gray)),
            Span::styled(
                "   Tab focus · ↑↓ channels · PgUp/PgDn scroll · p pause · a approve · x cancel · F1 help · q quit",
                dim(),
            ),
        ])),
        rows[3],
    );
    if app.show_help {
        render_help(frame, frame.area());
    }
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn pane(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(if focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            dim()
        })
}

fn status_style(status: MissionStatus) -> Style {
    let color = match status {
        MissionStatus::Active => Color::Green,
        MissionStatus::Paused => Color::Yellow,
        MissionStatus::AwaitingHuman | MissionStatus::Reviewing => Color::Magenta,
        MissionStatus::Completed => Color::LightGreen,
        MissionStatus::Stalled | MissionStatus::OutOfBudget => Color::LightRed,
        MissionStatus::Failed | MissionStatus::Cancelled => Color::Red,
    };
    Style::default()
        .fg(Color::Black)
        .bg(color)
        .add_modifier(Modifier::BOLD)
}

/// Style for a member handle: the profile's color, else a stable palette slot.
fn author_style(team: &TeamSpec, author: &str) -> Style {
    match author {
        nexus_teams::SYSTEM => dim().add_modifier(Modifier::ITALIC),
        nexus_teams::HUMAN => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        _ => {
            let index = team
                .members
                .iter()
                .position(|member| member.handle == author)
                .unwrap_or(0);
            let color = team
                .members
                .get(index)
                .and_then(|member| member.color.as_deref())
                .and_then(|name| name.parse::<Color>().ok())
                .unwrap_or(PALETTE[index % PALETTE.len()]);
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        }
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled(
            " ◈ NEXUS TEAMS ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", app.team.name),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(mission) = &app.mission {
        spans.push(Span::styled(
            format!(" {} ", mission.status),
            status_style(mission.status),
        ));
        spans.push(Span::raw(format!(
            "  round {}/{} · turns {} · msgs {} · tokens {}  ",
            mission.round,
            mission.team.budget.max_rounds,
            mission.turns,
            mission.messages.len(),
            mission.usage.total()
        )));
        spans.push(Span::styled(
            mission.goal.statement.clone(),
            Style::default().add_modifier(Modifier::ITALIC),
        ));
    } else {
        spans.push(Span::styled(
            " idle ",
            Style::default().fg(Color::Black).bg(Color::DarkGray),
        ));
        spans.push(Span::raw(format!(
            "  {} bots · lead @{} · policy {}",
            app.team.members.len(),
            app.team.lead_handle(),
            app.team.policy
        )));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .block(Block::default().borders(Borders::ALL).border_style(dim())),
        area,
    );
}

fn display_channel(name: &str) -> String {
    name.strip_prefix("dm:")
        .map_or_else(|| name.to_owned(), |pair| pair.replace('+', " ↔ "))
}

fn render_sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let channels = app.channels();
    let wanted = u16::try_from(channels.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let height = wanted.min(area.height / 2).max(3);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(height), Constraint::Min(3)])
        .split(area);
    let current = app.current_channel();
    let channel_lines = channels
        .iter()
        .map(|channel| {
            let selected = channel.name == current;
            let marker = if selected { "▸" } else { " " };
            let sigil = if channel.direct { "✉ " } else { "# " };
            let style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let mut spans = vec![Span::styled(
                format!("{marker}{sigil}{}", display_channel(&channel.name)),
                style,
            )];
            if let Some(unread) = app.unread.get(&channel.name).filter(|count| **count > 0) {
                spans.push(Span::styled(
                    format!(" ({unread})"),
                    Style::default().fg(Color::Yellow),
                ));
            }
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(channel_lines)
            .block(pane("Channels".to_owned(), app.focus == Focus::Channels)),
        parts[0],
    );

    let team = app
        .mission
        .as_ref()
        .map_or(&app.team, |mission| &mission.team);
    let lead = team.lead_handle();
    let mut lines = Vec::new();
    for member in &team.members {
        let dot = if app.speaking.contains(&member.handle) {
            Span::styled("● ", Style::default().fg(Color::Green))
        } else {
            Span::styled("○ ", dim())
        };
        let star = if member.handle == lead { " ★" } else { "" };
        lines.push(Line::from(vec![
            dot,
            Span::styled(
                format!("@{}{star}", member.handle),
                author_style(team, &member.handle),
            ),
        ]));
        let turns = app
            .mission
            .as_ref()
            .and_then(|mission| mission.stats.get(&member.handle))
            .map_or(0, |stats| stats.turns);
        lines.push(Line::from(Span::styled(
            truncate(
                &format!("  {} · {turns} turns", member.role),
                usize::from(area.width.saturating_sub(2)),
            ),
            dim(),
        )));
    }
    lines.push(Line::from(vec![
        Span::styled("○ ", dim()),
        Span::styled("@human (you)", author_style(team, nexus_teams::HUMAN)),
    ]));
    frame.render_widget(
        Paragraph::new(lines).block(pane("Team".to_owned(), false)),
        parts[1],
    );
}

fn render_messages(frame: &mut Frame, area: Rect, app: &App) {
    let channel = app.current_channel();
    let purpose = app
        .channels()
        .into_iter()
        .find(|candidate| candidate.name == channel)
        .map(|candidate| candidate.purpose)
        .unwrap_or_default();
    let title = if purpose.is_empty() {
        format!("#{}", display_channel(&channel))
    } else {
        format!("#{} — {purpose}", display_channel(&channel))
    };
    let block = pane(title, app.focus == Focus::Messages);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width.max(10));
    let lines = app.mission.as_ref().map_or_else(
        || intro_lines(app),
        |mission| message_lines(mission, &channel, width),
    );
    let height = usize::from(inner.height);
    let max_scroll = lines.len().saturating_sub(height);
    let top = max_scroll - app.scroll.min(max_scroll);
    let visible = lines.into_iter().skip(top).take(height).collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(visible), inner);
}

fn intro_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Welcome to Nexus Teams",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Your team \"{}\" is standing by:", app.team.name)),
    ];
    for member in &app.team.members {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("@{}", member.handle),
                author_style(&app.team, &member.handle),
            ),
            Span::styled(format!("  {}", member.role), dim()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Type /goal <what you want the team to achieve> and press Enter.",
    ));
    lines.push(Line::from(Span::styled(
        "While it runs: chat with @mentions, /answer questions, /approve or /reject completion.",
        dim(),
    )));
    lines
}

/// Renders a channel as wrapped lines: messages with headers, thread replies indented.
#[must_use]
pub fn message_lines(mission: &MissionState, channel: &str, width: usize) -> Vec<Line<'static>> {
    let messages = mission
        .messages
        .iter()
        .filter(|message| message.channel == channel)
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    for message in messages.iter().filter(|message| message.thread.is_none()) {
        push_message(&mut lines, mission, message, "", width);
        for reply in messages
            .iter()
            .filter(|reply| reply.thread == Some(message.id))
        {
            push_message(&mut lines, mission, reply, "   ↳ ", width);
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no messages in this channel yet)",
            dim(),
        )));
    }
    lines
}

fn push_message(
    lines: &mut Vec<Line<'static>>,
    mission: &MissionState,
    message: &Message,
    indent: &str,
    width: usize,
) {
    let body_width = width.saturating_sub(indent.chars().count() + 2).max(8);
    if message.kind == MessageKind::System {
        for (index, text) in wrap(&message.text, body_width).into_iter().enumerate() {
            let lead = if index == 0 { "· " } else { "  " };
            lines.push(Line::from(Span::styled(
                format!("{indent}{lead}{text}"),
                dim().add_modifier(Modifier::ITALIC),
            )));
        }
        return;
    }
    let badge = match message.kind {
        MessageKind::TaskUpdate => " ≡ task",
        MessageKind::Question => " ? question",
        MessageKind::Proposal => " ⚑ proposal",
        MessageKind::Vote => " ✓ vote",
        MessageKind::Handoff => " → handoff",
        MessageKind::Direct => " ✉ direct",
        MessageKind::Chat | MessageKind::System => "",
    };
    lines.push(Line::from(vec![
        Span::raw(indent.to_owned()),
        Span::styled(
            format!("@{}", message.author),
            author_style(&mission.team, &message.author),
        ),
        Span::styled(
            format!("{badge}  {} · r{}", message.id, message.round),
            dim(),
        ),
    ]));
    for text in wrap(&message.text, body_width) {
        let mut spans = vec![Span::raw(format!("{indent}  "))];
        spans.extend(highlight_mentions(&text, &mission.team));
        lines.push(Line::from(spans));
    }
}

/// Bolds `@mentions` of known members inside a text line.
fn highlight_mentions(text: &str, team: &TeamSpec) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    for (index, word) in text.split(' ').enumerate() {
        let separator = if index == 0 { "" } else { " " };
        let handle = word.strip_prefix('@').map(|rest| {
            rest.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        });
        match handle {
            Some(handle)
                if team.is_member(handle) || handle == nexus_teams::HUMAN || handle == "all" =>
            {
                plain.push_str(separator);
                spans.push(Span::raw(std::mem::take(&mut plain)));
                spans.push(Span::styled(
                    word.to_owned(),
                    author_style(team, handle).add_modifier(Modifier::UNDERLINED),
                ));
            }
            _ => {
                plain.push_str(separator);
                plain.push_str(word);
            }
        }
    }
    if !plain.is_empty() {
        spans.push(Span::raw(plain));
    }
    spans
}

fn render_inspector(frame: &mut Frame, area: Rect, app: &App) {
    let Some(mission) = &app.mission else {
        frame.render_widget(
            Paragraph::new(setup_lines(&app.team)).block(pane("Mission setup".to_owned(), false)),
            area,
        );
        return;
    };
    let decision = decision_lines(mission, usize::from(area.width.saturating_sub(2)));
    let decision_height = if decision.is_empty() {
        0
    } else {
        u16::try_from(decision.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .min(area.height / 3)
    };
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),
            Constraint::Length(decision_height),
            Constraint::Length(area.height.saturating_sub(decision_height) / 3),
        ])
        .split(area);
    let width = usize::from(area.width.saturating_sub(4));
    let title = format!(
        "Task board · {}/{} done",
        mission.board.count(TaskStatus::Done),
        mission.board.len()
    );
    frame.render_widget(
        Paragraph::new(board_lines(mission, width)).block(pane(title, false)),
        parts[0],
    );
    if decision_height > 0 {
        frame.render_widget(
            Paragraph::new(decision)
                .wrap(Wrap { trim: true })
                .block(pane("Needs attention".to_owned(), true)),
            parts[1],
        );
    }
    let activity_height = usize::from(parts[2].height.saturating_sub(2));
    let activity = app
        .activity
        .iter()
        .rev()
        .take(activity_height)
        .rev()
        .map(|line| Line::from(Span::styled(truncate(line, width), dim())))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(activity).block(pane("Activity".to_owned(), false)),
        parts[2],
    );
}

fn heading(text: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

/// Team configuration shown before a mission starts.
fn setup_lines(team: &TeamSpec) -> Vec<Line<'static>> {
    vec![
        heading("Floor policy"),
        Line::from(format!("  {}", team.policy)),
        Line::from(""),
        heading("Approval"),
        Line::from(format!("  {}", team.approval_summary())),
        Line::from(""),
        heading("Budget"),
        Line::from(format!(
            "  {} rounds · {} turns · stall {}",
            team.budget.max_rounds, team.budget.max_turns, team.budget.stall_rounds
        )),
    ]
}

/// The task board: one status line and one detail line per task.
fn board_lines(mission: &MissionState, width: usize) -> Vec<Line<'static>> {
    let mut board = Vec::new();
    for task in mission.board.tasks() {
        let (symbol, color) = match task.status {
            TaskStatus::Todo => ("○", Color::Gray),
            TaskStatus::InProgress => ("◐", Color::Yellow),
            TaskStatus::Blocked => ("✖", Color::Red),
            TaskStatus::Review => ("◑", Color::Magenta),
            TaskStatus::Done => ("●", Color::Green),
            TaskStatus::Cancelled => ("⊘", Color::DarkGray),
        };
        board.push(Line::from(vec![
            Span::styled(format!("{symbol} "), Style::default().fg(color)),
            Span::styled(
                format!("{} ", task.id),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(truncate(&task.title, width.saturating_sub(5))),
        ]));
        let mut detail = task
            .assignee
            .as_deref()
            .map_or_else(|| "unassigned".to_owned(), |handle| format!("@{handle}"));
        detail.push_str(" · ");
        detail.push_str(task.status.label());
        if !task.depends_on.is_empty() {
            let deps = task
                .depends_on
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            detail.push_str(" · after ");
            detail.push_str(&deps);
        }
        board.push(Line::from(Span::styled(
            format!("   {}", truncate(&detail, width.saturating_sub(3))),
            dim(),
        )));
    }
    if board.is_empty() {
        board.push(Line::from(Span::styled("(no tasks yet)", dim())));
    }
    board
}

/// Pending decisions: completion proposals, votes, and questions for the human.
fn decision_lines(mission: &MissionState, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some((bot, question)) = &mission.pending_question {
        lines.push(Line::from(Span::styled(
            format!("? @{bot} asks you:"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        for text in wrap(question, width.max(8)) {
            lines.push(Line::from(format!("  {text}")));
        }
        lines.push(Line::from(Span::styled("  reply: /answer <text>", dim())));
    }
    if let Some(proposal) = &mission.proposal {
        lines.push(Line::from(Span::styled(
            format!("⚑ @{} proposes completion", proposal.by),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
        for text in wrap(&proposal.summary, width.max(8)).into_iter().take(3) {
            lines.push(Line::from(format!("  {text}")));
        }
        for (voter, vote) in &proposal.votes {
            let (mark, color) = if vote.approve {
                ("✔", Color::Green)
            } else {
                ("✖", Color::Red)
            };
            lines.push(Line::from(Span::styled(
                format!("  {mark} @{voter} {}", vote.reason),
                Style::default().fg(color),
            )));
        }
        for voter in mission.pending_voters() {
            lines.push(Line::from(Span::styled(
                format!("  … @{voter} reviewing"),
                dim(),
            )));
        }
        if mission.status == MissionStatus::AwaitingHuman {
            lines.push(Line::from(Span::styled(
                "  your call: a / /approve or /reject <reason>",
                Style::default().fg(Color::Yellow),
            )));
        }
    }
    lines
}

fn render_composer(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Composer;
    let title = if app.read_only {
        "Replay (read-only)".to_owned()
    } else if app.running() {
        format!(
            "Message #{} as @human · /help",
            display_channel(&app.current_channel())
        )
    } else {
        "Start a mission: /goal <what you want>".to_owned()
    };
    let cursor = if focused { "▌" } else { "" };
    frame.render_widget(
        Paragraph::new(format!("> {}{cursor}", app.input)).block(pane(title, focused)),
        area,
    );
}

fn render_help(frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_mul(7) / 10;
    let height = area.height.saturating_mul(7) / 10;
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(App::help_text())
            .wrap(Wrap { trim: false })
            .block(pane("Help · Esc or F1 to close".to_owned(), true)),
        popup,
    );
}

/// Greedy word wrap by character count; words longer than `width` are split.
#[must_use]
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut current = String::new();
        let mut length = 0;
        for word in raw.split_whitespace() {
            let mut chars = word.chars().collect::<Vec<_>>();
            while chars.len() > width {
                if length > 0 {
                    lines.push(std::mem::take(&mut current));
                    length = 0;
                }
                lines.push(chars.drain(..width).collect());
            }
            if chars.is_empty() {
                continue;
            }
            if length > 0 && length + 1 + chars.len() > width {
                lines.push(std::mem::take(&mut current));
                length = 0;
            }
            if length > 0 {
                current.push(' ');
                length += 1;
            }
            length += chars.len();
            current.extend(chars);
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Truncates to `width` characters with an ellipsis.
#[must_use]
pub fn truncate(text: &str, width: usize) -> String {
    let single_line = text.replace('\n', " ");
    if single_line.chars().count() <= width {
        return single_line;
    }
    let mut truncated = single_line
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use nexus_teams::{
        BotProfile, Goal, Message, MessageId, MessageKind, MissionId, MissionState, TeamEvent,
        TeamSpec,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::{message_lines, render, truncate, wrap};
    use crate::app::App;

    fn team() -> TeamSpec {
        TeamSpec::new(
            "t",
            vec![
                BotProfile::from_archetype("lead", "lead").expect("lead"),
                BotProfile::from_archetype("ada", "engineer").expect("eng"),
            ],
        )
    }

    fn mission_with(messages: &[(u64, Option<u64>, &str)]) -> MissionState {
        let mut state = MissionState::new(MissionId("m".to_owned()), team(), Goal::new("g"));
        for (id, thread, text) in messages {
            state.apply(&TeamEvent::MessagePosted {
                message: Message {
                    id: MessageId(*id),
                    channel: "general".to_owned(),
                    author: "ada".to_owned(),
                    kind: MessageKind::Chat,
                    text: (*text).to_owned(),
                    thread: thread.map(MessageId),
                    mentions: Vec::new(),
                    round: 1,
                },
            });
        }
        state
    }

    #[test]
    fn wrapping_respects_width_and_splits_long_words() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
        assert_eq!(wrap("", 10), vec![""]);
    }

    #[test]
    fn truncation_adds_ellipsis() {
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("hi", 5), "hi");
    }

    #[test]
    fn thread_replies_follow_their_root() {
        let state = mission_with(&[(1, None, "root"), (2, None, "other"), (3, Some(1), "reply")]);
        let rendered = message_lines(&state, "general", 40)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let root = rendered
            .iter()
            .position(|line| line.contains("root"))
            .expect("root");
        let reply = rendered
            .iter()
            .position(|line| line.contains("reply"))
            .expect("reply");
        let other = rendered
            .iter()
            .position(|line| line.contains("other"))
            .expect("other");
        assert!(root < reply && reply < other, "{rendered:?}");
        assert!(rendered[reply].starts_with("   ↳ "));
    }

    fn frame_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        buffer
            .content()
            .chunks(width)
            .map(|row| {
                row.iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn completed_mission_frame_shows_conversation_board_and_roster() {
        use std::sync::Arc;

        use nexus_teams::{BrainRouter, MemoryEventSink, MissionEngine, SimulatedBrain};

        let team = TeamSpec::new(
            "squad",
            vec![
                BotProfile::from_archetype("lead", "lead").expect("lead"),
                BotProfile::from_archetype("ada", "engineer").expect("eng"),
                BotProfile::from_archetype("rev", "reviewer").expect("rev"),
            ],
        )
        .with_approvers(["rev"]);
        let sink = Arc::new(MemoryEventSink::default());
        MissionEngine::new(
            team.clone(),
            Goal::new("Add a --json flag"),
            BrainRouter::new(Arc::new(SimulatedBrain)),
        )
        .expect("engine")
        .with_sink(sink.clone())
        .run()
        .await
        .expect("mission");
        let mut app = App::new(team);
        for event in sink.events() {
            app.apply(&event);
        }
        let mut terminal = Terminal::new(TestBackend::new(150, 46)).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("frame");
        let text = frame_text(&terminal);
        println!("{text}");
        assert!(text.contains("NEXUS TEAMS"));
        assert!(text.contains("completed"));
        assert!(text.contains("Task board · 2/2 done"));
        assert!(text.contains("@rev"));
        assert!(text.contains("#general"));
    }

    #[test]
    fn full_layout_renders_without_panicking() {
        let backend = TestBackend::new(140, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::new(team());
        terminal
            .draw(|frame| render(frame, &app))
            .expect("idle frame");
        app.apply(&TeamEvent::MissionStarted {
            mission: MissionId("mission-x".to_owned()),
            team: team(),
            goal: Goal::new("Ship the parser"),
        });
        app.show_help = true;
        terminal
            .draw(|frame| render(frame, &app))
            .expect("mission frame");
        let small = TestBackend::new(40, 12);
        let mut tiny = Terminal::new(small).expect("terminal");
        tiny.draw(|frame| render(frame, &app)).expect("tiny frame");
    }
}
