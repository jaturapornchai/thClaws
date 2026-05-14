//! Bridge inbound Telegram updates into the worker's `ShellInput`
//! channel. cfg=gui — `shared_session` is gui-gated. CLI builds get
//! the long-poll + webhook transport surface but not the forwarder.
//!
//! Mention gating (groups):
//!   - `requireMention=true` (default): drop unless the message
//!     `@mention`s the bot OR is a direct reply to a bot message.
//!     If `getMe`/`bot_username` is unknown, FAIL CLOSED — drop
//!     the message (P5). The operator who needs mention-required
//!     groups to work after a `getMe` outage can either flip
//!     `require_mention_in_groups=false` or wait for the cached
//!     username to come back.
//!   - `requireMention=false`: forward every allowed group message.
//!
//! DMs are unaffected by mention gating.

use std::sync::mpsc;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::oneshot;

use super::approver::TelegramApprover;
use super::client::TelegramClient;
use super::long_poll::TelegramUpdateSink;
use super::types::{Message, MessageEntity, Update};
use crate::shared_session::ShellInput;

pub struct TelegramSink {
    pub input_tx: mpsc::Sender<ShellInput>,
    pub client: Arc<TelegramClient>,
    /// `getMe` result, lower-cased. Used for mention detection in
    /// groups. `None` → mention check is skipped (forward all).
    pub bot_username: Option<String>,
    pub require_mention_in_groups: bool,
    /// When the bridge is connected, the approver shares the chat_id
    /// state so inline-keyboard approvals can land in the active
    /// conversation. `None` for transport-only tests.
    pub approver: Option<Arc<TelegramApprover>>,
}

#[async_trait]
impl TelegramUpdateSink for TelegramSink {
    async fn on_update(&self, u: Update) {
        if let Some(msg) = u.message.clone().or(u.channel_post.clone()) {
            self.handle_message(msg).await;
            return;
        }
        if let Some(cb) = u.callback_query {
            self.handle_callback(cb).await;
        }
    }
}

impl TelegramSink {
    async fn handle_message(&self, msg: Message) {
        let Some(text) = msg.text.clone() else {
            return;
        };
        if let Some(approver) = &self.approver {
            approver.note_chat_id(msg.chat.id);
        }
        if msg.chat.is_group() && self.require_mention_in_groups {
            if !is_mentioned(&text, &msg.entities, self.bot_username.as_deref())
                && !replied_to_bot(&msg, self.bot_username.as_deref())
            {
                return;
            }
        }
        let cleaned = strip_mention(&text, &msg.entities, self.bot_username.as_deref());
        let (tx_oneshot, rx_oneshot) = oneshot::channel();
        if self
            .input_tx
            .send(ShellInput::TelegramMessage {
                chat_id: msg.chat.id,
                reply_to_message_id: Some(msg.message_id),
                text: cleaned,
                respond: tx_oneshot,
            })
            .is_err()
        {
            eprintln!("[telegram] worker unavailable chat={}", msg.chat.id);
            return;
        }
        let client = self.client.clone();
        let chat_id = msg.chat.id;
        let reply_to = msg.message_id;
        tokio::spawn(async move {
            let answer = match rx_oneshot.await {
                Ok(s) => s,
                Err(_) => return,
            };
            if answer.trim().is_empty() {
                return;
            }
            if let Err(e) = client.send_message(chat_id, &answer, Some(reply_to)).await {
                eprintln!("[telegram] reply failed chat={chat_id}: {e}");
            }
        });
    }

    async fn handle_callback(&self, cb: super::types::CallbackQuery) {
        let cb_id = cb.id.clone();
        let client_for_ack = self.client.clone();
        tokio::spawn(async move {
            if let Err(e) = client_for_ack.answer_callback_query(&cb_id, None).await {
                eprintln!("[telegram] answerCallbackQuery failed: {e}");
            }
        });
        // Resolve the approval directly via the shared `TelegramApprover`
        // rather than routing through `input_tx`. The worker's input
        // loop is BLOCKED inside `ShellInput::TelegramMessage` awaiting
        // the approver's oneshot — sending a TelegramCallback into the
        // same channel would deadlock (the callback arm only runs after
        // the message arm returns, but the message arm can't return
        // until the approver resolves). Calling
        // `record_decision_from_postback` here fires the oneshot
        // directly from the long-poll / webhook task.
        if let Some(data) = cb.data {
            match self.approver.as_ref() {
                Some(approver) => {
                    if approver.record_decision_from_postback(&data).is_none() {
                        eprintln!(
                            "[telegram] callback dropped — no pending approval matches data={data}"
                        );
                    }
                }
                None => {
                    eprintln!("[telegram] callback received but no approver configured");
                }
            }
        }
    }
}

fn is_mentioned(
    text: &str,
    entities: &[MessageEntity],
    username: Option<&str>,
) -> bool {
    // P5 fail-closed: unknown bot username → can't verify a mention;
    // treat as "not mentioned" so the caller drops the message.
    let Some(u) = username else {
        return false;
    };
    let needle = format!("@{u}");
    for e in entities {
        if e.typ == "mention" {
            let start = e.offset as usize;
            let end = start.saturating_add(e.length as usize);
            let slice: String = text.chars().skip(start).take(end - start).collect();
            if slice.eq_ignore_ascii_case(&needle) {
                return true;
            }
        }
    }
    false
}

