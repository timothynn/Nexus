//! Operator console grammar shared by every interface (CLI, TUI, embedders).

use crate::{ControlInput, conversation::GENERAL};

/// A parsed operator console line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorCommand {
    /// Input for the running mission.
    Control(ControlInput),
    /// Start a new mission toward this goal (interfaces that support it).
    Goal(String),
    Cancel,
    Help,
    Invalid(String),
}

pub const OPERATOR_HELP: &str = "operator commands:
  <text>              post as @human to the current channel (use @handle to mention bots)
  #channel <text>     post to a specific channel
  /dm <bot> <text>    private message to one bot
  /goal <text>        start a mission toward a goal (where supported)
  /answer <text>      answer a bot's pending question
  /approve            approve a pending completion proposal
  /reject <reason>    reject a pending completion proposal
  /pause | /resume    pause or resume the mission
  /cancel             cancel the mission
  /help               show this help";

/// Parses one console line; plain text posts to `default_channel`.
#[must_use]
pub fn parse_operator_line(line: &str, default_channel: &str) -> Option<OperatorCommand> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if let Some(rest) = line.strip_prefix('/') {
        let (command, argument) = rest
            .split_once(' ')
            .map_or((rest, ""), |(command, argument)| (command, argument.trim()));
        return Some(match command {
            "approve" => OperatorCommand::Control(ControlInput::Approve),
            "reject" => OperatorCommand::Control(ControlInput::Reject(if argument.is_empty() {
                "rejected by operator".to_owned()
            } else {
                argument.to_owned()
            })),
            "pause" => OperatorCommand::Control(ControlInput::Pause),
            "resume" => OperatorCommand::Control(ControlInput::Resume),
            "answer" if !argument.is_empty() => {
                OperatorCommand::Control(ControlInput::Answer(argument.to_owned()))
            }
            "goal" if !argument.is_empty() => OperatorCommand::Goal(argument.to_owned()),
            "dm" => match argument.split_once(' ') {
                Some((to, text)) if !text.trim().is_empty() => {
                    OperatorCommand::Control(ControlInput::Direct {
                        to: to.trim_start_matches('@').to_owned(),
                        text: text.trim().to_owned(),
                    })
                }
                _ => OperatorCommand::Invalid("usage: /dm <bot> <text>".to_owned()),
            },
            "answer" | "goal" => OperatorCommand::Invalid(format!("usage: /{command} <text>")),
            "cancel" | "quit" => OperatorCommand::Cancel,
            "help" => OperatorCommand::Help,
            other => OperatorCommand::Invalid(format!("unknown command /{other}")),
        });
    }
    if let Some(rest) = line.strip_prefix('#') {
        let (channel, text) = rest.split_once(' ').unwrap_or((rest, ""));
        if text.trim().is_empty() {
            return Some(OperatorCommand::Invalid(
                "usage: #channel <text>".to_owned(),
            ));
        }
        return Some(post(channel, text.trim()));
    }
    let channel = if default_channel.is_empty() {
        GENERAL
    } else {
        default_channel
    };
    Some(post(channel, line))
}

fn post(channel: &str, text: &str) -> OperatorCommand {
    OperatorCommand::Control(ControlInput::Post {
        channel: channel.to_owned(),
        text: text.to_owned(),
        thread: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{OperatorCommand, parse_operator_line};
    use crate::ControlInput;

    #[test]
    fn slash_commands_parse() {
        assert_eq!(parse_operator_line("  ", "general"), None);
        assert_eq!(
            parse_operator_line("/approve", "general"),
            Some(OperatorCommand::Control(ControlInput::Approve))
        );
        assert_eq!(
            parse_operator_line("/reject needs tests", "general"),
            Some(OperatorCommand::Control(ControlInput::Reject(
                "needs tests".to_owned()
            )))
        );
        assert_eq!(
            parse_operator_line("/goal Ship v2", "general"),
            Some(OperatorCommand::Goal("Ship v2".to_owned()))
        );
        assert!(matches!(
            parse_operator_line("/goal", "general"),
            Some(OperatorCommand::Invalid(_))
        ));
        assert_eq!(
            parse_operator_line("/cancel", "general"),
            Some(OperatorCommand::Cancel)
        );
        assert_eq!(
            parse_operator_line("/dm @ada can you pair on T2?", "general"),
            Some(OperatorCommand::Control(ControlInput::Direct {
                to: "ada".to_owned(),
                text: "can you pair on T2?".to_owned()
            }))
        );
        assert!(matches!(
            parse_operator_line("/dm ada", "general"),
            Some(OperatorCommand::Invalid(_))
        ));
        assert!(matches!(
            parse_operator_line("/dance", "general"),
            Some(OperatorCommand::Invalid(_))
        ));
    }

    #[test]
    fn posts_target_explicit_or_current_channel() {
        assert_eq!(
            parse_operator_line("#design @ada mock it up", "general"),
            Some(OperatorCommand::Control(ControlInput::Post {
                channel: "design".to_owned(),
                text: "@ada mock it up".to_owned(),
                thread: None
            }))
        );
        assert!(matches!(
            parse_operator_line("hello @lead", "design"),
            Some(OperatorCommand::Control(ControlInput::Post { channel, .. })) if channel == "design"
        ));
        assert!(matches!(
            parse_operator_line("#design", "general"),
            Some(OperatorCommand::Invalid(_))
        ));
    }
}
