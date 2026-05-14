//! Allowlist + sink dispatch. Shared by `long_poll` and `webhook`
//! transports so the gating policy is identical regardless of how
//! the `Update` arrived.
//!
//! Policy:
//!   1. message + channel_post: gate on (from.id, chat) via allowlist.
//!      No `from` (rare — anonymous group admins) → drop.
//!   2. callback_query: gate on `from.id` only (callbacks don't have
//!      chat-level allowlist semantics; the user already had to be
//!      authorised to receive the keyboard in the first place).
//!   3. Anything else: passthrough — the sink may handle or ignore.

use std::sync::Arc;

use super::allowlist::Allowlist;
use super::dedup::DedupStore;
use super::long_poll::TelegramUpdateSink;
use super::types::Update;

pub async fn route_update(
    allowlist: &Allowlist,
    dedup: &DedupStore,
    sink: Arc<dyn TelegramUpdateSink>,
    update: Update,
) {
    // P3 idempotency: drop a duplicate update_id before we burn an
    // agent turn or fire a callback. Webhook retries + long-poll
    // offset rewinds both land here.
    if !dedup.check_and_record(update.update_id) {
        return;
    }
    if let Some(msg) = update.message.as_ref().or(update.channel_post.as_ref()) {
        let from_id = match &msg.from {
            Some(u) => u.id,
            None => return,
        };
        if !allowlist.is_allowed(from_id, &msg.chat) {
            return;
        }
    } else if let Some(cb) = update.callback_query.as_ref() {
        // Callback gate: sender must appear in the user allowlist
        // (regardless of chat scope — buttons are user-targeted).
        // Explicit open-mode opt-in forwards all callbacks too.
        if !allowlist.is_open() && !allowlist.users.contains(&cb.from.id) {
            return;
        }
    }
    sink.on_update(update).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::types::{CallbackQuery, Chat, Message, User};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Cap {
        seen: Mutex<Vec<i64>>,
    }
    #[async_trait::async_trait]
    impl TelegramUpdateSink for Cap {
        async fn on_update(&self, u: Update) {
            self.seen.lock().unwrap().push(u.update_id);
        }
    }

    fn msg_update(id: i64, from_id: i64, chat_id: i64, chat_type: &str) -> Update {
        Update {
            update_id: id,
            message: Some(Message {
                message_id: 1,
                from: Some(User {
                    id: from_id,
                    is_bot: false,
                    username: None,
                    first_name: None,
                }),
                chat: Chat {
                    id: chat_id,
                    typ: chat_type.into(),
                    username: None,
                    title: None,
                },
                text: Some("hi".into()),
                reply_to_message: None,
                entities: vec![],
            }),
            channel_post: None,
            callback_query: None,
        }
    }

    #[tokio::test]
    async fn allowed_dm_dispatched() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("42", "");
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(1, 42, 42, "private")).await;
        assert_eq!(sink.seen.lock().unwrap().clone(), vec![1]);
    }

    #[tokio::test]
    async fn blocked_dm_dropped() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("42", "");
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(2, 99, 99, "private")).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn group_allowed_when_both_chat_and_user_listed() {
        // P4 (fail-closed group auth): chat allow-listed AND sender
        // allow-listed → dispatched.
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("42", "-100123");
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(3, 42, -100123, "supergroup")).await;
        assert_eq!(sink.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn group_denied_when_chat_listed_but_user_not_listed() {
        // P4 fail-closed: chat is in chats, sender is NOT in users
        // → drop. Previous behaviour forwarded; that was the bug.
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("", "-100123");
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(3, 42, -100123, "supergroup")).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn group_chat_only_mode_skips_user_check_when_flag_set() {
        let sink = Arc::new(Cap::default());
        let al =
            Allowlist::from_csv("", "-100123").with_any_user_in_group(true);
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(31, 42, -100123, "supergroup")).await;
        assert_eq!(sink.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn group_blocked_when_chat_missing() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("", "-100999");
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(4, 42, -100123, "supergroup")).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn anonymous_sender_dropped() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("42", "");
        let mut u = msg_update(5, 42, 42, "private");
        u.message.as_mut().unwrap().from = None;
        route_update(&al, &DedupStore::new(), sink.clone(), u).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn callback_from_listed_user_dispatched() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("42", "");
        let u = Update {
            update_id: 6,
            message: None,
            channel_post: None,
            callback_query: Some(CallbackQuery {
                id: "cb-1".into(),
                from: User {
                    id: 42,
                    is_bot: false,
                    username: None,
                    first_name: None,
                },
                data: Some("tool:allow:x".into()),
                message: None,
            }),
        };
        route_update(&al, &DedupStore::new(), sink.clone(), u).await;
        assert_eq!(sink.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn callback_from_unlisted_user_dropped() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("42", "");
        let u = Update {
            update_id: 7,
            message: None,
            channel_post: None,
            callback_query: Some(CallbackQuery {
                id: "cb-2".into(),
                from: User {
                    id: 99,
                    is_bot: false,
                    username: None,
                    first_name: None,
                },
                data: Some("tool:allow:x".into()),
                message: None,
            }),
        };
        route_update(&al, &DedupStore::new(), sink.clone(), u).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_allowlist_default_drops_p1_fail_closed() {
        // P1: a freshly-constructed (or empty-form-submission)
        // allowlist must deny every update. Open mode is opt-in.
        let sink = Arc::new(Cap::default());
        let al = Allowlist::default();
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(20, 99, 99, "private")).await;
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(21, 100, -1234, "supergroup")).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn open_mode_forwards_any_dm_when_explicit() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::default().with_open_mode(true);
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(10, 99, 99, "private")).await;
        route_update(&al, &DedupStore::new(), sink.clone(), msg_update(11, 100, -1234, "supergroup")).await;
        assert_eq!(sink.seen.lock().unwrap().clone(), vec![10, 11]);
    }

    #[tokio::test]
    async fn open_mode_forwards_any_callback_when_explicit() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::default().with_open_mode(true);
        let u = Update {
            update_id: 12,
            message: None,
            channel_post: None,
            callback_query: Some(CallbackQuery {
                id: "cb-open".into(),
                from: User {
                    id: 999,
                    is_bot: false,
                    username: None,
                    first_name: None,
                },
                data: Some("tool:allow:x".into()),
                message: None,
            }),
        };
        route_update(&al, &DedupStore::new(), sink.clone(), u).await;
        assert_eq!(sink.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn duplicate_update_id_dispatched_once_p3() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("42", "");
        let dedup = DedupStore::new();
        // Same update_id arrives twice (webhook retry).
        route_update(&al, &dedup, sink.clone(), msg_update(99, 42, 42, "private")).await;
        route_update(&al, &dedup, sink.clone(), msg_update(99, 42, 42, "private")).await;
        assert_eq!(sink.seen.lock().unwrap().clone(), vec![99]);
    }

    #[tokio::test]
    async fn empty_allowlist_default_drops_callbacks_p1_fail_closed() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::default();
        let u = Update {
            update_id: 22,
            message: None,
            channel_post: None,
            callback_query: Some(CallbackQuery {
                id: "cb-x".into(),
                from: User {
                    id: 1,
                    is_bot: false,
                    username: None,
                    first_name: None,
                },
                data: Some("tool:allow:x".into()),
                message: None,
            }),
        };
        route_update(&al, &DedupStore::new(), sink.clone(), u).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }
}
