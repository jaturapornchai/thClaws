//! Axum webhook server.
//!
//! Pipeline per request:
//!   1. Read raw body bytes (signature covers raw bytes — never re-serialize).
//!   2. Verify `X-Line-Signature` → 401 on mismatch.
//!   3. Parse JSON → 400 on malformed.
//!   4. For each event: dedup → allowlist → stash reply token → sink dispatch.
//!
//! The handler returns 200 OK ASAP. It never blocks on agent work;
//! the sink contract is fire-and-forget.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};

use super::allowlist::Allowlist;
use super::config::WEBHOOK_PATH;
use super::dedup::DedupStore;
use super::reply_store::ReplyTokenStore;
use super::signature::verify;
use super::slow_response::SlowResponseCache;
use super::types::{Event, MessagePayload, ShowResponsePostback, WebhookBody};

/// Trait the runtime supplies. Decouples server from worker + slow-
/// response wiring so the server can be unit-tested in isolation.
#[async_trait::async_trait]
pub trait DirectEventSink: Send + Sync + 'static {
    /// New text message arrived. Implementor stashes the reply
    /// token, starts the slow-response timer, dispatches to the agent.
    async fn on_message(&self, chat_id: String, reply_token: String, text: String);

    /// `show_response` postback arrived. `request_id` keys the
    /// slow-response cache entry. `reply_token` here is FRESH
    /// (every webhook event carries its own) — use it for the
    /// final reply.
    async fn on_show_response_postback(
        &self,
        chat_id: String,
        reply_token: String,
        request_id: String,
    );
}

pub struct DirectServerState {
    pub channel_secret: Vec<u8>,
    pub allowlist: Allowlist,
    pub dedup: Arc<DedupStore>,
    pub reply_store: Arc<ReplyTokenStore>,
    pub slow_cache: Arc<SlowResponseCache>,
    pub sink: Arc<dyn DirectEventSink>,
}

pub fn router(state: Arc<DirectServerState>) -> Router {
    Router::new()
        .route(WEBHOOK_PATH, post(handle_webhook))
        .route("/line/health", get(handle_health))
        .with_state(state)
}

async fn handle_health() -> &'static str {
    "ok"
}

async fn handle_webhook(
    State(state): State<Arc<DirectServerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let sig = headers
        .get("x-line-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify(&body, sig, &state.channel_secret) {
        return (StatusCode::UNAUTHORIZED, "bad signature").into_response();
    }
    let parsed: WebhookBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad body").into_response(),
    };
    for event in parsed.events {
        dispatch_event(state.clone(), event).await;
    }
    (StatusCode::OK, "").into_response()
}

