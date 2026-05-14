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
    /// Explicit opt-in for the dev / single-owner "anyone can DM"
    /// posture. When `true` AND both allowlists are empty, the
    /// bridge forwards every update. Default `false` → empty
    /// allowlists deny all (P1 fail-closed).
    #[serde(default)]
    pub allow_open_mode: bool,
    /// Explicit opt-in: in groups, sender doesn't have to be on the
    /// user allowlist (chat-only gating). Default `false` → group
    /// auth requires BOTH chat_id AND sender user_id (P4 fail-closed).
    #[serde(default)]
    pub allow_any_user_in_group: bool,
    /// Skip per-tool approval gating entirely while this bridge is
    /// connected. When `true`, the worker leaves the permission mode
    /// at `Auto` instead of swapping to `LineGated`, so every tool
    /// call runs without prompting Telegram. Use for single-owner
    /// trusted bots.
    #[serde(default)]
    pub auto_approve_all: bool,
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
            allow_open_mode: false,
            allow_any_user_in_group: false,
            auto_approve_all: false,
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
            allow_open_mode: std::env::var("TELEGRAM_ALLOW_OPEN_MODE")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            allow_any_user_in_group: std::env::var("TELEGRAM_ALLOW_ANY_USER_IN_GROUP")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            auto_approve_all: std::env::var("TELEGRAM_AUTO_APPROVE_ALL")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
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

    /// Canonical on-disk path. None only on truly headless setups
    /// where no home dir is resolvable.
    pub fn path() -> Option<std::path::PathBuf> {
        crate::util::home_dir()
            .map(|h| h.join(".config").join("thclaws").join("telegram.json"))
    }

    /// Load saved (non-secret) config. Returns `Ok(None)` when the
    /// file is absent. Secrets live only in keychain / env and are
    /// never written here (P7 constraint).
    pub fn load() -> std::io::Result<Option<Self>> {
        let Some(path) = Self::path() else {
            return Ok(None);
        };
        match std::fs::read_to_string(&path) {
            Ok(body) => match serde_json::from_str::<Self>(&body) {
                Ok(cfg) => Ok(Some(cfg)),
                Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Persist atomically (write-then-rename). Skips writing if no
    /// home dir is resolvable (silent no-op for CI / sandbox builds).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &path)
    }

    /// Idempotent — missing file = success.
    pub fn delete() -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
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
