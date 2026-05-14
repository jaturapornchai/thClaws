//! `TelegramApprover` — implements `ApprovalSink` by routing the
//! approval prompt to a Telegram chat with an inline keyboard.
//!
//! Posture: identical contract to `LineApprover`. Difference is that
//! Telegram permits unsolicited `sendMessage` to any chat that has
//! previously messaged the bot (no Reply API token rotation), so the
//! approver only needs to know *some* allowed chat_id.
//!
//! The `last_chat_id` lock is updated by the sink on every inbound
//! allowed user message. If nothing has ever messaged the bot, the
//! approver denies the tool call and logs
//! `[telegram] approval denied: no_chat_known`.
//!
//! Callback shape (matches `LineApprover::parse_postback`):
//!   - `tool:allow:<request_id>` → allow
//!   - `tool:deny:<request_id>` → deny
//!
//! `record_decision_*` API mirrors `LineApprover` so the session
//! could one day use a trait object — but for now they live in
//! parallel because both modules pre-date a shared abstraction.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::oneshot;

use super::client::TelegramClient;
use crate::permissions::{ApprovalDecision, ApprovalRequest, ApprovalSink};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

const INPUT_PREVIEW_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalReply {
    Allow,
    Deny,
    Unrecognised,
}

impl ApprovalReply {
    pub fn parse_text(input: &str) -> Self {
        let t = input.trim().to_lowercase();
        match t.as_str() {
            "y" | "yes" | "ok" | "approve" | "approved" | "allow" | "a" | "ใช่" | "อนุญาต" => {
                Self::Allow
            }
            "n" | "no" | "deny" | "denied" | "block" | "reject" | "d" | "ไม่" | "ปฏิเสธ" => {
                Self::Deny
            }
            _ => Self::Unrecognised,
        }
    }

    /// Telegram callback_data is plain ASCII. Accept both
    /// `tool:<verb>:<id>` (matches LINE convention so the same UI
    /// strings can be shared) and the bare `<verb>:<id>`.
    pub fn parse_postback(data: &str) -> (Self, Option<String>) {
        let parts: Vec<&str> = data.split(':').collect();
        let (verb, req_id) = match parts.as_slice() {
            ["tool", verb, req] => (*verb, Some((*req).to_string())),
            [verb, req] => (*verb, Some((*req).to_string())),
            [verb] => (*verb, None),
            _ => return (Self::Unrecognised, None),
        };
        let decision = match verb.to_lowercase().as_str() {
            "allow" | "approve" | "yes" => Self::Allow,
            "deny" | "reject" | "no" => Self::Deny,
            _ => Self::Unrecognised,
        };
        (decision, req_id)
    }
}

#[derive(Default)]
struct Pending {
    by_id: HashMap<String, oneshot::Sender<ApprovalDecision>>,
    order: Vec<String>,
}

impl Pending {
    fn insert(&mut self, id: String, tx: oneshot::Sender<ApprovalDecision>) {
        self.by_id.insert(id.clone(), tx);
        self.order.push(id);
    }
    fn take_by_id(&mut self, id: &str) -> Option<oneshot::Sender<ApprovalDecision>> {
        let tx = self.by_id.remove(id)?;
        self.order.retain(|x| x != id);
        Some(tx)
    }
    fn take_most_recent(&mut self) -> Option<oneshot::Sender<ApprovalDecision>> {
        let id = self.order.pop()?;
        self.by_id.remove(&id)
    }
    fn has_any(&self) -> bool {
        !self.order.is_empty()
    }
}

#[derive(Clone)]
pub struct TelegramApprover {
    client: Option<Arc<TelegramClient>>,
    /// The chat_id the next approval prompt will be sent to. Updated
    /// by the sink on every inbound allowed user message — last
    /// activity wins so approvals follow the user's most recent
    /// conversation. None until the first user message arrives.
    last_chat_id: Arc<Mutex<Option<i64>>>,
    pending: Arc<Mutex<Pending>>,
    timeout: Duration,
}

