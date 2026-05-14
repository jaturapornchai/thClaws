//! `BridgeApprovalRouter` — routes tool approval prompts to the LINE
//! or Telegram bridge based on the ambient task-local scope set by
//! the worker arm that drove the current agent turn.
//!
//! Why this exists:
//!   `state.approver` is a single `Arc<dyn ApprovalSink>` slot. Before
//!   this router, the LineConnect / TelegramConnect arms swapped the
//!   slot directly. With two bridges connected, the second swap
//!   silently rerouted the first bridge's approvals to the wrong
//!   chat. The router fans out by ambient context so each turn's
//!   approval lands in the channel that originated it.
//!
//! Ambient context (set by worker arms before driving the agent):
//!   - `crate::telegram::CURRENT_CHAT_ID` (i64) — `TelegramMessage` arm
//!   - `crate::line::LINE_DRIVEN_TURN` (unit) — `LineMessage` arm
//!
//! Out-of-bridge turns (GUI button, REPL, agent self-test) fall back
//! to the pre-bridge approver snapshot.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::permissions::{ApprovalDecision, ApprovalRequest, ApprovalSink};

pub struct BridgeApprovalRouter {
    line: Mutex<Option<Arc<crate::line::approver::LineApprover>>>,
    telegram: Mutex<Option<Arc<crate::telegram::approver::TelegramApprover>>>,
    /// ApprovalSink that was active before the first bridge connected.
    /// Used when an approval fires outside any bridge-driven turn
    /// (GUI button, REPL slash command, etc.).
    fallback: Arc<dyn ApprovalSink>,
}

impl BridgeApprovalRouter {
    pub fn new(fallback: Arc<dyn ApprovalSink>) -> Self {
        Self {
            line: Mutex::new(None),
            telegram: Mutex::new(None),
            fallback,
        }
    }

    pub fn register_line(&self, a: Arc<crate::line::approver::LineApprover>) {
        if let Ok(mut g) = self.line.lock() {
            *g = Some(a);
        }
        eprintln!("[bridge_router] register_line");
    }

    pub fn unregister_line(&self) {
        if let Ok(mut g) = self.line.lock() {
            *g = None;
        }
        eprintln!("[bridge_router] unregister_line");
    }

    pub fn register_telegram(
        &self,
        a: Arc<crate::telegram::approver::TelegramApprover>,
    ) {
        if let Ok(mut g) = self.telegram.lock() {
            *g = Some(a);
        }
        eprintln!("[bridge_router] register_telegram");
    }

    pub fn unregister_telegram(&self) {
        if let Ok(mut g) = self.telegram.lock() {
            *g = None;
        }
        eprintln!("[bridge_router] unregister_telegram");
    }

    /// True when neither bridge is registered — caller restores
    /// `state.approver` ↩ fallback and drops the router.
    pub fn is_empty(&self) -> bool {
        let l = self.line.lock().map(|g| g.is_none()).unwrap_or(true);
        let t = self
            .telegram
            .lock()
            .map(|g| g.is_none())
            .unwrap_or(true);
        l && t
    }

    /// Borrow the fallback so callers can restore it on the way out.
    pub fn fallback(&self) -> Arc<dyn ApprovalSink> {
        self.fallback.clone()
    }
}

#[async_trait]
impl ApprovalSink for BridgeApprovalRouter {
    async fn approve(&self, req: &ApprovalRequest) -> ApprovalDecision {
        // Telegram task-local first — its scope is tightest (per chat_id).
        if crate::telegram::CURRENT_CHAT_ID.try_with(|_| ()).is_ok() {
            let tg = self.telegram.lock().ok().and_then(|g| g.clone());
            if let Some(tg) = tg {
                eprintln!("[bridge_router] route → telegram");
                return tg.approve(req).await;
            }
            // Telegram-driven turn but no telegram approver registered
            // any more (race against disconnect). Fall through to LINE
            // check, then fallback.
        }
        // LINE task-local marker — set by ShellInput::LineMessage arm.
        if crate::line::LINE_DRIVEN_TURN.try_with(|_| ()).is_ok() {
            let line = self.line.lock().ok().and_then(|g| g.clone());
            if let Some(line) = line {
                eprintln!("[bridge_router] route → line");
                return line.approve(req).await;
            }
        }
        // Out-of-bridge turn — use the pre-bridge approver.
        eprintln!("[bridge_router] route → fallback");
        self.fallback.approve(req).await
    }

