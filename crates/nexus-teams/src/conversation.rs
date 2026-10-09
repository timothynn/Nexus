//! Channels, messages, threads, and mentions.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The channel every member belongs to.
pub const GENERAL: &str = "general";

/// Monotonic message identifier within a mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(pub u64);

impl fmt::Display for MessageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "m{}", self.0)
    }
}

/// What a message represents, so interfaces can render it distinctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Ordinary channel conversation.
    Chat,
    /// Private message in a direct channel.
    Direct,
    /// Engine-authored notice.
    System,
    /// A task board change narrated into the channel.
    TaskUpdate,
    /// A question escalated to the human operator.
    Question,
    /// A proposal to finish the mission.
    Proposal,
    /// An approve/reject vote on a proposal.
    Vote,
    /// A handoff passing the floor to another member.
    Handoff,
}

/// A message posted to a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub channel: String,
    pub author: String,
    pub kind: MessageKind,
    pub text: String,
    /// Root message of the thread this message replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<MessageId>,
    /// Handles mentioned with `@handle`, in order of first appearance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
    /// Mission round in which the message was posted.
    pub round: u32,
}

impl Message {
    #[must_use]
    pub fn mentions(&self, handle: &str) -> bool {
        self.mentions.iter().any(|mention| mention == handle)
            || self
                .mentions
                .iter()
                .any(|mention| mention == "all" || mention == "team")
    }
}

/// A conversation space within a team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    #[serde(default)]
    pub purpose: String,
    /// Members allowed in the channel; empty means everyone.
    #[serde(default)]
    pub members: Vec<String>,
    /// Whether this is a private two-person channel.
    #[serde(default)]
    pub direct: bool,
}

impl Channel {
    #[must_use]
    pub fn open(name: impl Into<String>, purpose: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            purpose: purpose.into(),
            members: Vec::new(),
            direct: false,
        }
    }

    /// The private channel between two members, named deterministically.
    #[must_use]
    pub fn direct(first: &str, second: &str) -> Self {
        let mut pair = [first.to_owned(), second.to_owned()];
        pair.sort();
        Self {
            name: direct_channel_name(&pair[0], &pair[1]),
            purpose: format!("Direct messages between @{} and @{}", pair[0], pair[1]),
            members: pair.to_vec(),
            direct: true,
        }
    }

    #[must_use]
    pub fn includes(&self, handle: &str) -> bool {
        self.members.is_empty() || self.members.iter().any(|member| member == handle)
    }
}

#[must_use]
pub fn direct_channel_name(first: &str, second: &str) -> String {
    let (low, high) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };
    format!("dm:{low}+{high}")
}

/// Channel names follow handle rules, except direct channels which are engine-named.
#[must_use]
pub fn valid_channel_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

/// Extracts `@handle` mentions in order of first appearance.
///
/// A mention starts at `@` that is not preceded by a word character (so email
/// addresses are ignored) and continues over handle characters. Trailing `-`
/// and `_` are trimmed so `@coder—` and `@coder-,` both resolve to `coder`.
#[must_use]
pub fn parse_mentions(text: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    let mut previous: Option<char> = None;
    let mut chars = text.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        let preceded_by_word = previous.is_some_and(|p| p.is_alphanumeric() || p == '_');
        previous = Some(character);
        if character != '@' || preceded_by_word {
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while let Some(&(next_index, next)) = chars.peek() {
            if next.is_ascii_alphanumeric() || matches!(next, '-' | '_') {
                end = next_index + next.len_utf8();
                previous = Some(next);
                chars.next();
            } else {
                break;
            }
        }
        let handle = text[start..end]
            .trim_end_matches(['-', '_'])
            .to_ascii_lowercase();
        if !handle.is_empty()
            && handle.starts_with(|c: char| c.is_ascii_lowercase())
            && !mentions.contains(&handle)
        {
            mentions.push(handle);
        }
    }
    mentions
}

#[cfg(test)]
mod tests {
    use super::{Channel, direct_channel_name, parse_mentions, valid_channel_name};

    #[test]
    fn mentions_are_ordered_and_deduplicated() {
        assert_eq!(
            parse_mentions("@coder please sync with @reviewer, then ping @coder again"),
            vec!["coder", "reviewer"]
        );
    }

    #[test]
    fn emails_are_not_mentions() {
        assert!(parse_mentions("mail ops@example.com").is_empty());
    }

    #[test]
    fn mention_punctuation_is_trimmed() {
        assert_eq!(
            parse_mentions("thanks @qa-, and (@lead)."),
            vec!["qa", "lead"]
        );
    }

    #[test]
    fn mentions_are_case_insensitive() {
        assert_eq!(parse_mentions("@Lead"), vec!["lead"]);
    }

    #[test]
    fn direct_channels_are_symmetric_and_private() {
        let channel = Channel::direct("zed", "amy");
        assert_eq!(channel.name, direct_channel_name("amy", "zed"));
        assert_eq!(channel.name, "dm:amy+zed");
        assert!(channel.includes("amy"));
        assert!(!channel.includes("bob"));
    }

    #[test]
    fn open_channels_include_everyone() {
        assert!(Channel::open("general", "").includes("anyone"));
    }

    #[test]
    fn channel_names_are_validated() {
        assert!(valid_channel_name("design-review"));
        assert!(!valid_channel_name("Design"));
        assert!(!valid_channel_name("dm:a+b"));
    }
}
