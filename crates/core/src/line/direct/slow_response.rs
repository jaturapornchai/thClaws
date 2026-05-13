//! Slow-response postback cache.
//!
//! When the agent takes longer than the threshold to answer, we
//! reply via the still-valid reply token with a Template Button
//! ("รับคำตอบ"). The agent finishes in the background and stashes
//! its answer here keyed by `request_id`. When the user taps the
//! button, the postback arrives with a NEW reply token; the handler
//! pops the cached answer and replies with that new token.
//!
//! State transitions:
//!
//! ```text
//!     register(chat_id) ──► Pending
//!                              │
//!              ┌───────────────┼───────────────┐
//!     set_ready│      set_error│      mark_button_sent (no state change,
//!              │               │                       just toggles flag)
//!              ▼               ▼
//!            Ready          Error
//!              │               │
//!     on_postback (READY)│   on_postback (ERROR)
//!              ▼               ▼
//!          Delivered       Delivered
//! ```
//!
//! `button_sent: bool` on each entry tells `shared_session` whether
//! it should reply directly (button not yet sent → reply now) or
//! cache for postback (button already sent → store as READY).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Pending,
    Ready(String),
    Delivered,
    Error(String),
}

struct Entry {
    state: State,
    button_sent: bool,
    created: Instant,
}

#[derive(Default)]
pub struct SlowResponseCache {
    inner: Mutex<HashMap<String, Entry>>,
}

#[derive(Debug)]
pub enum PostbackResult {
    Ready(String),
    Error(String),
    StillPending,
    AlreadyDelivered,
    Unknown,
}

impl SlowResponseCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self) -> String {
        let request_id = Uuid::new_v4().to_string();
        let mut g = self.inner.lock().unwrap();
        g.insert(
            request_id.clone(),
            Entry {
                state: State::Pending,
                button_sent: false,
                created: Instant::now(),
            },
        );
        request_id
    }

    pub fn peek_state(&self, request_id: &str) -> Option<State> {
        let g = self.inner.lock().unwrap();
        g.get(request_id).map(|e| e.state.clone())
    }

    pub fn was_button_sent(&self, request_id: &str) -> bool {
        let g = self.inner.lock().unwrap();
        g.get(request_id).is_some_and(|e| e.button_sent)
    }

    pub fn mark_button_sent(&self, request_id: &str) {
        let mut g = self.inner.lock().unwrap();
        if let Some(e) = g.get_mut(request_id) {
            e.button_sent = true;
        }
    }

    pub fn set_ready(&self, request_id: &str, text: String) {
        let mut g = self.inner.lock().unwrap();
        if let Some(e) = g.get_mut(request_id) {
            if matches!(e.state, State::Pending) {
                e.state = State::Ready(text);
            }
        }
    }

    pub fn set_error(&self, request_id: &str, msg: String) {
        let mut g = self.inner.lock().unwrap();
        if let Some(e) = g.get_mut(request_id) {
            if matches!(e.state, State::Pending) {
                e.state = State::Error(msg);
            }
        }
    }

    pub fn on_postback(&self, request_id: &str) -> PostbackResult {
        let mut g = self.inner.lock().unwrap();
        let Some(entry) = g.get_mut(request_id) else {
            return PostbackResult::Unknown;
        };
        // Snapshot + transition. Pending/Delivered are reported but
        // do not mutate state (stay Pending until set_ready/error;
        // stay Delivered to keep idempotent "already replied" UX).
        let snapshot = std::mem::replace(&mut entry.state, State::Delivered);
        match snapshot {
            State::Ready(text) => PostbackResult::Ready(text),
            State::Error(msg) => PostbackResult::Error(msg),
            State::Pending => {
                entry.state = State::Pending;
                PostbackResult::StillPending
            }
            State::Delivered => {
                entry.state = State::Delivered;
                PostbackResult::AlreadyDelivered
            }
        }
    }

    /// Drop DELIVERED/ERROR entries older than 1 h, PENDING older
    /// than 24 h. Worker calls this on a timer.
    pub fn reap(&self) {
        let mut g = self.inner.lock().unwrap();
        g.retain(|_, e| {
            let age = e.created.elapsed().as_secs();
            match &e.state {
                State::Pending => age < 24 * 3600,
                _ => age < 3600,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_returns_unique_request_ids() {
        let c = SlowResponseCache::new();
        let a = c.register();
        let b = c.register();
        assert_ne!(a, b);
    }

    #[test]
    fn fresh_entry_is_pending() {
        let c = SlowResponseCache::new();
        let id = c.register();
        assert!(matches!(c.peek_state(&id), Some(State::Pending)));
        assert!(!c.was_button_sent(&id));
    }

    #[test]
    fn set_ready_transitions_pending_to_ready() {
        let c = SlowResponseCache::new();
        let id = c.register();
        c.set_ready(&id, "answer".into());
        assert!(matches!(c.peek_state(&id), Some(State::Ready(_))));
    }

    #[test]
    fn set_ready_noop_after_delivered() {
        let c = SlowResponseCache::new();
        let id = c.register();
        c.set_ready(&id, "first".into());
        let _ = c.on_postback(&id); // → Delivered
        c.set_ready(&id, "second".into()); // ignored
        assert!(matches!(c.peek_state(&id), Some(State::Delivered)));
    }

    #[test]
    fn on_postback_ready_returns_text_and_marks_delivered() {
        let c = SlowResponseCache::new();
        let id = c.register();
        c.set_ready(&id, "hi".into());
        match c.on_postback(&id) {
            PostbackResult::Ready(t) => assert_eq!(t, "hi"),
            other => panic!("expected Ready, got {other:?}"),
        }
        assert!(matches!(c.peek_state(&id), Some(State::Delivered)));
    }

    #[test]
    fn on_postback_pending_returns_still_pending_no_transition() {
        let c = SlowResponseCache::new();
        let id = c.register();
        match c.on_postback(&id) {
            PostbackResult::StillPending => {}
            other => panic!("expected StillPending, got {other:?}"),
        }
        assert!(matches!(c.peek_state(&id), Some(State::Pending)));
    }

    #[test]
    fn on_postback_delivered_returns_already_delivered() {
        let c = SlowResponseCache::new();
        let id = c.register();
        c.set_ready(&id, "x".into());
        let _ = c.on_postback(&id); // → Delivered
        match c.on_postback(&id) {
            PostbackResult::AlreadyDelivered => {}
            other => panic!("expected AlreadyDelivered, got {other:?}"),
        }
    }

    #[test]
    fn on_postback_unknown_id_returns_unknown() {
        let c = SlowResponseCache::new();
        match c.on_postback("nonexistent") {
            PostbackResult::Unknown => {}
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn set_error_path() {
        let c = SlowResponseCache::new();
        let id = c.register();
        c.set_error(&id, "boom".into());
        match c.on_postback(&id) {
            PostbackResult::Error(m) => assert_eq!(m, "boom"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn button_sent_flag_independent_of_state() {
        let c = SlowResponseCache::new();
        let id = c.register();
        assert!(!c.was_button_sent(&id));
        c.mark_button_sent(&id);
        assert!(c.was_button_sent(&id));
        assert!(matches!(c.peek_state(&id), Some(State::Pending)));
        c.set_ready(&id, "x".into());
        assert!(c.was_button_sent(&id));
    }
}