impl TelegramApprover {
    pub fn new(client: Arc<TelegramClient>) -> Self {
        Self {
            client: Some(client),
            last_chat_id: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(Pending::default())),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    #[cfg(test)]
    pub fn for_test() -> Self {
        Self {
            client: None,
            last_chat_id: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(Pending::default())),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    /// Called by the sink whenever a user message lands. Always
    /// overwrite — last activity wins.
    pub fn note_chat_id(&self, chat_id: i64) {
        if let Ok(mut g) = self.last_chat_id.lock() {
            *g = Some(chat_id);
        }
    }

    pub fn last_chat_id(&self) -> Option<i64> {
        self.last_chat_id.lock().ok().and_then(|g| *g)
    }

    pub fn has_pending(&self) -> bool {
        self.pending.lock().map(|p| p.has_any()).unwrap_or(false)
    }

    pub fn record_decision_by_id(&self, request_id: &str, decision: ApprovalDecision) -> bool {
        let tx = self
            .pending
            .lock()
            .ok()
            .and_then(|mut p| p.take_by_id(request_id));
        match tx {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    pub fn record_decision_from_text(&self, text: &str) -> Option<ApprovalReply> {
        if !self.has_pending() {
            return None;
        }
        let reply = ApprovalReply::parse_text(text);
        let decision = match reply {
            ApprovalReply::Allow => ApprovalDecision::Allow,
            ApprovalReply::Deny => ApprovalDecision::Deny,
            ApprovalReply::Unrecognised => return Some(ApprovalReply::Unrecognised),
        };
        if let Some(tx) = self
            .pending
            .lock()
            .ok()
            .and_then(|mut p| p.take_most_recent())
        {
            let _ = tx.send(decision);
            return Some(reply);
        }
        None
    }

    pub fn record_decision_from_postback(&self, data: &str) -> Option<ApprovalReply> {
        let (reply, req_id) = ApprovalReply::parse_postback(data);
        let decision = match reply {
            ApprovalReply::Allow => ApprovalDecision::Allow,
            ApprovalReply::Deny => ApprovalDecision::Deny,
            ApprovalReply::Unrecognised => return None,
        };
        let resolved = match req_id {
            Some(id) => self.record_decision_by_id(&id, decision),
            None => self
                .pending
                .lock()
                .ok()
                .and_then(|mut p| p.take_most_recent())
                .map(|tx| tx.send(decision).is_ok())
                .unwrap_or(false),
        };
        if resolved {
            Some(reply)
        } else {
            None
        }
    }

    fn build_prompt(req: &ApprovalRequest) -> String {
        let input_str = serde_json::to_string(&req.input).unwrap_or_else(|_| String::new());
        let preview: String = input_str.chars().take(INPUT_PREVIEW_CHARS).collect();
        let ellipsis = if input_str.chars().count() > INPUT_PREVIEW_CHARS {
            "…"
        } else {
            ""
        };
        format!(
            "🔐 thClaws wants to run: {tool}\n\nInput: {preview}{ellipsis}\n\nTap Approve or Deny (auto-denies in 60s).",
            tool = req.tool_name,
            preview = preview,
            ellipsis = ellipsis,
        )
    }

    fn build_keyboard(request_id: &str) -> serde_json::Value {
        json!({
            "inline_keyboard": [[
                {"text": "✅ Approve", "callback_data": format!("tool:allow:{request_id}")},
                {"text": "🚫 Deny", "callback_data": format!("tool:deny:{request_id}")},
            ]]
        })
    }
}

#[async_trait]
impl ApprovalSink for TelegramApprover {
    async fn approve(&self, req: &ApprovalRequest) -> ApprovalDecision {
        // Per-chat routing: try the task-local scope first (set by
        // the TelegramMessage worker arm around handle_line). Falls
        // back to the last-seen chat across all users only when the
        // approval is fired outside a Telegram-driven turn (no
        // ambient chat → use the legacy global default).
        let chat_id = match super::CURRENT_CHAT_ID.try_with(|c| *c) {
            Ok(c) => c,
            Err(_) => match self.last_chat_id() {
                Some(c) => c,
                None => {
                    eprintln!(
                        "[telegram] approval denied: no_chat_known (tool={})",
                        req.tool_name
                    );
                    return ApprovalDecision::Deny;
                }
            },
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            p.insert(request_id.clone(), tx);
        }

        if let Some(client) = &self.client {
            let prompt = Self::build_prompt(req);
            let keyboard = Self::build_keyboard(&request_id);
            if let Err(e) = client
                .send_message_with_keyboard(chat_id, &prompt, &keyboard)
                .await
            {
                eprintln!(
                    "[telegram] approval send failed chat={chat_id}: {e}; auto-denying"
                );
                self.record_decision_by_id(&request_id, ApprovalDecision::Deny);
                return ApprovalDecision::Deny;
            }
        }

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(_canceled)) => ApprovalDecision::Deny,
            Err(_elapsed) => {
                eprintln!(
                    "[telegram] approval for {} timed out after {:?}; auto-denying",
                    req.tool_name, self.timeout
                );
                if let Ok(mut p) = self.pending.lock() {
                    let _ = p.take_by_id(&request_id);
                }
                if let Some(client) = &self.client {
                    let _ = client
                        .send_message(
                            chat_id,
                            &format!("⏰ Approval for {} timed out; auto-denied.", req.tool_name),
                            None,
                        )
                        .await;
                }
                ApprovalDecision::Deny
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(tool: &str) -> ApprovalRequest {
        ApprovalRequest {
            tool_name: tool.into(),
            input: json!({"path": "/tmp/x"}),
            summary: None,
            originator: crate::permissions::AgentOrigin::default(),
        }
    }

    #[tokio::test]
    async fn no_chat_known_denies_immediately() {
        let a = TelegramApprover::for_test();
        let decision = a.approve(&req("Bash")).await;
        assert_eq!(decision, ApprovalDecision::Deny);
        // Nothing pending — the approver shortcut returned without
        // registering the request.
        assert!(!a.has_pending());
    }

    #[tokio::test]
    async fn text_reply_allow_resolves() {
        let a = TelegramApprover::for_test();
        a.note_chat_id(42);
        let a2 = a.clone();
        let h = tokio::spawn(async move { a2.approve(&req("Bash")).await });
        tokio::task::yield_now().await;
        assert!(a.has_pending());
        let r = a.record_decision_from_text("approve");
        assert_eq!(r, Some(ApprovalReply::Allow));
        assert_eq!(h.await.unwrap(), ApprovalDecision::Allow);
    }

    #[tokio::test]
    async fn text_reply_deny_resolves() {
        let a = TelegramApprover::for_test();
        a.note_chat_id(42);
        let a2 = a.clone();
        let h = tokio::spawn(async move { a2.approve(&req("Edit")).await });
        tokio::task::yield_now().await;
        let r = a.record_decision_from_text("no");
        assert_eq!(r, Some(ApprovalReply::Deny));
        assert_eq!(h.await.unwrap(), ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn unrecognised_keeps_pending() {
        let a = TelegramApprover::for_test();
        a.note_chat_id(42);
        let a2 = a.clone();
        let h = tokio::spawn(async move { a2.approve(&req("Bash")).await });
        tokio::task::yield_now().await;
        let r = a.record_decision_from_text("hmm");
        assert_eq!(r, Some(ApprovalReply::Unrecognised));
        assert!(a.has_pending());
        // Second-try resolves.
        a.record_decision_from_text("yes");
        assert_eq!(h.await.unwrap(), ApprovalDecision::Allow);
    }

    #[tokio::test]
    async fn postback_with_id_resolves_specific() {
        let a = TelegramApprover::for_test();
        a.note_chat_id(42);
        let a1 = a.clone();
        let h1 = tokio::spawn(async move { a1.approve(&req("Bash")).await });
        tokio::task::yield_now().await;
        let a2 = a.clone();
        let h2 = tokio::spawn(async move { a2.approve(&req("Edit")).await });
        tokio::task::yield_now().await;
        let ids: Vec<String> = {
            let p = a.pending.lock().unwrap();
            p.order.clone()
        };
        assert_eq!(ids.len(), 2);
        let raw = format!("tool:allow:{}", ids[0]);
        let r = a.record_decision_from_postback(&raw);
        assert_eq!(r, Some(ApprovalReply::Allow));
        assert_eq!(h1.await.unwrap(), ApprovalDecision::Allow);

        // Second still pending → resolve via text deny
        assert!(a.has_pending());
        a.record_decision_from_text("deny");
        assert_eq!(h2.await.unwrap(), ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn timeout_auto_denies() {
        let a = TelegramApprover::for_test().with_timeout(Duration::from_millis(50));
        a.note_chat_id(42);
        let decision = a.approve(&req("Bash")).await;
        assert_eq!(decision, ApprovalDecision::Deny);
        assert!(!a.has_pending());
    }

    #[test]
    fn parse_text_accepts_common_short_forms() {
        for s in ["yes", "Y", " approve ", "OK", "a", "ใช่", "อนุญาต"] {
            assert_eq!(ApprovalReply::parse_text(s), ApprovalReply::Allow, "{s}");
        }
        for s in ["no", "N", "deny", "reject", "d", "ไม่", "ปฏิเสธ"] {
            assert_eq!(ApprovalReply::parse_text(s), ApprovalReply::Deny, "{s}");
        }
        for s in ["maybe", "later", ""] {
            assert_eq!(
                ApprovalReply::parse_text(s),
                ApprovalReply::Unrecognised,
                "{s}"
            );
        }
    }

    #[test]
    fn parse_postback_accepts_both_shapes() {
        let (r, id) = ApprovalReply::parse_postback("tool:allow:abc123");
        assert_eq!(r, ApprovalReply::Allow);
        assert_eq!(id.as_deref(), Some("abc123"));
        let (r, id) = ApprovalReply::parse_postback("deny:xyz789");
        assert_eq!(r, ApprovalReply::Deny);
        assert_eq!(id.as_deref(), Some("xyz789"));
        let (r, id) = ApprovalReply::parse_postback("garbage");
        assert_eq!(r, ApprovalReply::Unrecognised);
        assert!(id.is_none());
    }

    #[test]
    fn build_keyboard_has_two_buttons_per_row() {
        let kb = TelegramApprover::build_keyboard("req-1");
        let arr = kb["inline_keyboard"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].as_array().unwrap().len(), 2);
        assert_eq!(arr[0][0]["callback_data"], "tool:allow:req-1");
        assert_eq!(arr[0][1]["callback_data"], "tool:deny:req-1");
    }
}