    fn reset_session_flag(&self) {
        // Propagate the session reset to whichever approvers we still
        // hold plus the fallback so a yolo-decision from session A
        // doesn't leak into session B regardless of which sink owns
        // the approval thread.
        if let Ok(g) = self.line.lock() {
            if let Some(a) = g.as_ref() {
                a.reset_session_flag();
            }
        }
        if let Ok(g) = self.telegram.lock() {
            if let Some(a) = g.as_ref() {
                a.reset_session_flag();
            }
        }
        self.fallback.reset_session_flag();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{ApprovalRequest, DenyApprover};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn req() -> ApprovalRequest {
        ApprovalRequest {
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            summary: None,
            originator: crate::permissions::AgentOrigin::default(),
        }
    }

    /// Fallback that flips a flag so a test can confirm we hit it.
    struct CountSink {
        hits: AtomicUsize,
    }
    #[async_trait]
    impl ApprovalSink for CountSink {
        async fn approve(&self, _req: &ApprovalRequest) -> ApprovalDecision {
            self.hits.fetch_add(1, Ordering::SeqCst);
            ApprovalDecision::Allow
        }
    }

    #[tokio::test]
    async fn routes_to_fallback_when_no_scope() {
        let counter = Arc::new(CountSink {
            hits: AtomicUsize::new(0),
        });
        let router = BridgeApprovalRouter::new(counter.clone());
        let d = router.approve(&req()).await;
        assert_eq!(d, ApprovalDecision::Allow);
        assert_eq!(counter.hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn routes_to_line_when_line_scope_active() {
        let counter = Arc::new(CountSink {
            hits: AtomicUsize::new(0),
        });
        let router = Arc::new(BridgeApprovalRouter::new(counter.clone()));
        let line = Arc::new(crate::line::approver::LineApprover::for_test());
        router.register_line(line.clone());

        let r = router.clone();
        // Drive an approval inside the LINE task-local scope so the
        // router picks the LineApprover.
        let answerer = line.clone();
        let h = tokio::spawn(async move {
            crate::line::LINE_DRIVEN_TURN
                .scope((), async move { r.approve(&req()).await })
                .await
        });
        // The LineApprover blocks on a oneshot until we drive a
        // decision. Yield so the spawn registers the pending entry,
        // then push an Allow text reply through.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert!(answerer.has_pending());
        answerer.record_decision_from_text("approve");
        let d = h.await.unwrap();
        assert_eq!(d, ApprovalDecision::Allow);
        // Fallback was NOT consulted.
        assert_eq!(counter.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn routes_to_telegram_when_chat_id_scope_active() {
        let counter = Arc::new(CountSink {
            hits: AtomicUsize::new(0),
        });
        let router = Arc::new(BridgeApprovalRouter::new(counter.clone()));
        let tg = Arc::new(crate::telegram::approver::TelegramApprover::for_test());
        // TelegramApprover requires last_chat_id to be set before
        // approve() will block on a oneshot. Without it, the approver
        // short-circuits to Deny (correct behaviour — no chat to
        // send the prompt to).
        tg.note_chat_id(42);
        router.register_telegram(tg.clone());

        let r = router.clone();
        let answerer = tg.clone();
        let h = tokio::spawn(async move {
            crate::telegram::CURRENT_CHAT_ID
                .scope(42i64, async move { r.approve(&req()).await })
                .await
        });
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert!(answerer.has_pending());
        answerer.record_decision_from_text("yes");
        let d = h.await.unwrap();
        assert_eq!(d, ApprovalDecision::Allow);
        assert_eq!(counter.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unregister_line_keeps_telegram_active() {
        let counter = Arc::new(CountSink {
            hits: AtomicUsize::new(0),
        });
        let router = Arc::new(BridgeApprovalRouter::new(counter.clone()));
        let line = Arc::new(crate::line::approver::LineApprover::for_test());
        let tg = Arc::new(crate::telegram::approver::TelegramApprover::for_test());
        tg.note_chat_id(7);
        router.register_line(line);
        router.register_telegram(tg.clone());
        router.unregister_line();
        assert!(!router.is_empty());

        let r = router.clone();
        let answerer = tg.clone();
        let h = tokio::spawn(async move {
            crate::telegram::CURRENT_CHAT_ID
                .scope(7i64, async move { r.approve(&req()).await })
                .await
        });
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        answerer.record_decision_from_text("no");
        let d = h.await.unwrap();
        assert_eq!(d, ApprovalDecision::Deny);
        // Fallback never touched.
        assert_eq!(counter.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn is_empty_after_both_unregistered() {
        let router = BridgeApprovalRouter::new(Arc::new(DenyApprover));
        let line = Arc::new(crate::line::approver::LineApprover::for_test());
        let tg = Arc::new(crate::telegram::approver::TelegramApprover::for_test());
        router.register_line(line);
        router.register_telegram(tg);
        assert!(!router.is_empty());
        router.unregister_line();
        assert!(!router.is_empty());
        router.unregister_telegram();
        assert!(router.is_empty());
    }

    #[tokio::test]
    async fn out_of_scope_with_bridges_registered_still_uses_fallback() {
        // Edge case: both bridges are registered, but the approval
        // request fires from a turn that has neither task-local set
        // (e.g. GUI Approve button hit while a Telegram bridge is up).
        let counter = Arc::new(CountSink {
            hits: AtomicUsize::new(0),
        });
        let router = BridgeApprovalRouter::new(counter.clone());
        router.register_line(Arc::new(crate::line::approver::LineApprover::for_test()));
        let tg = Arc::new(crate::telegram::approver::TelegramApprover::for_test());
        tg.note_chat_id(1);
        router.register_telegram(tg);
        let d = router.approve(&req()).await;
        assert_eq!(d, ApprovalDecision::Allow);
        assert_eq!(counter.hits.load(Ordering::SeqCst), 1);
    }
}
