//! Per-source allowlist. Fail-closed by default — an empty allowlist
//! denies every inbound update. To intentionally accept any sender,
//! the operator must opt into `allow_open_mode = true` (deve / single
//! owner bots whose username is kept private).
//!
//! Group authorisation requires BOTH the chat_id AND the sender's
//! user_id to be listed by default. Use `allow_any_user_in_group =
//! true` to revert to chat-only gating (any member of an allow-listed
//! group can drive the agent).

use std::collections::HashSet;

use super::types::Chat;

#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    pub users: HashSet<i64>,
    pub chats: HashSet<i64>,
    /// Explicit opt-in: empty allowlist = forward every update.
    /// Default `false` — empty = deny all.
    pub allow_open_mode: bool,
    /// Explicit opt-in: in groups, sender doesn't have to be in
    /// `users`. Default `false` — both chat_id AND user_id must
    /// be allow-listed.
    pub allow_any_user_in_group: bool,
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
            allow_open_mode: false,
            allow_any_user_in_group: false,
        }
    }

    pub fn with_open_mode(mut self, on: bool) -> Self {
        self.allow_open_mode = on;
        self
    }

    pub fn with_any_user_in_group(mut self, on: bool) -> Self {
        self.allow_any_user_in_group = on;
        self
    }

    /// True iff the operator explicitly enabled open mode AND both
    /// lists are empty. Open mode with non-empty lists is treated
    /// as a misconfiguration and ignored (lists win).
    pub fn is_open(&self) -> bool {
        self.allow_open_mode && self.users.is_empty() && self.chats.is_empty()
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
            // Default: BOTH chat_id AND user_id must be listed.
            // Opt-in `allow_any_user_in_group` skips the user check
            // (chat membership is the only gate).
            if !self.chats.contains(&chat.id) {
                return false;
            }
            if self.allow_any_user_in_group {
                return true;
            }
            self.users.contains(&from_user_id)
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
    fn group_requires_both_chat_and_user_by_default() {
        // chat allow-listed, user NOT listed → denied (P4 fail-closed)
        let a = Allowlist::from_csv("", "-100123");
        assert!(!a.is_allowed(42, &supergroup(-100123)));
    }

    #[test]
    fn group_allowed_when_both_chat_and_user_listed() {
        let a = Allowlist::from_csv("42", "-100123");
        assert!(a.is_allowed(42, &supergroup(-100123)));
    }

    #[test]
    fn group_with_any_user_flag_skips_user_check() {
        let a = Allowlist::from_csv("", "-100123").with_any_user_in_group(true);
        assert!(a.is_allowed(42, &supergroup(-100123)));
        // But missing chat is still denied.
        assert!(!a.is_allowed(42, &supergroup(-999)));
    }

    #[test]
    fn group_denied_when_chat_missing() {
        let a = Allowlist::from_csv("42", "-100999");
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
    fn empty_list_default_denies_all_p1_fail_closed() {
        let a = Allowlist::default();
        assert!(!a.is_open(), "no explicit opt-in → not open");
        assert!(
            !a.is_allowed(42, &private_chat(42)),
            "fail-closed: empty + no opt-in = deny"
        );
        assert!(!a.is_allowed(42, &supergroup(-100123)));
    }

    #[test]
    fn open_mode_requires_explicit_opt_in() {
        let a = Allowlist::default().with_open_mode(true);
        assert!(a.is_open());
        assert!(a.is_allowed(42, &private_chat(42)));
        assert!(a.is_allowed(42, &supergroup(-100123)));
    }

    #[test]
    fn open_mode_ignored_when_lists_non_empty() {
        // open_mode is opt-in for the "intentionally empty" case;
        // a non-empty allowlist means the operator wants gating, so
        // we don't quietly forward unlisted IDs even with the flag.
        let a = Allowlist::from_csv("42", "").with_open_mode(true);
        assert!(!a.is_open());
        assert!(a.is_allowed(42, &private_chat(42)));
        assert!(!a.is_allowed(99, &private_chat(99)));
    }

    #[test]
    fn non_empty_list_disables_open_mode_flag() {
        let a = Allowlist::from_csv("42", "");
        assert!(!a.is_open());
    }
}
