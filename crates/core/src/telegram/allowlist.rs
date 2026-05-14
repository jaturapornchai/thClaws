//! Per-source allowlist. DMs allowed if user_id is listed; groups
//! allowed if chat_id is listed.

use std::collections::HashSet;

use super::types::Chat;

#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    pub users: HashSet<i64>,
    pub chats: HashSet<i64>,
}

impl Allowlist {
    pub fn from_csv(users: &str, chats: &str) -> Self {
        fn parse(s: &str) -> HashSet<i64> {
            s.split(',')
                .map(|t| t.trim())
                .filter(|t| !t.is_empty())
                // Tolerate "telegram:" / "tg:" prefix per OpenClaw convention.
                .map(|t| t.trim_start_matches("telegram:").trim_start_matches("tg:"))
                .filter_map(|t| t.parse::<i64>().ok())
                .collect()
        }
        Self {
            users: parse(users),
            chats: parse(chats),
        }
    }

    /// Open mode = both lists empty. Single-owner / dev bots opt into
    /// this by submitting the setup form without any IDs — the gate
    /// then forwards everything. Use ONLY when the bot's username is
    /// kept private and the owner accepts that anyone who discovers
    /// the bot can drive the agent.
    pub fn is_open(&self) -> bool {
        self.users.is_empty() && self.chats.is_empty()
    }

    /// `from_user_id` is the user who SENT the message (DM = chat id
    /// equals user id; group = chat id is the group id).
    pub fn is_allowed(&self, from_user_id: i64, chat: &Chat) -> bool {
        if self.is_open() {
            return true;
        }
        if chat.is_private() {
            self.users.contains(&from_user_id)
        } else if chat.is_group() {
            self.chats.contains(&chat.id)
        } else {
            // Channels and unknown chat types: deny by default.
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_chat(id: i64) -> Chat {
        Chat {
            id,
            typ: "private".into(),
            username: None,
            title: None,
        }
    }
    fn supergroup(id: i64) -> Chat {
        Chat {
            id,
            typ: "supergroup".into(),
            username: None,
            title: Some("QA".into()),
        }
    }

    #[test]
    fn dm_allowed_when_user_listed() {
        let a = Allowlist::from_csv("42,99", "");
        assert!(a.is_allowed(42, &private_chat(42)));
    }

    #[test]
    fn dm_denied_when_user_missing() {
        let a = Allowlist::from_csv("42", "");
        assert!(!a.is_allowed(100, &private_chat(100)));
    }

    #[test]
    fn group_allowed_when_chat_listed() {
        let a = Allowlist::from_csv("", "-100123");
        assert!(a.is_allowed(42, &supergroup(-100123)));
    }

    #[test]
    fn group_denied_when_chat_missing() {
        let a = Allowlist::from_csv("", "-100999");
        assert!(!a.is_allowed(42, &supergroup(-100123)));
    }

    #[test]
    fn channel_denied_by_default() {
        let chan = Chat {
            id: -200,
            typ: "channel".into(),
            username: None,
            title: None,
        };
        let a = Allowlist::from_csv("", "-200");
        assert!(!a.is_allowed(42, &chan));
    }

    #[test]
    fn tolerates_telegram_prefix() {
        let a = Allowlist::from_csv("telegram:42, tg:99", "");
        assert!(a.is_allowed(42, &private_chat(42)));
        assert!(a.is_allowed(99, &private_chat(99)));
    }

    #[test]
    fn empty_list_is_open_mode_allows_all() {
        let a = Allowlist::default();
        assert!(a.is_open());
        // DM allowed
        assert!(a.is_allowed(42, &private_chat(42)));
        // Group allowed
        assert!(a.is_allowed(42, &supergroup(-100123)));
    }

    #[test]
    fn non_empty_list_disables_open_mode() {
        let a = Allowlist::from_csv("42", "");
        assert!(!a.is_open());
    }
}
