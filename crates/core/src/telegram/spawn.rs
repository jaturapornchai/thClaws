//! Orchestrate the Telegram bridge transport — load secrets, build
//! state, spawn either the long-poll loop or the axum webhook
//! server, return a handle the worker stashes for cancellation.
//!
//! Secret resolution priority (same convention as line/direct):
//!   process env var → OS keychain → error.
//!
//! Env vars:
//!   - `TELEGRAM_BOT_TOKEN` (required)
//!   - `TELEGRAM_WEBHOOK_SECRET_TOKEN` (required only when mode=webhook)
//!
//! Keychain accounts:
//!   - `telegram-bot-token`
//!   - `telegram-webhook-secret-token`

use std::net::SocketAddr;
use std::sync::Arc;

use super::allowlist::Allowlist;
use super::client::TelegramClient;
use super::config::TelegramConfig;
use super::errors::TelegramError;
use super::long_poll::{self, TelegramUpdateSink};
use super::mode::TelegramMode;
use crate::cancel::CancelToken;

pub const KEYCHAIN_BOT_TOKEN: &str = "telegram-bot-token";
pub const KEYCHAIN_WEBHOOK_SECRET: &str = "telegram-webhook-secret-token";

pub struct TelegramHandle {
    pub cancel: CancelToken,
    pub join: tokio::task::JoinHandle<()>,
    pub client: Arc<TelegramClient>,
    pub allowlist: Arc<Allowlist>,
    pub mode: TelegramMode,
    /// Some(addr) only when mode = Webhook. None for long-poll.
    pub bind_addr: Option<SocketAddr>,
    /// Cached lower-case bot username from `getMe` (used by the sink
    /// for mention detection). None if `getMe` failed; the sink
    /// degrades to forwarding all allowed group messages.
    pub bot_username: Option<String>,
}

