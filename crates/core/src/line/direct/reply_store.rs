//! ReplyToken store with 50s TTL + single-consume semantics.
//!
//! LINE replyTokens expire ~60s after delivery; we cap at 50s for
//! network slack. Single-consume: once `take()` returns it, gone.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const TOKEN_TTL: Duration = Duration::from_secs(50);

#[derive(Default)]
pub struct ReplyTokenStore {
    inner: Mutex<HashMap<String, (String, Instant)>>,
}

impl ReplyTokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a fresh reply token for `chat_id`. Overwrites prior token.
    pub fn put(&self, chat_id: &str, token: String) {
        let mut g = self.inner.lock().unwrap();
        g.insert(chat_id.to_string(), (token, Instant::now()));
    }

    /// Pop the token if still within TTL.
    pub fn take(&self, chat_id: &str) -> Option<String> {
        let mut g = self.inner.lock().unwrap();
        let (token, recorded) = g.remove(chat_id)?;
        if recorded.elapsed() <= TOKEN_TTL {
            Some(token)
        } else {
            None
        }
    }

    /// Peek without consuming.
    pub fn has_valid(&self, chat_id: &str) -> bool {
        let g = self.inner.lock().unwrap();
        g.get(chat_id)
            .map(|(_, recorded)| recorded.elapsed() <= TOKEN_TTL)
            .unwrap_or(false)
    }

    /// Pop the most-recently-recorded token from any chat (single-
    /// owner / single-user bot scenario). Used by approval flows
    /// that need a reply token but don't know which chat triggered
    /// the agent turn at call time. Skips entries past TTL. Garbage-
    /// collects expired entries along the way.
    pub fn take_any(&self) -> Option<(String, String)> {
        let mut g = self.inner.lock().unwrap();
        g.retain(|_, (_, recorded)| recorded.elapsed() <= TOKEN_TTL);
        let key = g
            .iter()
            .max_by_key(|(_, (_, recorded))| *recorded)
            .map(|(k, _)| k.clone())?;
        let (token, _) = g.remove(&key)?;
        Some((key, token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_take_returns_token() {
        let s = ReplyTokenStore::new();
        s.put("U1", "tok-1".into());
        assert_eq!(s.take("U1").as_deref(), Some("tok-1"));
    }

    #[test]
    fn second_take_returns_none() {
        let s = ReplyTokenStore::new();
        s.put("U1", "tok".into());
        assert!(s.take("U1").is_some());
        assert!(s.take("U1").is_none());
    }

    #[test]
    fn new_put_overwrites_old() {
        let s = ReplyTokenStore::new();
        s.put("U1", "old".into());
        s.put("U1", "new".into());
        assert_eq!(s.take("U1").as_deref(), Some("new"));
    }

    #[test]
    fn distinct_chats_independent() {
        let s = ReplyTokenStore::new();
        s.put("U1", "a".into());
        s.put("U2", "b".into());
        assert_eq!(s.take("U1").as_deref(), Some("a"));
        assert_eq!(s.take("U2").as_deref(), Some("b"));
    }

    #[test]
    fn has_valid_after_put_true() {
        let s = ReplyTokenStore::new();
        s.put("U1", "x".into());
        assert!(s.has_valid("U1"));
    }

    #[test]
    fn has_valid_after_take_false() {
        let s = ReplyTokenStore::new();
        s.put("U1", "x".into());
        let _ = s.take("U1");
        assert!(!s.has_valid("U1"));
    }

    #[test]
    fn take_any_pops_most_recent_chat() {
        let s = ReplyTokenStore::new();
        s.put("U1", "old".into());
        std::thread::sleep(Duration::from_millis(10));
        s.put("U2", "newer".into());
        let (chat, tok) = s.take_any().expect("token available");
        assert_eq!(chat, "U2");
        assert_eq!(tok, "newer");
        // Second take_any picks the remaining one.
        let (chat, tok) = s.take_any().expect("still one");
        assert_eq!(chat, "U1");
        assert_eq!(tok, "old");
        assert!(s.take_any().is_none());
    }

    #[test]
    fn take_any_returns_none_when_empty() {
        let s = ReplyTokenStore::new();
        assert!(s.take_any().is_none());
    }
}
