//! LIFF (LINE Front-end Framework) chat surface.
//!
//! Surface:
//!   - `GET /liff/index.html` — serves the embedded `liff.html` with
//!     the configured LIFF ID substituted in.
//!   - `GET /liff` — alias of the above so the public URL is short
//!     enough for a LINE rich-menu deep link.
//!   - `WS  /liff/ws` — bidirectional bridge to the worker:
//!       - client → `{type:"auth", id_token, user_id, display_name}`
//!         (first frame; verified against `Allowlist::is_allowed`)
//!       - client → `{type:"prompt", text}` (subsequent)
//!       - server → `{type:"delta", text}` (AssistantTextDelta)
//!       - server → `{type:"tool", tool_name}` (ToolCallStart)
//!       - server → `{type:"done"}` (TurnDone)
//!       - server → `{type:"error", text}` / `{type:"denied", reason}`
//!
//! Auth model:
//!   The LIFF SDK on the client provides a LINE-signed `id_token`
//!   that carries the user's `sub` (LINE userId, "U..." prefix).
//!   We verify it server-side via `https://api.line.me/oauth2/v2.1/verify`
//!   before accepting any prompt. The id_token also carries `aud`
//!   (the LIFF channel id) which we accept as-is — the operator
//!   pointed thClaws at their own LIFF endpoint, so any token issued
//!   for that channel is fine.
//!
//!   Anonymous mode: when `LineConfig.direct.liff_id` is empty AND
//!   no id_token arrives, the WS still runs but the auth payload
//!   needs `user_id` explicitly. Useful for `curl`/dev smoke tests
//!   over the Tailscale tunnel. In production, leave the LIFF ID
//!   configured so the LIFF SDK is the only legitimate caller.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};

use super::allowlist::Allowlist;

const LIFF_HTML_TEMPLATE: &str = include_str!("liff.html");
const LIFF_ID_PLACEHOLDER: &str = "__LIFF_ID__";

/// Shared with `DirectServerState` so the axum router can stand up
/// the LIFF endpoints. `None` on `DirectServerState.liff` disables
/// `/liff*` entirely — the routes just aren't registered.
pub struct LiffBridge {
    pub input_tx: std::sync::mpsc::Sender<crate::shared_session::ShellInput>,
    pub events_tx: tokio::sync::broadcast::Sender<crate::shared_session::ViewEvent>,
    pub allowlist: Arc<Allowlist>,
    /// LIFF channel id rendered into the served HTML so the page-side
    /// `liff.init` call can use it. `None` ⇒ HTML still serves but
    /// runs in anonymous mode (LIFF SDK init is skipped).
    pub liff_id: Option<String>,
}

pub fn router(state: Arc<LiffBridge>) -> Router {
    Router::new()
        .route("/liff", get(serve_html))
        .route("/liff/", get(serve_html))
        .route("/liff/index.html", get(serve_html))
        .route("/liff/ws", get(ws_upgrade))
        .with_state(state)
}

