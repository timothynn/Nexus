//! Teams-style chat feed rendering for mission events.

use std::collections::BTreeMap;

use nexus_teams::{MessageKind, TeamEvent};

const PALETTE: [&str; 6] = ["36", "32", "35", "33", "34", "91"];

/// Formats mission events as a Teams-style chat feed.
pub struct ChatPrinter {
    color: bool,
    verbose: bool,
    palette: BTreeMap<String, &'static str>,
}

impl ChatPrinter {
    #[must_use]
    pub fn new(color: bool, verbose: bool) -> Self {
        Self {
            color: color && std::env::var_os("NO_COLOR").is_none(),
            verbose,
            palette: BTreeMap::new(),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    fn author(&mut self, handle: &str) -> String {
        let code = match handle {
            "system" => "2",
            "human" => "1;97",
            _ => {
                let next = PALETTE[self.palette.len() % PALETTE.len()];
                self.palette.entry(handle.to_owned()).or_insert(next)
            }
        };
        self.paint(code, &format!("@{handle}"))
    }

    #[must_use]
    pub fn format(&mut self, event: &TeamEvent) -> Option<String> {
        match event {
            TeamEvent::MissionStarted { mission, team, .. } => Some(self.paint(
                "1",
                &format!(
                    "◈ {} · team {} · {} bots",
                    mission,
                    team.name,
                    team.members.len()
                ),
            )),
            TeamEvent::MissionResumed { budget, .. } => Some(self.paint(
                "1",
                &format!(
                    "↻ mission resumed · budget now {} rounds",
                    budget.max_rounds
                ),
            )),
            TeamEvent::RoundStarted { round } => {
                Some(self.paint("2", &format!("── round {round} ──")))
            }
            TeamEvent::MessagePosted { message } => {
                let badge = match message.kind {
                    MessageKind::TaskUpdate => "📋 ",
                    MessageKind::Question => "❓ ",
                    MessageKind::Proposal => "🏁 ",
                    MessageKind::Vote => "🗳  ",
                    MessageKind::Handoff => "🤝 ",
                    MessageKind::Direct => "✉  ",
                    MessageKind::Chat | MessageKind::System => "",
                };
                let indent = if message.thread.is_some() {
                    "    ↳ "
                } else {
                    ""
                };
                let channel = self.paint("2", &format!("#{:<10}", message.channel));
                let author = self.author(&message.author);
                let text = if message.kind == MessageKind::System {
                    self.paint("2", &message.text)
                } else {
                    message.text.clone()
                };
                Some(format!("{indent}{channel} {author} {badge}{text}"))
            }
            TeamEvent::TurnStarted { bot, reason, .. } if self.verbose => {
                let author = self.author(bot);
                Some(self.paint("2", &format!("  … {author} has the floor: {reason}")))
            }
            TeamEvent::BotActivity { bot, kind, detail } if self.verbose => {
                Some(self.paint("2", &format!("  ⚙ @{bot} {kind} {detail}")))
            }
            TeamEvent::ActionDenied {
                bot,
                action,
                reason,
            } => Some(self.paint("33", &format!("  ⚠ @{bot} {action} denied: {reason}"))),
            TeamEvent::TurnFailed { bot, error, .. } => {
                Some(self.paint("31", &format!("  ✖ @{bot} turn failed: {error}")))
            }
            TeamEvent::StatusChanged { status, reason } => {
                Some(self.paint("1", &format!("● {status}: {reason}")))
            }
            TeamEvent::HumanQuestion { bot, question } => Some(self.paint(
                "1;33",
                &format!("? @{bot} asks @human: {question}   (reply with /answer <text>)"),
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use nexus_teams::{Budget, TeamEvent};

    use super::ChatPrinter;

    #[test]
    fn activity_is_hidden_unless_verbose() {
        let event = TeamEvent::BotActivity {
            bot: "ada".to_owned(),
            kind: "tool".to_owned(),
            detail: "x".to_owned(),
        };
        assert!(ChatPrinter::new(false, false).format(&event).is_none());
        assert!(ChatPrinter::new(false, true).format(&event).is_some());
    }

    #[test]
    fn rounds_and_resumes_are_announced() {
        let mut printer = ChatPrinter::new(false, false);
        assert_eq!(
            printer
                .format(&TeamEvent::RoundStarted { round: 2 })
                .as_deref(),
            Some("── round 2 ──")
        );
        let resumed = printer.format(&TeamEvent::MissionResumed {
            budget: Budget::default(),
            note: None,
        });
        assert!(resumed.is_some_and(|line| line.contains("resumed")));
    }
}
