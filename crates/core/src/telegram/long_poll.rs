//! Long-polling loop. Issues `getUpdates` in a loop with a tracking
//! offset, dispatches each `Update` to the supplied sink, exits on
//! cancel.
//!
//! Errors are logged + back off; the loop never panics out so a
//! transient `429 Too Many Requests` doesn't kill the bridge.

use std::sync::Arc;
use std::time::Duration;

use super::allowlist::Allowlist;
use super::client::TelegramClient;
use super::dispatch;
use super::errors::TelegramError;
use super::types::Update;
use crate::cancel::CancelToken;

#[async_trait::async_trait]
pub trait TelegramUpdateSink: Send + Sync + 'static {
    async fn on_update(&self, update: Update);
}

const ALLOWED_UPDATE_TYPES: &[&str] = &["message", "channel_post", "callback_query"];

pub async fn run(
    client: Arc<TelegramClient>,
    sink: Arc<dyn TelegramUpdateSink>,
    allowlist: Arc<Allowlist>,
    timeout_secs: u64,
    cancel: CancelToken,
) -> Result<(), TelegramError> {
    let mut offset: i64 = 0;
    let mut backoff = Duration::from_millis(500);
    let max_backoff = Duration::from_secs(60);

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let fetch = client.get_updates(offset, timeout_secs, ALLOWED_UPDATE_TYPES);
        let updates = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            r = fetch => match r {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("[telegram/long_poll] getUpdates failed: {e}; backoff {backoff:?}");
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }
            }
        };
        backoff = Duration::from_millis(500);
        for u in updates {
            offset = offset.max(u.update_id + 1);
            dispatch::route_update(&allowlist, sink.clone(), u).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct CapturingSink {
        seen: Mutex<Vec<i64>>,
    }
    #[async_trait::async_trait]
    impl TelegramUpdateSink for CapturingSink {
        async fn on_update(&self, u: Update) {
            self.seen.lock().unwrap().push(u.update_id);
        }
    }

    #[tokio::test]
    async fn cancel_before_first_poll_returns_immediately() {
        let client = Arc::new(TelegramClient::new("123:abc".into()).unwrap());
        let sink = Arc::new(CapturingSink {
            seen: Mutex::new(vec![]),
        });
        let allowlist = Arc::new(Allowlist::default());
        let cancel = CancelToken::new();
        cancel.cancel();
        let r = run(client, sink, allowlist, 30, cancel).await;
        assert!(r.is_ok());
    }
}
