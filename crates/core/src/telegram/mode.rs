//! Transport mode for the Telegram bridge. Long-polling needs no
//! infrastructure; webhook needs a public HTTPS endpoint.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TelegramMode {
    /// `GET /bot<TOKEN>/getUpdates?offset=N&timeout=30` in a loop.
    /// Zero infrastructure — thClaws hits the public Telegram API
    /// from the local machine. Default.
    #[default]
    LongPoll,
    /// Telegram POSTs updates to the configured URL. The user is
    /// responsible for the public HTTPS endpoint (reverse proxy /
    /// tunnel). Requires a `secret_token` shared with `setWebhook`.
    Webhook,
}

impl TelegramMode {
    pub fn is_webhook(self) -> bool {
        matches!(self, TelegramMode::Webhook)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_long_poll() {
        assert_eq!(TelegramMode::default(), TelegramMode::LongPoll);
    }

    #[test]
    fn serde_round_trip_snake_case() {
        assert_eq!(
            serde_json::to_string(&TelegramMode::LongPoll).unwrap(),
            "\"long_poll\""
        );
        assert_eq!(
            serde_json::to_string(&TelegramMode::Webhook).unwrap(),
            "\"webhook\""
        );
        let back: TelegramMode = serde_json::from_str("\"webhook\"").unwrap();
        assert_eq!(back, TelegramMode::Webhook);
    }
}
