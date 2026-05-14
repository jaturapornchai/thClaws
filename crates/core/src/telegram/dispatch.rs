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
use super::long_poll::TelegramUpdateSink;
use super::types::Update;

pub async fn route_update(
    allowlist: &Allowlist,
    sink: Arc<dyn TelegramUpdateSink>,
    update: Update,
) {
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
        // Open mode (both lists empty) → forward all callbacks too.
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
        route_update(&al, sink.clone(), msg_update(1, 42, 42, "private")).await;
        assert_eq!(sink.seen.lock().unwrap().clone(), vec![1]);
    }

    #[tokio::test]
    async fn blocked_dm_dropped() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("42", "");
        route_update(&al, sink.clone(), msg_update(2, 99, 99, "private")).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn group_allowed_when_chat_listed() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("", "-100123");
        route_update(&al, sink.clone(), msg_update(3, 42, -100123, "supergroup")).await;
        assert_eq!(sink.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn group_blocked_when_chat_missing() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("", "-100999");
        route_update(&al, sink.clone(), msg_update(4, 42, -100123, "supergroup")).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn anonymous_sender_dropped() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::from_csv("42", "");
        let mut u = msg_update(5, 42, 42, "private");
        u.message.as_mut().unwrap().from = None;
        route_update(&al, sink.clone(), u).await;
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
        route_update(&al, sink.clone(), u).await;
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
        route_update(&al, sink.clone(), u).await;
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn open_mode_forwards_any_dm() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::default(); // empty = open mode
        route_update(&al, sink.clone(), msg_update(10, 99, 99, "private")).await;
        route_update(&al, sink.clone(), msg_update(11, 100, -1234, "supergroup")).await;
        assert_eq!(sink.seen.lock().unwrap().clone(), vec![10, 11]);
    }

    #[tokio::test]
    async fn open_mode_forwards_any_callback() {
        let sink = Arc::new(Cap::default());
        let al = Allowlist::default(); // empty = open mode
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
        route_update(&al, sink.clone(), u).await;
        assert_eq!(sink.seen.lock().unwrap().len(), 1);
    }
}
