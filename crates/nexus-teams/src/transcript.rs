//! Human-readable renderings of missions: Markdown transcripts and reports.

use std::fmt::Write as _;

use crate::{MissionReport, MissionState, conversation::MessageKind};

/// Renders a full mission as Markdown, grouped by channel with threads indented.
#[must_use]
pub fn render_markdown(state: &MissionState) -> String {
    let mut out = format!("# Mission: {}\n\n", state.goal.statement);
    let _ = writeln!(out, "- **Mission:** `{}`", state.id);
    let _ = writeln!(
        out,
        "- **Team:** {} (lead @{}, policy {})",
        state.team.name,
        state.team.lead_handle(),
        state.team.policy
    );
    let _ = writeln!(
        out,
        "- **Status:** {} — {}",
        state.status, state.status_reason
    );
    let _ = writeln!(
        out,
        "- **Rounds/turns/messages:** {}/{}/{}",
        state.round,
        state.turns,
        state.messages.len()
    );
    let _ = writeln!(
        out,
        "- **Tokens:** {} in / {} out\n",
        state.usage.input_tokens, state.usage.output_tokens
    );
    if !state.goal.success_criteria.is_empty() {
        let _ = writeln!(out, "## Success criteria\n");
        for criterion in &state.goal.success_criteria {
            let _ = writeln!(out, "- {criterion}");
        }
        out.push('\n');
    }
    let _ = writeln!(out, "## Team\n");
    for member in &state.team.members {
        let member_stats = state.stats.get(&member.handle).cloned().unwrap_or_default();
        let _ = writeln!(
            out,
            "- {} — {} turns, {} messages, {} tokens",
            member.label(),
            member_stats.turns,
            member_stats.messages,
            member_stats.usage.total()
        );
    }
    let _ = writeln!(
        out,
        "\n## Task board\n\n```\n{}\n```\n",
        state.board.render()
    );
    for channel in &state.channels {
        let messages = state
            .messages
            .iter()
            .filter(|message| message.channel == channel.name)
            .collect::<Vec<_>>();
        if messages.is_empty() {
            continue;
        }
        let _ = writeln!(out, "## #{}\n", channel.name);
        if !channel.purpose.is_empty() {
            let _ = writeln!(out, "_{}_\n", channel.purpose);
        }
        for message in messages.iter().filter(|message| message.thread.is_none()) {
            let _ = writeln!(out, "{}", markdown_line(message, ""));
            for reply in messages
                .iter()
                .filter(|reply| reply.thread == Some(message.id))
            {
                let _ = writeln!(out, "{}", markdown_line(reply, "  "));
            }
        }
        out.push('\n');
    }
    if let Some(summary) = &state.accepted_summary {
        let _ = writeln!(out, "## Outcome\n\n{summary}");
    }
    out
}

fn markdown_line(message: &crate::Message, indent: &str) -> String {
    let badge = match message.kind {
        MessageKind::System => "⚙️ ",
        MessageKind::TaskUpdate => "📋 ",
        MessageKind::Question => "❓ ",
        MessageKind::Proposal => "🏁 ",
        MessageKind::Vote => "🗳️ ",
        MessageKind::Handoff => "🤝 ",
        MessageKind::Direct => "✉️ ",
        MessageKind::Chat => "",
    };
    format!(
        "{indent}- **@{}** {badge}{} _(r{}, {})_",
        message.author,
        message.text.replace('\n', " "),
        message.round,
        message.id
    )
}

/// Plain-text report for terminals.
#[must_use]
pub fn render_report(report: &MissionReport) -> String {
    let mut out = format!(
        "Mission {} — {} ({})\n",
        report.mission, report.status, report.reason
    );
    let _ = writeln!(out, "Team: {}  Goal: {}", report.team, report.goal);
    let _ = writeln!(
        out,
        "Rounds {}  Turns {}  Messages {}  Tasks {}/{} done  Tokens {} in / {} out",
        report.rounds,
        report.turns,
        report.messages,
        report.tasks_done,
        report.tasks_total,
        report.usage.input_tokens,
        report.usage.output_tokens
    );
    for (handle, stats) in &report.members {
        let _ = writeln!(
            out,
            "  @{handle:<12} turns {:>3}  msgs {:>3}  actions {:>3}  denied {:>2}  failures {:>2}  tokens {:>7}",
            stats.turns,
            stats.messages,
            stats.actions,
            stats.denied,
            stats.failures,
            stats.usage.total()
        );
    }
    if let Some(summary) = &report.summary {
        let _ = writeln!(out, "Outcome: {summary}");
    }
    out
}
