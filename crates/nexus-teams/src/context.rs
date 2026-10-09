//! Renders a bot's view of the mission into model prompts within a token budget.

use std::fmt::Write as _;

use nexus_context::estimate_tokens;

use crate::{
    Message, MissionState, TurnContext, conversation::MessageKind, protocol::protocol_guide,
};

/// Returns `(system, user)` prompts for a turn.
///
/// The system prompt holds the persona, team charter, and protocol; the user
/// prompt holds the situation: goal, board, notes, and the newest history that
/// fits in `history_token_budget`.
#[must_use]
pub fn render_turn_prompt(context: &TurnContext, history_token_budget: usize) -> (String, String) {
    (
        render_system(context),
        render_situation(context, history_token_budget),
    )
}

fn render_system(context: &TurnContext) -> String {
    let bot = &context.bot;
    let state = &context.state;
    let mut system = format!(
        "You are {} (@{}), {} on the team \"{}\".\n",
        bot.name,
        bot.handle,
        if bot.role.is_empty() {
            "a member"
        } else {
            &bot.role
        },
        state.team.name
    );
    if !bot.instructions.trim().is_empty() {
        let _ = writeln!(system, "\n## Your persona\n{}", bot.instructions.trim());
    }
    if !bot.expertise.is_empty() {
        let _ = writeln!(system, "Expertise: {}", bot.expertise.join(", "));
    }
    if !state.team.description.trim().is_empty() {
        let _ = writeln!(
            system,
            "\n## Team charter\n{}",
            state.team.description.trim()
        );
    }
    let _ = writeln!(
        system,
        "\n## Team rules\n- The lead is @{}. {}\n- Completion approval: {}.\n- Work only within your role; hand off when someone else is better placed.\n- The human operator is @human; ask them only when truly blocked.",
        state.team.lead_handle(),
        if context.is_lead() {
            "That is you: plan, assign, integrate, and propose completion."
        } else {
            "Report progress and results to the lead."
        },
        state.team.approval_summary()
    );
    if !bot.tools.is_empty() {
        let _ = writeln!(
            system,
            "\nYou can call workspace tools ({}) before replying; tool results are private until you report them.",
            bot.tools.join(", ")
        );
    }
    system.push('\n');
    system.push_str(&protocol_guide());
    system
}

