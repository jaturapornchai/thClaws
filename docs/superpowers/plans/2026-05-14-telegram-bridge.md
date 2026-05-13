# Telegram Bot Bridge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Telegram Bot API bridge mirroring the `crate::line::direct` design — bot DMs + group messages flow into the same `shared_session` worker, agent replies go back via Bot API. Long-polling default + optional webhook. No relay; thClaws talks to `api.telegram.org` directly.

**Architecture:** `crates/core/src/telegram/` package. Long-poll loop (`getUpdates`) or axum webhook server (`POST /telegram/webhook`) dispatches `Update` → `TelegramSink` (cfg=gui) → `ShellInput::TelegramMessage` → worker turn → captured assistant text → `sendMessage`. Approvals use inline keyboard with `callback_query` (`tool:allow:<req_id>` / `tool:deny:<req_id>`). Allowlist on numeric user/chat IDs (tolerates `telegram:` / `tg:` prefix per openclaw convention). No `sendMessageDraft`, no streaming edits — chunked `sendMessage` like LINE.

**Tech Stack:** Rust, axum 0.7, reqwest, tokio, serde_json, async-trait, hmac (already on tree), uuid (already on tree). Reference: [openclaw/openclaw `extensions/telegram/`](https://github.com/openclaw/openclaw) and [NousResearch/hermes-agent `gateway/platforms/telegram.py`](https://github.com/NousResearch/hermes-agent).

---

## Existing state (already on branch, untracked)

| File                                 | Status | Tests |
| ------------------------------------ | ------ | ----- |
| `crates/core/src/telegram/mod.rs`    | done   | —     |
| `crates/core/src/telegram/mode.rs`   | done   | 2     |
| `crates/core/src/telegram/config.rs` | done   | 4     |
| `crates/core/src/telegram/types.rs`  | done   | 4     |
| `crates/core/src/telegram/errors.rs` | done   | —     |
| `crates/core/src/telegram/allowlist.rs` | done | 6   |
| `crates/core/src/telegram/chunk.rs`  | done   | 6     |
| `crates/core/src/telegram/client.rs` | done   | 3     |
| `crates/core/src/telegram/long_poll.rs` | done | 1   |

Missing: `webhook.rs`, `sink.rs`, `spawn.rs`, `approver.rs`, lib.rs wiring, shared_session wiring, ipc handler, GUI modal, docs.

---

## Task A: Wire `telegram` module into `lib.rs`

**Files:**
- Modify: `crates/core/src/lib.rs`

- [ ] **Step 1:** Add `pub mod telegram;` next to `pub mod line;` (search for `pub mod line;`).
- [ ] **Step 2:** `cargo test -p thclaws-core --lib telegram::` — expect 26 unit tests pass (allowlist 6 + chunk 6 + client 3 + config 4 + types 4 + mode 2 + long_poll 1).
- [ ] **Step 3:** Commit `feat(telegram): wire module into lib`.

---

## Task B: `webhook.rs` — axum server with secret_token verify

Telegram webhook verification uses a header `X-Telegram-Bot-Api-Secret-Token` (1–256 chars, A–Z/a–z/0–9/`_`/`-`) which Telegram echoes back the value the bot owner set via `setWebhook(secret_token=...)`. Not HMAC — just a shared secret in a header. Treat it like a bearer token.

**Files:**
- Create: `crates/core/src/telegram/webhook.rs`

- [ ] **Step 1: Write the server**

```rust
//! Axum webhook server for Telegram. Default off — only used when
//! the user opts into `TelegramMode::Webhook`.
//!
//! Verification: `X-Telegram-Bot-Api-Secret-Token` must match the
//! value the user passed to Telegram's `setWebhook`. Telegram echoes
//! the configured secret on every POST; mismatch → 401.

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
use super::long_poll::TelegramUpdateSink;
use super::types::Update;

pub struct WebhookState {
    pub secret_token: String,
    pub allowlist: Allowlist,
    pub sink: Arc<dyn TelegramUpdateSink>,
}

pub fn router(state: Arc<WebhookState>) -> Router {
    Router::new()
        .route(WEBHOOK_PATH, post(handle_webhook))
        .route("/telegram/health", get(handle_health))
        .with_state(state)
}

async fn handle_health() -> &'static str { "ok" }

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
    // Allowlist + sink. Mirror what long_poll does so behaviour is
    // identical across transports — same dispatch, same gating.
    super::dispatch::route_update(&state.allowlist, state.sink.clone(), update).await;
    (StatusCode::OK, "").into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) { diff |= x ^ y; }
    diff == 0
}
```

- [ ] **Step 2: Create `dispatch.rs` for shared sink-routing logic** — both `webhook.rs` and `long_poll.rs` should route updates the same way (extract sender + chat + text, run allowlist, call sink).

Create `crates/core/src/telegram/dispatch.rs`:

```rust
//! Allowlist + mention check + sink dispatch. Shared by long_poll
//! and webhook transports so policy is identical.

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
    }
    sink.on_update(update).await;
}
```

- [ ] **Step 3: Wire `dispatch` into `mod.rs` and `long_poll.rs`**

In `mod.rs` add `pub mod dispatch;` and `pub mod webhook;`.

In `long_poll.rs` replace the inline `sink.on_update(u).await;` with `dispatch::route_update(&allowlist, sink.clone(), u).await;`. Pass `Allowlist` into `run()` so both paths use the same gate. Update `run()` signature and the unit test.

- [ ] **Step 4: Write webhook tests** (mirror `line/direct/server.rs` tests — bad secret → 401, malformed → 400, allowed user → sink called, blocked user → 200 + sink not called, /telegram/health → 200).

- [ ] **Step 5:** `cargo test -p thclaws-core --lib telegram::webhook`.
- [ ] **Step 6:** Commit `feat(telegram): webhook server + dispatch`.

---

## Task C: `sink.rs` — bridge updates into `ShellInput::TelegramMessage`

**Files:**
- Create: `crates/core/src/telegram/sink.rs` (cfg=gui only)
- Modify: `crates/core/src/shared_session.rs` (add `TelegramMessage { chat_id, reply_to, text, respond }` and `TelegramConnect / TelegramDisconnect` variants)

- [ ] **Step 1: Add `ShellInput` variants** at the end of `ShellInput` enum (line ~180):

```rust
TelegramConnect(crate::telegram::TelegramConfig),
TelegramDisconnect,
TelegramMessage {
    chat_id: i64,
    reply_to_message_id: Option<i64>,
    text: String,
    respond: tokio::sync::oneshot::Sender<String>,
},
```

- [ ] **Step 2: Write `sink.rs`**

```rust
//! Bridge inbound Telegram updates into the worker's `ShellInput`
//! channel. cfg=gui because `shared_session` only exists in GUI
//! builds; CLI builds get the long-poll + webhook surface but not
//! the forwarder.

use std::sync::mpsc;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::oneshot;

use super::client::TelegramClient;
use super::long_poll::TelegramUpdateSink;
use super::types::Update;
use crate::shared_session::ShellInput;

pub struct TelegramSink {
    pub input_tx: mpsc::Sender<ShellInput>,
    pub client: Arc<TelegramClient>,
    pub bot_username: Option<String>,
    pub require_mention_in_groups: bool,
}

#[async_trait]
impl TelegramUpdateSink for TelegramSink {
    async fn on_update(&self, u: Update) {
        if let Some(msg) = u.message.or(u.channel_post) {
            let Some(text) = msg.text.clone() else { return };
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
            let msg_id = msg.message_id;
            tokio::spawn(async move {
                let answer = match rx_oneshot.await {
                    Ok(s) => s,
                    Err(_) => return,
                };
                if answer.trim().is_empty() { return; }
                if let Err(e) = client.send_message(chat_id, &answer, Some(msg_id)).await {
                    eprintln!("[telegram] reply failed chat={chat_id}: {e}");
                }
            });
        } else if let Some(cb) = u.callback_query {
            // Approver path. ack first so the spinner clears, then
            // forward decision to TelegramApprover via input_tx —
            // shared_session resolves it via state.approver downcast.
            let id = cb.id.clone();
            let client_for_ack = self.client.clone();
            tokio::spawn(async move {
                let _ = client_for_ack.answer_callback_query(&id, None).await;
            });
            if let Some(data) = cb.data {
                let _ = self
                    .input_tx
                    .send(ShellInput::TelegramCallback { data });
            }
        }
    }
}

fn is_mentioned(text: &str, entities: &[super::types::MessageEntity], username: Option<&str>) -> bool {
    let Some(u) = username else { return true; }; // unknown bot username → treat as mentioned (safer)
    let needle = format!("@{u}");
    for e in entities {
        if e.typ == "mention" {
            let start = e.offset as usize;
            let end = start.saturating_add(e.length as usize);
            let slice: String = text.chars().skip(start).take(end - start).collect();
            if slice.eq_ignore_ascii_case(&needle) { return true; }
        }
    }
    false
}

fn replied_to_bot(msg: &super::types::Message, bot_username: Option<&str>) -> bool {
    let Some(reply) = msg.reply_to_message.as_deref() else { return false };
    let Some(from) = reply.from.as_ref() else { return false };
    if from.is_bot {
        if let (Some(u), Some(bu)) = (from.username.as_deref(), bot_username) {
            u.eq_ignore_ascii_case(bu)
        } else {
            from.is_bot
        }
    } else {
        false
    }
}

fn strip_mention(text: &str, entities: &[super::types::MessageEntity], username: Option<&str>) -> String {
    let Some(u) = username else { return text.to_string() };
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
```

- [ ] **Step 3: Add `TelegramCallback { data: String }` to `ShellInput`** so callbacks travel through the worker thread (shared_session resolves them via `state.telegram_approver`).

- [ ] **Step 4: Tests** — mention detection, mention strip, reply-to-bot detection, no-username → allow all.

- [ ] **Step 5:** Commit `feat(telegram): sink — worker bridge + mention gating`.

---

## Task D: `spawn.rs` — orchestrator

**Files:**
- Create: `crates/core/src/telegram/spawn.rs`

Load `TELEGRAM_BOT_TOKEN` from env or keychain; load `TELEGRAM_WEBHOOK_SECRET_TOKEN` only when webhook mode is configured. Build `TelegramClient` once. Spawn `long_poll::run` or `webhook` axum server based on `config.mode`. Return `TelegramHandle { cancel, join, client, bind_addr: Option<SocketAddr> }`.

Keychain accounts:
- `KEYCHAIN_BOT_TOKEN = "telegram-bot-token"`
- `KEYCHAIN_WEBHOOK_SECRET = "telegram-webhook-secret-token"`

Tests:
- missing bot token → `TelegramError::MissingBotToken`
- long-poll mode + cancel → joins in <1 s
- webhook mode + 0 port → binds, cancel shuts down gracefully

- [ ] Implement, write tests, run `cargo test -p thclaws-core --lib telegram::spawn`, commit `feat(telegram): spawn orchestrator`.

---

## Task E: `TelegramApprover`

Mirror `LineApprover`. Inline-keyboard buttons with `callback_data = "tool:allow:<req_id>"` and `"tool:deny:<req_id>"`. Needs a `chat_id` to send the prompt to — store the **last allowed user chat_id** in a `Mutex<Option<i64>>` updated by the sink on every inbound message (mirrors hosted LINE which has push-anywhere; Telegram bot can send to any chat that has messaged the bot first).

If no chat ever messaged us → deny + log `[telegram] approval denied: no_chat_known`.

**Files:**
- Create: `crates/core/src/telegram/approver.rs`

Reuse `crate::line::approver::ApprovalReply` (parse_postback) — same shape `tool:allow:<id>`. Or duplicate the small parser into telegram/approver.rs to avoid a cross-module dep (cleaner).

Tests: allow, deny, timeout auto-deny, no-chat → deny.

- [ ] Implement, write tests, run `cargo test`, commit `feat(telegram): TelegramApprover`.

---

## Task F: shared_session + IPC wiring

**Files:**
- Modify: `crates/core/src/shared_session.rs`
- Modify: `crates/core/src/ipc.rs`

Handle the three new `ShellInput` variants:

1. **TelegramConnect** — `crate::telegram::spawn::spawn(cfg, sink)` → stash handle on `state.telegram_session` + swap approver to `Arc<TelegramApprover>`, mirror LineConnect's pre-mode/pre-approver stash. Broadcast `ViewEvent::TelegramStatus`.
2. **TelegramDisconnect** — cancel handle, drop, restore pre-mode/pre-approver.
3. **TelegramMessage** — subscribe to events_tx, push user prompt as `UserPrompt`, drive a turn, collect `AssistantTextDelta` until `TurnDone`, fulfil `respond`. Pattern lives in `LineMessage` handler ~line 1940+, copy that.
4. **TelegramCallback** — `state.telegram_approver.record_decision_from_postback(&data)` if approver is set.

IPC handler `telegram_setup` (mirror `line_self_hosted_setup`):
- Accepts `{ bot_token, webhook_secret_token?, allowed_users, allowed_chats, require_mention_in_groups, mode, webhook_public_url? }`.
- Writes secrets to keychain under the two `KEYCHAIN_*` accounts.
- Builds `TelegramConfig`, sends `ShellInput::TelegramConnect`.

`telegram_disconnect` IPC → sends `ShellInput::TelegramDisconnect`.

Define `ViewEvent::TelegramStatus(String)` next to `LineStatus`.

- [ ] Implement, build (`cargo check -p thclaws-core`), run `cargo test -p thclaws-core --lib`, commit `feat(telegram): shared_session + ipc wiring`.

---

## Task G: GUI sidebar + Connect modal

**Files:**
- Modify: `frontend/src/components/Sidebar/...` (find the LINE sidebar pill)
- Create: `frontend/src/components/TelegramConnectModal.tsx` (mirror `LineConnectModal`)

Form fields: bot token, mode (LongPoll / Webhook radio), public webhook URL (only if webhook), allowed user IDs (csv), allowed chat IDs (csv), require mention in groups (checkbox, default true).

Submit → `window.thclaws.ipc('telegram_setup', { ... })` → backend returns `{ ok: true, mode, bind_addr }` or `{ ok: false, error }`.

- [ ] Implement, test by hand (mock IPC), commit `feat(telegram): GUI connect modal + sidebar pill`.

---

## Task H: `docs/telegram-bridge.md` + README link

Sections: what it is, transport modes (long-poll default vs webhook), prereqs (BotFather token), setup (env vars + GUI), allowlist semantics (DM vs group), `requireMention`, security (keychain, allowlist mandatory in prod), limitations (no streaming via editMessage in v1, no topics, no inline mode).

- [ ] Write doc, link from README, commit `docs: telegram bridge guide`.

---

## Final validation

```bash
cargo fmt -p thclaws-core
cargo test -p thclaws-core --lib telegram::
cargo test -p thclaws-core --lib line::      # regression
cargo build -p thclaws-core --features gui
```

All must pass. Push branch.

### RESULT
- Telegram bot bridge: long-poll default, webhook optional. Mirrors LINE direct architecturally.
- New surface: ~12 source files + ~50 tests in `crates/core/src/telegram/`.
- No new heavy deps (axum, reqwest, hmac, serde_json all already there).

### IMPACT
- New runtime: long-poll loop (no listener) OR axum server on `:8647` (webhook mode only).
- IPC additions: `telegram_setup`, `telegram_disconnect`.
- ShellInput additions: `TelegramConnect`, `TelegramDisconnect`, `TelegramMessage`, `TelegramCallback`.

### RISK
- **Bot token = god mode.** A leaked token = anyone messaging the bot can drive thClaws. Allowlist must be non-empty in production. Setup form rejects empty allowlist when not in dev.
- **Group privacy mode.** BotFather default = privacy on → bot only sees `@mentions` and replies. The `requireMention` config matches that default; if user disables privacy via `/setprivacy`, the bot sees everything and the allowlist becomes the only gate.
- **No streaming / no editMessage.** Long agent answers arrive as one chunk after `TurnDone`. Users on slow networks see a "typing forever" feel for big tasks. Acceptable v1; can add `editMessageText` in a follow-up.

### NEXT STEP
1. Land tasks A–H sequentially on `feature/telegram-integration`.
2. Manual smoke: BotFather → token → GUI setup → DM bot → see reply.
3. Group smoke: add bot to group, `/setprivacy` off OR mention required → DM `@bot ping`.
4. PR back to main.