async fn dispatch_event(state: Arc<DirectServerState>, event: Event) {
    match event {
        Event::Message {
            webhook_event_id,
            reply_token,
            source,
            message,
        } => {
            if !state.dedup.check_and_record(&webhook_event_id) {
                return;
            }
            if !state.allowlist.is_allowed(&source) {
                return;
            }
            let MessagePayload::Text { text } = message else {
                return;
            };
            let Some(chat_id) = source.chat_id() else {
                return;
            };
            state.reply_store.put(chat_id, reply_token.clone());
            state
                .sink
                .on_message(chat_id.to_string(), reply_token, text)
                .await;
        }
        Event::Postback {
            webhook_event_id,
            reply_token,
            source,
            postback,
        } => {
            if !state.dedup.check_and_record(&webhook_event_id) {
                return;
            }
            if !state.allowlist.is_allowed(&source) {
                return;
            }
            let Some(chat_id) = source.chat_id() else {
                return;
            };
            if let Ok(p) = serde_json::from_str::<ShowResponsePostback>(&postback.data) {
                if p.action == "show_response" {
                    state
                        .sink
                        .on_show_response_postback(
                            chat_id.to_string(),
                            reply_token,
                            p.request_id,
                        )
                        .await;
                }
            }
        }
        Event::Other => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use std::sync::Mutex;
    use tower::ServiceExt;

    type HmacSha256 = Hmac<Sha256>;

    #[derive(Default)]
    struct FakeSink {
        messages: Mutex<Vec<(String, String, String)>>,
        postbacks: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait::async_trait]
    impl DirectEventSink for FakeSink {
        async fn on_message(&self, chat_id: String, reply_token: String, text: String) {
            self.messages.lock().unwrap().push((chat_id, reply_token, text));
        }
        async fn on_show_response_postback(
            &self,
            chat_id: String,
            reply_token: String,
            request_id: String,
        ) {
            self.postbacks
                .lock()
                .unwrap()
                .push((chat_id, reply_token, request_id));
        }
    }

    fn sign(body: &[u8], secret: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
    }

    fn build(secret: &[u8], sink: Arc<FakeSink>, allow_user: &str) -> Router {
        let state = Arc::new(DirectServerState {
            channel_secret: secret.to_vec(),
            allowlist: Allowlist::from_csv(allow_user, "", ""),
            dedup: Arc::new(DedupStore::new()),
            reply_store: Arc::new(ReplyTokenStore::new()),
            slow_cache: Arc::new(SlowResponseCache::new()),
            sink,
        });
        router(state)
    }

    #[tokio::test]
    async fn bad_signature_returns_401() {
        let sink = Arc::new(FakeSink::default());
        let app = build(b"secret", sink.clone(), "U1");
        let body = br#"{"events":[]}"#;
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .header("x-line-signature", "deadbeef")
            .body(Body::from(body.to_vec()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn malformed_json_returns_400() {
        let sink = Arc::new(FakeSink::default());
        let app = build(b"secret", sink.clone(), "U1");
        let body = b"not json";
        let sig = sign(body, b"secret");
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .header("x-line-signature", sig)
            .body(Body::from(body.to_vec()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_user_dropped_silently_with_200() {
        let sink = Arc::new(FakeSink::default());
        let app = build(b"secret", sink.clone(), "U_ALLOWED");
        let body = br#"{"events":[{
            "type":"message","webhookEventId":"e1","replyToken":"t1",
            "source":{"type":"user","userId":"U_OTHER"},
            "message":{"type":"text","text":"hi"}
        }]}"#;
        let sig = sign(body, b"secret");
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .header("x-line-signature", sig)
            .body(Body::from(body.to_vec()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(sink.messages.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn duplicate_webhook_event_dispatched_once() {
        let sink = Arc::new(FakeSink::default());
        let app = build(b"secret", sink.clone(), "U1");
        let body = br#"{"events":[{
            "type":"message","webhookEventId":"dup","replyToken":"t1",
            "source":{"type":"user","userId":"U1"},
            "message":{"type":"text","text":"hi"}
        }]}"#;
        let sig = sign(body, b"secret");
        for _ in 0..2 {
            let req = Request::builder()
                .method("POST")
                .uri(WEBHOOK_PATH)
                .header("x-line-signature", sig.clone())
                .body(Body::from(body.to_vec()))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
        assert_eq!(sink.messages.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn text_message_dispatches_to_sink_with_reply_token() {
        let sink = Arc::new(FakeSink::default());
        let app = build(b"secret", sink.clone(), "U1");
        let body = br#"{"events":[{
            "type":"message","webhookEventId":"e1","replyToken":"tok-X",
            "source":{"type":"user","userId":"U1"},
            "message":{"type":"text","text":"hello"}
        }]}"#;
        let sig = sign(body, b"secret");
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .header("x-line-signature", sig)
            .body(Body::from(body.to_vec()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let m = sink.messages.lock().unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, "U1");
        assert_eq!(m[0].1, "tok-X");
        assert_eq!(m[0].2, "hello");
    }

    #[tokio::test]
    async fn show_response_postback_dispatches_with_request_id() {
        let sink = Arc::new(FakeSink::default());
        let app = build(b"secret", sink.clone(), "U1");
        let body = br#"{"events":[{
            "type":"postback","webhookEventId":"e2","replyToken":"new-tok",
            "source":{"type":"user","userId":"U1"},
            "postback":{"data":"{\"action\":\"show_response\",\"request_id\":\"req-abc\"}"}
        }]}"#;
        let sig = sign(body, b"secret");
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .header("x-line-signature", sig)
            .body(Body::from(body.to_vec()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let p = sink.postbacks.lock().unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "U1");
        assert_eq!(p[0].1, "new-tok");
        assert_eq!(p[0].2, "req-abc");
    }

    #[tokio::test]
    async fn health_endpoint() {
        let sink = Arc::new(FakeSink::default());
        let app = build(b"secret", sink, "U1");
        let req = Request::builder()
            .method("GET")
            .uri("/line/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
