//! Orchestrate the self-hosted LINE bridge — load secrets, build
//! state, bind axum to the configured port, return a handle the
//! worker stashes for later cancellation.
//!
//! Secret resolution priority: process env var → OS keychain (via
//! `crate::secrets::keychain_get_raw`). Falls back to
//! `DirectLineError::MissingAccessToken` / `MissingSecret` when both
//! sources are empty — the caller surfaces this to the GUI/CLI.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use super::allowlist::Allowlist;
use super::client::DirectLineClient;
use super::config::DirectConfig;
use super::dedup::DedupStore;
use super::errors::DirectLineError;
use super::reply_store::ReplyTokenStore;
use super::server::{router, DirectEventSink, DirectServerState};
use super::slow_response::SlowResponseCache;
use crate::cancel::CancelToken;

/// Keychain account names. Match the convention used elsewhere in
/// `crate::secrets`: short, namespaced strings the GUI can write to.
pub const KEYCHAIN_ACCESS_TOKEN: &str = "line-self-hosted-access-token";
pub const KEYCHAIN_SECRET: &str = "line-self-hosted-channel-secret";

pub struct DirectHandle {
    pub cancel: CancelToken,
    pub join: tokio::task::JoinHandle<()>,
    pub client: Arc<DirectLineClient>,
    pub reply_store: Arc<ReplyTokenStore>,
    pub slow_cache: Arc<SlowResponseCache>,
    pub bind_addr: SocketAddr,
    pub threshold: Duration,
}

fn load_access_token() -> Result<String, DirectLineError> {
    if let Ok(v) = std::env::var("LINE_CHANNEL_ACCESS_TOKEN") {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    if let Some(v) = crate::secrets::keychain_get_raw(KEYCHAIN_ACCESS_TOKEN) {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    Err(DirectLineError::MissingAccessToken)
}

fn load_channel_secret() -> Result<String, DirectLineError> {
    if let Ok(v) = std::env::var("LINE_CHANNEL_SECRET") {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    if let Some(v) = crate::secrets::keychain_get_raw(KEYCHAIN_SECRET) {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    Err(DirectLineError::MissingSecret)
}

/// Bind the webhook server and return a handle. Caller is the
/// worker's `LineConnect` arm (via `bootstrap::spawn`) — it stashes
/// the handle on `state.line_session` and cancels via the token on
/// `LineDisconnect`.
pub async fn spawn(
    config: DirectConfig,
    sink: Arc<dyn DirectEventSink>,
) -> Result<DirectHandle, DirectLineError> {
    let access_token = load_access_token()?;
    let secret = load_channel_secret()?;

    let client = Arc::new(DirectLineClient::new(access_token)?);
    let reply_store = Arc::new(ReplyTokenStore::new());
    let slow_cache = Arc::new(SlowResponseCache::new());
    let dedup = Arc::new(DedupStore::new());
    let threshold = config.threshold();

    let state = Arc::new(DirectServerState {
        channel_secret: secret.into_bytes(),
        allowlist: Allowlist::from_csv(
            &config.allowed_users_csv,
            &config.allowed_groups_csv,
            &config.allowed_rooms_csv,
        ),
        dedup,
        reply_store: reply_store.clone(),
        slow_cache: slow_cache.clone(),
        sink,
    });
    let app = router(state);

    let bind_addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e: std::net::AddrParseError| DirectLineError::Http(e.to_string()))?;

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|e| DirectLineError::Http(format!("bind {bind_addr}: {e}")))?;
    let actual_addr = listener
        .local_addr()
        .map_err(|e| DirectLineError::Http(e.to_string()))?;

    let cancel = CancelToken::new();
    let cancel_for_task = cancel.clone();
    let join = tokio::spawn(async move {
        let shutdown = async move { cancel_for_task.cancelled().await };
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
        {
            eprintln!("[line/direct] server stopped: {e}");
        }
    });

    Ok(DirectHandle {
        cancel,
        join,
        client,
        reply_store,
        slow_cache,
        bind_addr: actual_addr,
        threshold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct NoopSink;

    #[async_trait::async_trait]
    impl DirectEventSink for NoopSink {
        async fn on_message(&self, _: String, _: String, _: String) {}
        async fn on_show_response_postback(&self, _: String, _: String, _: String) {}
    }

    #[tokio::test]
    async fn spawn_missing_access_token_errors() {
        // Clear both sources for both keys so the spawn fails on
        // MissingAccessToken, not "found something stale".
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINE_CHANNEL_ACCESS_TOKEN");
        std::env::remove_var("LINE_CHANNEL_SECRET");
        let config = DirectConfig::default();
        let sink: Arc<dyn DirectEventSink> = Arc::new(NoopSink);
        let r = spawn(config, sink).await;
        assert!(matches!(r, Err(DirectLineError::MissingAccessToken)));
    }

    #[tokio::test]
    async fn spawn_missing_channel_secret_errors() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LINE_CHANNEL_ACCESS_TOKEN", "token-for-test");
        std::env::remove_var("LINE_CHANNEL_SECRET");
        let config = DirectConfig::default();
        let sink: Arc<dyn DirectEventSink> = Arc::new(NoopSink);
        let r = spawn(config, sink).await;
        std::env::remove_var("LINE_CHANNEL_ACCESS_TOKEN");
        assert!(matches!(r, Err(DirectLineError::MissingSecret)));
    }

    #[tokio::test]
    async fn spawn_binds_and_can_shutdown_gracefully() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LINE_CHANNEL_ACCESS_TOKEN", "tok");
        std::env::set_var("LINE_CHANNEL_SECRET", "secret");
        // Port 0 = OS-assigned ephemeral; avoid collisions with
        // anything else listening locally during CI.
        let config = DirectConfig {
            host: "127.0.0.1".into(),
            port: 0,
            ..DirectConfig::default()
        };
        let sink: Arc<dyn DirectEventSink> = Arc::new(NoopSink);
        let handle = spawn(config, sink).await.expect("spawn ok");
        assert_eq!(handle.bind_addr.ip().to_string(), "127.0.0.1");
        assert_ne!(handle.bind_addr.port(), 0);
        // Cancel and ensure the task finishes within 5s.
        handle.cancel.cancel();
        let join = handle.join;
        let _ = tokio::time::timeout(Duration::from_secs(5), join)
            .await
            .expect("server shutdown timed out");
        std::env::remove_var("LINE_CHANNEL_ACCESS_TOKEN");
        std::env::remove_var("LINE_CHANNEL_SECRET");
    }

    // Env-variable tests need to be serialized — concurrent set/remove
    // would race across tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());
}
