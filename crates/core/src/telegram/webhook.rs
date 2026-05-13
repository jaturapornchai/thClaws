//! Axum webhook server for Telegram updates.
//!
//! Only used when `TelegramMode::Webhook`. Default mode is long-poll
//! which needs no public endpoint.
//!
//! Verification: Telegram echoes the `secret_token` configured at
//! `setWebhook` time in the `X-Telegram-Bot-Api-Secret-Token` header
//! on every POST. Not HMAC — just a shared secret. We compare in
//! constant time. Mismatch → 401.
//!
//! Pipeline per request:
//!   1. Header check → 401 on mismatch / missing.
//!   2. JSON parse → 400 on malformed.
//!   3. dispatch::route_update — same allowlist gate as long-poll.
//!   4. 200 OK regardless of whether the update was forwarded.

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
use super::dispatch;
use super::long_poll::TelegramUpdateSink;
use super::types::Update;

pub struct WebhookState {
    pub secret_token: String,
    pub allowlist: Arc<Allowlist>,
    pub sink: Arc<dyn TelegramUpdateSink>,
}

pub fn router(state: Arc<WebhookState>) -> Router {
    Router::new()
        .route(WEBHOOK_PATH, post(handle_webhook))
        .route("/telegram/health", get(handle_health))
        .with_state(state)
}

async fn handle_health() -> &'static str {
    "ok"
}

async fn handle_webhook(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let provided = headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(provided.as_bytes(), state.secret_token.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "bad secret_token").into_response();
    }
    let update: Update = match serde_json::from_slice(&body) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad body").into_response(),
    };
    dispatch::route_update(&state.allowlist, state.sink.clone(), update).await;
    (StatusCode::OK, "").into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Mutex;
    use tower::ServiceExt;

    #[derive(Default)]
    struct Capture {
        seen: Mutex<Vec<i64>>,
    }
    #[async_trait::async_trait]
    impl TelegramUpdateSink for Capture {
        async fn on_update(&self, u: Update) {
            self.seen.lock().unwrap().push(u.update_id);
        }
    }

    fn build(secret: &str, allow_users: &str, sink: Arc<Capture>) -> Router {
        let state = Arc::new(WebhookState {
            secret_token: secret.into(),
            allowlist: Arc::new(Allowlist::from_csv(allow_users, "")),
            sink,
        });
        router(state)
    }

    fn dm_body(update_id: i64, user_id: i64, text: &str) -> String {
        serde_json::json!({
            "update_id": update_id,
            "message": {
                "message_id": 1,
                "from": {"id": user_id, "is_bot": false, "first_name": "T"},
                "chat": {"id": user_id, "type": "private"},
                "text": text,
            }
        })
        .to_string()
    }

    #[tokio::test]
    async fn missing_secret_header_401() {
        let sink = Arc::new(Capture::default());
        let app = build("shh", "42", sink.clone());
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .body(Body::from(dm_body(1, 42, "hi")))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn bad_secret_header_401() {
        let sink = Arc::new(Capture::default());
        let app = build("shh", "42", sink.clone());
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .header("x-telegram-bot-api-secret-token", "wrong")
            .body(Body::from(dm_body(1, 42, "hi")))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn malformed_body_400() {
        let sink = Arc::new(Capture::default());
        let app = build("shh", "42", sink.clone());
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .header("x-telegram-bot-api-secret-token", "shh")
            .body(Body::from("not json"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn allowed_dm_dispatches_to_sink() {
        let sink = Arc::new(Capture::default());
        let app = build("shh", "42", sink.clone());
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .header("x-telegram-bot-api-secret-token", "shh")
            .body(Body::from(dm_body(7, 42, "hello")))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(sink.seen.lock().unwrap().clone(), vec![7]);
    }

    #[tokio::test]
    async fn blocked_dm_200_no_dispatch() {
        let sink = Arc::new(Capture::default());
        let app = build("shh", "42", sink.clone());
        let req = Request::builder()
            .method("POST")
            .uri(WEBHOOK_PATH)
            .header("x-telegram-bot-api-secret-token", "shh")
            .body(Body::from(dm_body(8, 99, "hi")))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn health_endpoint_200() {
        let sink = Arc::new(Capture::default());
        let app = build("shh", "42", sink);
        let req = Request::builder()
            .method("GET")
            .uri("/telegram/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn constant_time_eq_matches_only_exact() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }
}