fn replied_to_bot(msg: &Message, bot_username: Option<&str>) -> bool {
    let Some(reply) = msg.reply_to_message.as_deref() else {
        return false;
    };
    let Some(from) = reply.from.as_ref() else {
        return false;
    };
    if !from.is_bot {
        return false;
    }
    // P5 fail-closed: without our cached @username we can't be sure
    // this reply is to OUR bot vs. another bot in the same group.
    // Drop unless both sides have a username that matches.
    match (from.username.as_deref(), bot_username) {
        (Some(u), Some(bu)) => u.eq_ignore_ascii_case(bu),
        _ => false,
    }
}

fn strip_mention(
    text: &str,
    entities: &[MessageEntity],
    username: Option<&str>,
) -> String {
    let Some(u) = username else {
        return text.to_string();
    };
    let needle = format!("@{u}");
    for e in entities {
        if e.typ == "mention" {
            let start = e.offset as usize;
            let end = start.saturating_add(e.length as usize);
            let slice: String = text.chars().skip(start).take(end - start).collect();
            if slice.eq_ignore_ascii_case(&needle) {
                let head: String = text.chars().take(start).collect();
                let tail: String = text.chars().skip(end).collect();
                return format!("{head}{tail}").trim().to_string();
            }
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::types::{Chat, User};

    fn entity(typ: &str, offset: u64, length: u64) -> MessageEntity {
        MessageEntity {
            typ: typ.into(),
            offset,
            length,
        }
    }

    #[test]
    fn mention_detected_case_insensitive() {
        let text = "@ThclawsBot hello there";
        let ents = vec![entity("mention", 0, 11)];
        assert!(is_mentioned(text, &ents, Some("thclawsbot")));
    }

    #[test]
    fn mention_not_detected_when_username_differs() {
        let text = "@otherbot hello";
        let ents = vec![entity("mention", 0, 9)];
        assert!(!is_mentioned(text, &ents, Some("thclawsbot")));
    }

    #[test]
    fn unknown_username_fails_closed_p5() {
        // P5: when we can't resolve the bot's @username, we cannot
        // reliably verify a mention → treat as NOT mentioned so the
        // group gate drops the message. DMs are unaffected because
        // the caller only consults is_mentioned for groups.
        let text = "hello there";
        let ents = vec![];
        assert!(!is_mentioned(text, &ents, None));
    }

    #[test]
    fn reply_to_unknown_bot_username_fails_closed_p5() {
        let bot = User {
            id: 1,
            is_bot: true,
            username: None,
            first_name: None,
        };
        let parent = Message {
            message_id: 100,
            from: Some(bot),
            chat: Chat {
                id: -1,
                typ: "supergroup".into(),
                username: None,
                title: None,
            },
            text: Some("hi".into()),
            reply_to_message: None,
            entities: vec![],
        };
        let reply = Message {
            message_id: 101,
            from: Some(User {
                id: 42,
                is_bot: false,
                username: None,
                first_name: None,
            }),
            chat: Chat {
                id: -1,
                typ: "supergroup".into(),
                username: None,
                title: None,
            },
            text: Some("thanks".into()),
            reply_to_message: Some(Box::new(parent)),
            entities: vec![],
        };
        // No usernames at all → fail closed.
        assert!(!replied_to_bot(&reply, None));
        // Our cached name set, but the bot they replied to has no
        // username field — still ambiguous, deny.
        assert!(!replied_to_bot(&reply, Some("thclawsbot")));
    }

    #[test]
    fn strip_mention_removes_at_username_prefix() {
        let text = "@thclawsbot what time is it";
        let ents = vec![entity("mention", 0, 11)];
        assert_eq!(
            strip_mention(text, &ents, Some("thclawsbot")),
            "what time is it"
        );
    }

    #[test]
    fn strip_mention_passthrough_when_no_username() {
        let text = "hello";
        assert_eq!(strip_mention(text, &[], None), "hello");
    }

    #[test]
    fn replied_to_bot_matches_username() {
        let bot = User {
            id: 1,
            is_bot: true,
            username: Some("ThclawsBot".into()),
            first_name: None,
        };
        let parent = Message {
            message_id: 100,
            from: Some(bot),
            chat: Chat {
                id: -1,
                typ: "supergroup".into(),
                username: None,
                title: None,
            },
            text: Some("hi".into()),
            reply_to_message: None,
            entities: vec![],
        };
        let reply = Message {
            message_id: 101,
            from: Some(User {
                id: 42,
                is_bot: false,
                username: None,
                first_name: Some("Alice".into()),
            }),
            chat: Chat {
                id: -1,
                typ: "supergroup".into(),
                username: None,
                title: None,
            },
            text: Some("thanks".into()),
            reply_to_message: Some(Box::new(parent)),
            entities: vec![],
        };
        assert!(replied_to_bot(&reply, Some("thclawsbot")));
    }

    #[test]
    fn replied_to_human_not_a_bot_reply() {
        let parent = Message {
            message_id: 100,
            from: Some(User {
                id: 50,
                is_bot: false,
                username: None,
                first_name: Some("Bob".into()),
            }),
            chat: Chat {
                id: -1,
                typ: "supergroup".into(),
                username: None,
                title: None,
            },
            text: Some("hi".into()),
            reply_to_message: None,
            entities: vec![],
        };
        let reply = Message {
            message_id: 101,
            from: Some(User {
                id: 42,
                is_bot: false,
                username: None,
                first_name: None,
            }),
            chat: Chat {
                id: -1,
                typ: "supergroup".into(),
                username: None,
                title: None,
            },
            text: Some("yep".into()),
            reply_to_message: Some(Box::new(parent)),
            entities: vec![],
        };
        assert!(!replied_to_bot(&reply, Some("thclawsbot")));
    }
}