async fn serve_html(State(state): State<Arc<LiffBridge>>) -> Response {
    let id = state.liff_id.as_deref().unwrap_or("");
    let body = LIFF_HTML_TEMPLATE.replace(LIFF_ID_PLACEHOLDER, id);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        // Allow the LINE-hosted CDN script to load.
        .header("X-Frame-Options", "ALLOWALL")
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<LiffBridge>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientFrame {
    #[serde(rename = "auth")]
    Auth {
        #[serde(default)]
        id_token: Option<String>,
        #[serde(default)]
        user_id: Option<String>,
        #[serde(default)]
        display_name: Option<String>,
    },
    #[serde(rename = "prompt")]
    Prompt { text: String },
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum ServerFrame<'a> {
    #[serde(rename = "auth_ok")]
    AuthOk { user_id: &'a str },
    #[serde(rename = "denied")]
    Denied { reason: &'a str },
    #[serde(rename = "delta")]
    Delta { text: &'a str },
    #[serde(rename = "tool")]
    Tool { tool_name: &'a str },
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "error")]
    Error { text: &'a str },
}

async fn handle_socket(mut socket: WebSocket, state: Arc<LiffBridge>) {
    // First frame: auth. Anything else → reject.
    let user_id = match read_auth(&mut socket, &state).await {
        Ok(u) => u,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    serde_json::to_string(&ServerFrame::Denied { reason: &e })
                        .unwrap_or_default()
                        .into(),
                ))
                .await;
            let _ = socket.close().await;
            return;
        }
    };
    let _ = socket
        .send(Message::Text(
            serde_json::to_string(&ServerFrame::AuthOk {
                user_id: &user_id,
            })
            .unwrap_or_default()
            .into(),
        ))
        .await;

    // Subscribe AFTER auth so we don't bleed events to unauthorised
    // sockets.
    let mut events_rx = state.events_tx.subscribe();

    // Forwarding loop. Two concurrent halves:
    //   * client → server: read frames, ShellInput::Line(text) on prompts
    //   * server → client: subscribe broadcast, forward AssistantTextDelta etc.
    //
    // tokio::select! drives both so either close terminates the task.
    // Split via futures::StreamExt so the two directions can be
    // borrowed independently inside select!.
    let (mut sink, mut stream) = socket.split();

    let input_tx = state.input_tx.clone();
    let user_id_for_log = user_id.clone();
    let mut in_turn = false;

    loop {
        tokio::select! {
            ws_frame = stream.next() => {
                let msg = match ws_frame {
                    Some(Ok(m)) => m,
                    _ => break, // socket closed or error
                };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => break,
                    Message::Ping(p) => {
                        let _ = sink.send(Message::Pong(p)).await;
                        continue;
                    }
                    _ => continue,
                };
                let frame: ClientFrame = match serde_json::from_str(&text) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                match frame {
                    ClientFrame::Prompt { text } => {
                        if in_turn {
                            // Reject overlapping turn for now.
                            let _ = send_server(&mut sink, ServerFrame::Error {
                                text: "Previous turn still running",
                            }).await;
                            continue;
                        }
                        if text.trim().is_empty() {
                            continue;
                        }
                        in_turn = true;
                        if input_tx
                            .send(crate::shared_session::ShellInput::Line(text))
                            .is_err()
                        {
                            let _ = send_server(&mut sink, ServerFrame::Error {
                                text: "Worker unavailable",
                            }).await;
                            in_turn = false;
                        }
                    }
                    ClientFrame::Auth { .. } => {
                        // Re-auth ignored.
                    }
                }
            }

            ev = events_rx.recv() => {
                let ev = match ev {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // We're behind; the broadcast bus dropped events.
                        let _ = send_server(&mut sink, ServerFrame::Error {
                            text: "Lagged behind event stream",
                        }).await;
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if !in_turn {
                    // Don't leak events from a terminal-driven turn
                    // back to the LIFF user.
                    continue;
                }
                match ev {
                    crate::shared_session::ViewEvent::AssistantTextDelta(s) => {
                        let _ = send_server(&mut sink, ServerFrame::Delta { text: &s }).await;
                    }
                    crate::shared_session::ViewEvent::ToolCallStart { name, .. } => {
                        let _ = send_server(&mut sink, ServerFrame::Tool {
                            tool_name: &name,
                        }).await;
                    }
                    crate::shared_session::ViewEvent::ErrorText(s) => {
                        let _ = send_server(&mut sink, ServerFrame::Error { text: &s }).await;
                    }
                    crate::shared_session::ViewEvent::TurnDone => {
                        let _ = send_server(&mut sink, ServerFrame::Done).await;
                        in_turn = false;
                    }
                    _ => {}
                }
            }
        }
    }

    eprintln!("[line/direct/liff] socket closed user={}", user_id_for_log);
}

async fn read_auth(
    socket: &mut WebSocket,
    state: &LiffBridge,
) -> Result<String, String> {
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), socket.recv())
        .await
        .map_err(|_| "auth timeout".to_string())?
        .ok_or_else(|| "socket closed".to_string())?
        .map_err(|e| format!("ws error: {e}"))?;
    let text = match first {
        Message::Text(t) => t.to_string(),
        _ => return Err("expected text auth frame".into()),
    };
    let frame: ClientFrame =
        serde_json::from_str(&text).map_err(|e| format!("bad auth frame: {e}"))?;
    let (id_token, user_id) = match frame {
        ClientFrame::Auth {
            id_token, user_id, ..
        } => (id_token, user_id),
        _ => return Err("first frame must be auth".into()),
    };
    let user_id = if let Some(token) = id_token.filter(|t| !t.is_empty()) {
        verify_id_token(&token).await?
    } else {
        user_id
            .filter(|s| s.starts_with('U'))
            .ok_or_else(|| "no id_token and no user_id".to_string())?
    };
    if !user_id_allowed(&user_id, &state.allowlist) {
        return Err(format!(
            "user {} not on allowlist — add it in thClaws → Line Connect → Allowed user IDs",
            user_id
        ));
    }
    Ok(user_id)
}

fn user_id_allowed(user_id: &str, allowlist: &Allowlist) -> bool {
    // Build a fake Source::User to reuse the same gate the webhook
    // uses. allowlist::Source isn't part of public API so we hand-
    // roll the check: any LINE user_id starting with "U" matches
    // the user CSV.
    allowlist.users.iter().any(|u| u == user_id)
}

/// Call LINE's `oauth2/v2.1/verify` endpoint. Returns the verified
/// `sub` (user id, "U..."). Network failure or 4xx → reject.
async fn verify_id_token(token: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct VerifyResp {
        sub: String,
    }
    let resp = reqwest::Client::new()
        .post("https://api.line.me/oauth2/v2.1/verify")
        .form(&[("id_token", token)])
        .send()
        .await
        .map_err(|e| format!("verify network: {e}"))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("verify rejected: {body}"));
    }
    let body: VerifyResp = resp
        .json()
        .await
        .map_err(|e| format!("verify parse: {e}"))?;
    if !body.sub.starts_with('U') {
        return Err("verify returned non-LINE sub".into());
    }
    Ok(body.sub)
}

async fn send_server<'a, S>(
    sink: &mut S,
    frame: ServerFrame<'a>,
) -> Result<(), axum::Error>
where
    S: futures::SinkExt<Message, Error = axum::Error> + Unpin,
{
    let text = serde_json::to_string(&frame).unwrap_or_default();
    sink.send(Message::Text(text.into())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_substitution_uses_configured_id() {
        let id = "1234567890-abcXYZ";
        let html = LIFF_HTML_TEMPLATE.replace(LIFF_ID_PLACEHOLDER, id);
        assert!(html.contains(id));
        assert!(!html.contains(LIFF_ID_PLACEHOLDER));
    }

    #[test]
    fn placeholder_substitution_empty_id_yields_anonymous_page() {
        let html = LIFF_HTML_TEMPLATE.replace(LIFF_ID_PLACEHOLDER, "");
        assert!(html.contains("LIFF_ID = \"\""));
    }

    #[test]
    fn allowlist_user_match_check() {
        let al = Allowlist::from_csv("U0000000000000000000000000000000a", "", "");
        assert!(user_id_allowed(
            "U0000000000000000000000000000000a",
            &al
        ));
        assert!(!user_id_allowed("Uother", &al));
    }
}