fn load_bot_token() -> Result<String, TelegramError> {
    if let Ok(v) = std::env::var("TELEGRAM_BOT_TOKEN") {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    if let Some(v) = crate::secrets::keychain_get_raw(KEYCHAIN_BOT_TOKEN) {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    Err(TelegramError::MissingBotToken)
}

fn load_webhook_secret() -> Result<String, TelegramError> {
    if let Ok(v) = std::env::var("TELEGRAM_WEBHOOK_SECRET_TOKEN") {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    if let Some(v) = crate::secrets::keychain_get_raw(KEYCHAIN_WEBHOOK_SECRET) {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    Err(TelegramError::MissingWebhookSecretToken)
}

/// `getMe` to learn the bot's @username. Best-effort: a transient
/// network failure returns None so the bridge still comes up; the
/// sink falls back to "forward everything allowed" for groups.
async fn fetch_bot_username(client: &TelegramClient) -> Option<String> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct GetMe {
        ok: bool,
        #[serde(default)]
        result: Option<MeBody>,
    }
    #[derive(Deserialize)]
    struct MeBody {
        #[serde(default)]
        username: Option<String>,
    }

    let url = client.endpoint("getMe");
    let resp = match reqwest::Client::new().get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[telegram] getMe network failed: {e}");
            return None;
        }
    };
    let text = match resp.text().await {
        Ok(t) => t,
        Err(_) => return None,
    };
    let parsed: GetMe = serde_json::from_str(&text).ok()?;
    if !parsed.ok {
        return None;
    }
    parsed.result.and_then(|m| m.username).map(|s| s.to_lowercase())
}

pub async fn spawn(
    config: TelegramConfig,
    sink: Arc<dyn TelegramUpdateSink>,
) -> Result<TelegramHandle, TelegramError> {
    let bot_token = load_bot_token()?;
    let client = Arc::new(TelegramClient::new(bot_token)?);
    let bot_username = fetch_bot_username(&client).await;

    let allowlist = Arc::new(Allowlist::from_csv(
        &config.allowed_users_csv,
        &config.allowed_chats_csv,
    ));

    let cancel = CancelToken::new();
    match config.mode {
        TelegramMode::LongPoll => {
            // Long-poll mode actively drives `getUpdates`. Make sure
            // there's no stale webhook on Telegram's side — otherwise
            // Telegram refuses `getUpdates` with 409 Conflict. Best-
            // effort.
            if let Err(e) = client.delete_webhook().await {
                eprintln!("[telegram] deleteWebhook (pre-long-poll) warning: {e}");
            }
            let cancel_for_loop = cancel.clone();
            let client_for_loop = client.clone();
            let allow_for_loop = allowlist.clone();
            let sink_for_loop = sink.clone();
            let timeout_secs = config.long_poll_timeout_secs;
            let join = tokio::spawn(async move {
                if let Err(e) = long_poll::run(
                    client_for_loop,
                    sink_for_loop,
                    allow_for_loop,
                    timeout_secs,
                    cancel_for_loop,
                )
                .await
                {
                    eprintln!("[telegram] long_poll loop ended: {e}");
                }
            });
            Ok(TelegramHandle {
                cancel,
                join,
                client,
                allowlist,
                mode: TelegramMode::LongPoll,
                bind_addr: None,
                bot_username,
            })
        }
        TelegramMode::Webhook => {
            let secret_token = load_webhook_secret()?;
            let state = Arc::new(super::webhook::WebhookState {
                secret_token,
                allowlist: allowlist.clone(),
                sink,
            });
            let app = super::webhook::router(state);
            let bind_str = format!("{}:{}", config.webhook_host, config.webhook_port);
            let parsed: SocketAddr = bind_str
                .parse()
                .map_err(|e: std::net::AddrParseError| TelegramError::Http(e.to_string()))?;
            let listener = tokio::net::TcpListener::bind(parsed)
                .await
                .map_err(|e| TelegramError::Http(format!("bind {bind_str}: {e}")))?;
            let actual_addr = listener
                .local_addr()
                .map_err(|e| TelegramError::Http(e.to_string()))?;
            let cancel_for_task = cancel.clone();
            let join = tokio::spawn(async move {
                let shutdown = async move { cancel_for_task.cancelled().await };
                if let Err(e) = axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown)
                    .await
                {
                    eprintln!("[telegram] webhook server stopped: {e}");
                }
            });
            Ok(TelegramHandle {
                cancel,
                join,
                client,
                allowlist,
                mode: TelegramMode::Webhook,
                bind_addr: Some(actual_addr),
                bot_username,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::types::Update;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct NoopSink {
        _seen: Mutex<Vec<i64>>,
    }
    #[async_trait::async_trait]
    impl TelegramUpdateSink for NoopSink {
        async fn on_update(&self, _u: Update) {}
    }

    // Env-variable tests must serialize — concurrent get/set races.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn spawn_missing_bot_token_errors() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        std::env::remove_var("TELEGRAM_WEBHOOK_SECRET_TOKEN");
        let cfg = TelegramConfig::default();
        let sink: Arc<dyn TelegramUpdateSink> = Arc::new(NoopSink::default());
        let r = spawn(cfg, sink).await;
        assert!(matches!(r, Err(TelegramError::MissingBotToken)));
    }

    #[tokio::test]
    async fn spawn_webhook_mode_missing_secret_errors() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("TELEGRAM_BOT_TOKEN", "fake-token");
        std::env::remove_var("TELEGRAM_WEBHOOK_SECRET_TOKEN");
        let cfg = TelegramConfig {
            mode: TelegramMode::Webhook,
            webhook_port: 0,
            ..TelegramConfig::default()
        };
        let sink: Arc<dyn TelegramUpdateSink> = Arc::new(NoopSink::default());
        let r = spawn(cfg, sink).await;
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        assert!(matches!(r, Err(TelegramError::MissingWebhookSecretToken)));
    }

    #[tokio::test]
    async fn spawn_webhook_binds_and_cancels() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("TELEGRAM_BOT_TOKEN", "fake-token");
        std::env::set_var("TELEGRAM_WEBHOOK_SECRET_TOKEN", "shh");
        let cfg = TelegramConfig {
            mode: TelegramMode::Webhook,
            webhook_host: "127.0.0.1".into(),
            webhook_port: 0, // OS-assigned
            ..TelegramConfig::default()
        };
        let sink: Arc<dyn TelegramUpdateSink> = Arc::new(NoopSink::default());
        let handle = spawn(cfg, sink).await.expect("spawn ok");
        assert_eq!(handle.mode, TelegramMode::Webhook);
        let addr = handle.bind_addr.expect("webhook addr");
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert_ne!(addr.port(), 0);
        handle.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle.join)
            .await
            .expect("server shutdown timed out");
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        std::env::remove_var("TELEGRAM_WEBHOOK_SECRET_TOKEN");
    }
}
