//! Evidence chat: answers a surgeon's or patient's questions from one run's
//! stored evidence. Server-only; off unless `CHAT=on` (and `MEMORY=on`).

pub mod checks;
pub mod comparison;
pub mod job;
pub mod lines;
pub mod passages;
pub mod retrieve;
pub mod settings;
pub mod store;

/// Who the reply is written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatMode {
    Clinical,
    Plain,
}

impl ChatMode {
    /// The value stored in `chat_conversations.mode` and `chat_turns.mode`.
    pub fn as_str(self) -> &'static str {
        match self {
            ChatMode::Clinical => "clinical",
            ChatMode::Plain => "plain",
        }
    }

    /// Exact lowercase match only.
    pub fn parse(s: &str) -> Option<ChatMode> {
        match s {
            "clinical" => Some(ChatMode::Clinical),
            "plain" => Some(ChatMode::Plain),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_round_trips() {
        for m in [ChatMode::Clinical, ChatMode::Plain] {
            assert_eq!(ChatMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(ChatMode::parse("Clinical"), None);
        assert_eq!(ChatMode::parse(""), None);
    }
}