fn render_situation(context: &TurnContext, history_token_budget: usize) -> String {
    let state = &context.state;
    let me = &context.bot.handle;
    let mut user = format!("# Mission goal\n{}\n", state.goal.render());
    let _ = writeln!(
        user,
        "\nRound {} of at most {}. Status: {}.",
        state.round, state.team.budget.max_rounds, state.status
    );
    let _ = writeln!(user, "\n# Team");
    for member in &state.team.members {
        let marker = if &member.handle == me { " ← you" } else { "" };
        let _ = writeln!(user, "- {}{marker}", member.label());
    }
    let _ = writeln!(user, "\n# Channels you can see");
    for channel in state.channels.iter().filter(|channel| channel.includes(me)) {
        let _ = writeln!(user, "- #{} — {}", channel.name, channel.purpose);
    }
    let _ = writeln!(user, "\n# Task board\n{}", state.board.render());
    if let Some(notes) = state.notes.get(me) {
        let _ = writeln!(user, "\n# Your private notes");
        for note in notes.iter().rev().take(10).rev() {
            let _ = writeln!(user, "- {note}");
        }
    }
    if let Some(proposal) = &state.proposal {
        let _ = writeln!(
            user,
            "\n# Pending completion proposal by @{}\n{}\nVotes so far: {}",
            proposal.by,
            proposal.summary,
            if proposal.votes.is_empty() {
                "none".to_owned()
            } else {
                proposal
                    .votes
                    .iter()
                    .map(|(voter, vote)| {
                        format!(
                            "@{voter} {}",
                            if vote.approve { "approve" } else { "reject" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        );
    }
    let _ = writeln!(user, "\n# Conversation (newest last)");
    user.push_str(&render_history(state, me, history_token_budget));
    let unseen = state.unseen_by(me).len();
    let _ = writeln!(
        user,
        "\n# Your turn\nYou have the floor because: {}.\n{unseen} new message(s) since your last turn. Your plain reply goes to #{}.\nRespond now as @{me}.",
        context.reason, context.channel
    );
    user
}

/// Renders the newest visible messages that fit within the token budget.
#[must_use]
pub fn render_history(state: &MissionState, handle: &str, token_budget: usize) -> String {
    let mut lines = Vec::new();
    let mut used = 0;
    let visible = state.visible_to(handle).collect::<Vec<_>>();
    let mut omitted = 0;
    for message in visible.iter().rev() {
        let line = render_message(message);
        let cost = estimate_tokens(&line);
        if used + cost > token_budget {
            omitted = visible.len() - lines.len();
            break;
        }
        used += cost;
        lines.push(line);
    }
    lines.reverse();
    let mut rendered = String::new();
    if omitted > 0 {
        let _ = writeln!(rendered, "({omitted} older message(s) omitted)");
    }
    if lines.is_empty() && omitted == 0 {
        rendered.push_str("(no messages yet)\n");
    }
    for line in lines {
        rendered.push_str(&line);
        rendered.push('\n');
    }
    rendered
}

/// One-line chat rendering: `[m4 #general] @ada: text`.
#[must_use]
pub fn render_message(message: &Message) -> String {
    let thread = message
        .thread
        .map(|root| format!(" ↳{root}"))
        .unwrap_or_default();
    let tag = match message.kind {
        MessageKind::Chat | MessageKind::Direct => "",
        MessageKind::System => " (system)",
        MessageKind::TaskUpdate => " (task)",
        MessageKind::Question => " (question for human)",
        MessageKind::Proposal => " (completion proposal)",
        MessageKind::Vote => " (vote)",
        MessageKind::Handoff => " (handoff)",
    };
    format!(
        "[{} #{}{thread}] @{}{tag}: {}",
        message.id, message.channel, message.author, message.text
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{render_history, render_turn_prompt};
    use crate::{
        ActivityReporter, BotProfile, Goal, Message, MessageId, MissionId, MissionState, TeamEvent,
        TeamSpec, TurnContext, conversation::MessageKind,
    };

    fn state_with_messages(count: u64) -> MissionState {
        let mut state = MissionState::new(
            MissionId("m".to_owned()),
            TeamSpec::new(
                "core",
                vec![
                    BotProfile::from_archetype("lead", "lead").expect("lead"),
                    BotProfile::from_archetype("ada", "engineer").expect("eng"),
                ],
            ),
            Goal::new("Ship the parser").with_criteria(["tests pass"]),
        );
        for id in 1..=count {
            state.apply(&TeamEvent::MessagePosted {
                message: Message {
                    id: MessageId(id),
                    channel: "general".to_owned(),
                    author: "lead".to_owned(),
                    kind: MessageKind::Chat,
                    text: format!("message number {id} with some words in it"),
                    thread: None,
                    mentions: Vec::new(),
                    round: 1,
                },
            });
        }
        state
    }

    #[test]
    fn history_keeps_newest_messages_within_budget() {
        let state = state_with_messages(50);
        let rendered = render_history(&state, "ada", 60);
        assert!(rendered.contains("message number 50"));
        assert!(!rendered.contains("message number 1 "));
        assert!(rendered.starts_with('('));
    }

    #[test]
    fn prompts_contain_persona_goal_and_protocol() {
        let context = TurnContext {
            bot: BotProfile::from_archetype("ada", "engineer").expect("eng"),
            reason: "assigned T1".to_owned(),
            channel: "general".to_owned(),
            state: Arc::new(state_with_messages(2)),
            activity: ActivityReporter::disabled("ada"),
        };
        let (system, user) = render_turn_prompt(&context, 1_000);
        assert!(system.contains("@ada"));
        assert!(system.contains("nexus-actions"));
        assert!(system.contains("The lead is @lead"));
        assert!(user.contains("Ship the parser"));
        assert!(user.contains("- tests pass"));
        assert!(user.contains("assigned T1"));
        assert!(user.contains("← you"));
    }
}
