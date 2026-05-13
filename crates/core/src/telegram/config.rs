//! Telegram bridge config.
//!
//! Bot token + (optional) webhook secret_token load lazily from
//! keychain → env at startup. Persisted file holds only non-secret
//! operational settings.

use serde::{Deserialize, Serialize};

use super::mode::TelegramMode;

pub const DEFAULT_LONG_POLL_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_WEBHOOK_HOST: &str = "0.0.0.0";
pub const DEFAULT_WEBHOOK_PORT: u16 = 8647;
pub const WEBHOOK_PATH: &str = "/telegram/webhook";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    #[serde(default)]
    pub mode: TelegramMode,

    /// Long-polling timeout in seconds. 30 is the common sweet spot
    /// (long enough to coalesce updates, short enough to detect
    /// network changes).
    #[serde(default = "default_long_poll_timeout")]
    pub long_poll_timeout_secs: u64,

    /// Webhook listen host. Only used in `Webhook` mode.
    #[serde(default = "default_webhook_host")]
    pub webhook_host: String,
    #[serde(default = "default_webhook_port")]
    pub webhook_port: u16,

    /// Optional display hint for the GUI — the URL the user pastes
    /// into `setWebhook`. thClaws does not validate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_public_url: Option<String>,

    /// Allowed Telegram user IDs (comma-separated decimal).
    #[serde(default)]
    pub allowed_users_csv: String,
    /// Allowed group / supergroup chat IDs (comma-separated decimal,
    /// usually negative numbers).
    #[serde(default)]
    pub allowed_chats_csv: String,
    /// When the bot is added to a group, require an @mention before
    /// engaging. Mirrors OpenClaw's `requireMention`. DMs ignore this.
    #[serde(default)]
    pub require_mention_in_groups: bool,
}

fn default_long_poll_timeout() -> u64 {
    DEFAULT_LONG_POLL_TIMEOUT_SECS
}
fn default_webhook_host() -> String {
    DEFAULT_WEBHOOK_HOST.into()
}
fn default_webhook_port() -> u16 {
    DEFAULT_WEBHOOK_PORT
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            mode: TelegramMode::default(),
            long_poll_timeout_secs: DEFAULT_LONG_POLL_TIMEOUT_SECS,
            webhook_host: DEFAULT_WEBHOOK_HOST.into(),
            webhook_port: DEFAULT_WEBHOOK_PORT,
            webhook_public_url: None,
            allowed_users_csv: String::new(),
            allowed_chats_csv: String::new(),
            require_mention_in_groups: true,
        }
    }
}

impl TelegramConfig {
    /// Build from env vars. `THCLAWS_TELEGRAM_*` overrides bare
    /// `TELEGRAM_*` when both are present.
    pub fn from_env() -> Self {
        let env = |a: &str, b: &str| {
            std::env::var(a)
                .or_else(|_| std::env::var(b))
                .ok()
                .filter(|s| !s.is_empty())
        };
        Self {
            mode: if std::env::var("TELEGRAM_WEBHOOK")
                .ok()
                .filter(|s| !s.is_empty())
                .is_some()
            {
                TelegramMode::Webhook
            } else {
                TelegramMode::LongPoll
            },
            long_poll_timeout_secs: std::env::var("TELEGRAM_LONG_POLL_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_LONG_POLL_TIMEOUT_SECS),
            webhook_host: env("THCLAWS_TELEGRAM_HOST", "TELEGRAM_WEBHOOK_HOST")
                .unwrap_or_else(|| DEFAULT_WEBHOOK_HOST.into()),
            webhook_port: env("THCLAWS_TELEGRAM_PORT", "TELEGRAM_WEBHOOK_PORT")
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_WEBHOOK_PORT),
            webhook_public_url: env("THCLAWS_TELEGRAM_PUBLIC_URL", "TELEGRAM_WEBHOOK_URL"),
            allowed_users_csv: std::env::var("TELEGRAM_ALLOWED_USERS").unwrap_or_default(),
            allowed_chats_csv: std::env::var("TELEGRAM_ALLOWED_CHATS").unwrap_or_default(),
            require_mention_in_groups: std::env::var("TELEGRAM_REQUIRE_MENTION_IN_GROUPS")
                .map(|v| v != "0" && v.to_lowercase() != "false")
                .unwrap_or(true),
        }
    }

    pub fn webhook_display_url(&self) -> String {
        match self.webhook_public_url.as_deref() {
            Some(u) => format!("{}{}", u.trim_end_matches('/'), WEBHOOK_PATH),
            None => format!(
                "http://{}:{}{}",
                self.webhook_host, self.webhook_port, WEBHOOK_PATH
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_long_poll() {
        let c = TelegramConfig::default();
        assert_eq!(c.mode, TelegramMode::LongPoll);
    }

    #[test]
    fn webhook_display_url_falls_back_to_host_port() {
        let c = TelegramConfig::default();
        assert_eq!(
            c.webhook_display_url(),
            "http://0.0.0.0:8647/telegram/webhook"
        );
    }

    #[test]
    fn webhook_display_url_uses_public_url() {
        let c = TelegramConfig {
            webhook_public_url: Some("https://tg.example.com/".into()),
            ..TelegramConfig::default()
        };
        assert_eq!(
            c.webhook_display_url(),
            "https://tg.example.com/telegram/webhook"
        );
    }

    #[test]
    fn require_mention_default_true() {
        assert!(TelegramConfig::default().require_mention_in_groups);
    }
}
