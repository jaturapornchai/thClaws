//! `DirectSink` — bridges the webhook server to the worker's
//! `ShellInput::LineMessage` channel and orchestrates the slow-
//! response postback flow.
//!
//! Wiring:
//!   - inbound text → register slow-response entry → start the
//!     button timer → forward text to worker via input_tx → await
//!     agent reply in a detached task → reply now OR cache for
//!     postback depending on whether the button was already sent
//!   - inbound `show_response` postback → look up cache → reply
//!     using the FRESH reply token from the postback event
//!
//! The webhook handler returns 200 OK as soon as `on_message`
//! returns — every long-running step happens in spawned tasks so
//! LINE Platform does not see a slow webhook.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;

use super::client::DirectLineClient;
use super::reply_store::ReplyTokenStore;
use super::server::DirectEventSink;
use super::slow_response::{PostbackResult, SlowResponseCache, State};
use crate::shared_session::ShellInput;

pub struct DirectSink {
    pub input_tx: mpsc::Sender<ShellInput>,
    pub client: Arc<DirectLineClient>,
    pub reply_store: Arc<ReplyTokenStore>,
    pub slow_cache: Arc<SlowResponseCache>,
    pub threshold: Duration,
}

impl DirectSink {
    pub fn new(
        input_tx: mpsc::Sender<ShellInput>,
        client: Arc<DirectLineClient>,
        reply_store: Arc<ReplyTokenStore>,
        slow_cache: Arc<SlowResponseCache>,
        threshold: Duration,
    ) -> Self {
        Self {
            input_tx,
            client,
            reply_store,
            slow_cache,
            threshold,
        }
    }
}

#[async_trait]
impl DirectEventSink for DirectSink {
    async fn on_message(&self, chat_id: String, reply_token: String, text: String) {
        let request_id = self.slow_cache.register();

        // Slow-response timer: fire the Template Button if the agent
        // is still PENDING when the threshold elapses.
        let slow_client = self.client.clone();
        let slow_cache_timer = self.slow_cache.clone();
        let req_for_timer = request_id.clone();
        let token_for_timer = reply_token.clone();
        let chat_for_log = chat_id.clone();
        let threshold = self.threshold;
        let button_task = tokio::spawn(async move {
            tokio::time::sleep(threshold).await;
            // Only fire if still PENDING — agent might have replied
            // already, in which case the reply path consumed the
            // reply token and the cache state is no longer Pending.
            if !matches!(
                slow_cache_timer.peek_state(&req_for_timer),
                Some(State::Pending)
            ) {
                return;
            }
            let data =
                serde_json::json!({"action":"show_response","request_id":req_for_timer})
                    .to_string();
            match slow_client
                .reply_template_button(
                    &token_for_timer,
                    "กำลังคิดอยู่ กดรับคำตอบเมื่อพร้อม",
                    "รับคำตอบ",
                    &data,
                )
                .await
            {
                Ok(()) => slow_cache_timer.mark_button_sent(&req_for_timer),
                Err(e) => eprintln!(
                    "[line/direct] slow-response button failed chat={}: {e}",
                    hash_chat(&chat_for_log)
                ),
            }
        });

        // Forward to worker.
        let (oneshot_tx, oneshot_rx) = oneshot::channel();
        if self
            .input_tx
            .send(ShellInput::LineMessage {
                text,
                respond: oneshot_tx,
            })
            .is_err()
        {
            eprintln!(
                "[line/direct] worker unavailable chat={}",
                hash_chat(&chat_id)
            );
            button_task.abort();
            self.slow_cache
                .set_error(&request_id, "worker_unavailable".into());
            return;
        }

        // Await agent reply in a detached task so the webhook handler
        // returns 200 immediately.
        let client_for_reply = self.client.clone();
        let cache_for_reply = self.slow_cache.clone();
        let store_for_reply = self.reply_store.clone();
        let chat_for_reply = chat_id;
        let token_for_reply = reply_token;
        let req_for_reply = request_id;
        tokio::spawn(async move {
            let answer = match oneshot_rx.await {
                Ok(s) => s,
                Err(_) => {
                    cache_for_reply.set_error(&req_for_reply, "agent_dropped".into());
                    return;
                }
            };
            // Stop the button timer — either we beat it (reply direct
            // path) or we lost (cache for postback). In both cases
            // the timer task no longer needs to run.
            button_task.abort();

            if answer.trim().is_empty() {
                // Empty assistant turn (tool-only). Mark as ready with
                // empty text so postback path won't reply twice.
                cache_for_reply.set_ready(&req_for_reply, String::new());
                return;
            }

            if cache_for_reply.was_button_sent(&req_for_reply) {
                // Button already in user's chat. Caching for postback
                // (must not reply now — original token consumed by
                // the button send).
                cache_for_reply.set_ready(&req_for_reply, answer);
                return;
            }

            // Try direct reply with the original token.
            if let Some(tok) = store_for_reply.take(&chat_for_reply) {
                // `tok` should equal token_for_reply; using the store
                // version handles a "user sent another message"
                // intervening case where reply_token rotated.
                if let Err(e) = client_for_reply.reply_text(&tok, &answer).await {
                    eprintln!(
                        "[line/direct] reply failed chat={}: {e}",
                        hash_chat(&chat_for_reply)
                    );
                }
                cache_for_reply.set_ready(&req_for_reply, String::new());
            } else {
                // Token already gone (button raced ahead of consume,
                // or 50s TTL expired). Drop with sanitized log — no
                // Push fallback by design.
                eprintln!(
                    "[line/direct] drop: NoValidReplyToken chat={} (token used or expired)",
                    hash_chat(&chat_for_reply)
                );
                let _ = token_for_reply; // silence unused warning
                cache_for_reply.set_ready(&req_for_reply, String::new());
            }
        });
    }

    async fn on_show_response_postback(
        &self,
        chat_id: String,
        reply_token: String,
        request_id: String,
    ) {
        let text = match self.slow_cache.on_postback(&request_id) {
            PostbackResult::Ready(t) if !t.is_empty() => t,
            PostbackResult::Ready(_) => {
                // Empty cached answer — agent finished with no text
                // (tool-only turn). Tell the user politely.
                "(thClaws ทำงานเสร็จแล้วแต่ไม่มีคำตอบเป็นข้อความ)".into()
            }
            PostbackResult::Error(m) => format!("งานถูกยกเลิก: {m}"),
            PostbackResult::StillPending => "ยังทำงานอยู่ กรุณารอสักครู่".into(),
            PostbackResult::AlreadyDelivered => "ส่งคำตอบแล้ว ✅".into(),
            PostbackResult::Unknown => return,
        };
        if let Err(e) = self.client.reply_text(&reply_token, &text).await {
            eprintln!(
                "[line/direct] postback reply failed chat={}: {e}",
                hash_chat(&chat_id)
            );
        }
    }
}

/// SHA-256 first 8 hex chars of a chat id. Used in logs so a
/// support ticket can correlate without exposing the raw LINE id.
fn hash_chat(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(id.as_bytes());
    let out = h.finalize();
    let mut s = String::with_capacity(8);
    for b in &out[..4] {
        use std::fmt::Write;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_chat_stable_and_short() {
        let a = hash_chat("U12345");
        let b = hash_chat("U12345");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
        assert_ne!(hash_chat("U1"), hash_chat("U2"));
    }
}
