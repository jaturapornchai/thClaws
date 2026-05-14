//! Non-secret operational settings for the self-hosted LINE bridge.
//!
//! Channel access token + channel secret are NOT stored here — they
//! load lazily from the OS keychain (`crate::secrets`) at startup,
//! with `LINE_CHANNEL_ACCESS_TOKEN` / `LINE_CHANNEL_SECRET` env
//! fallback. This keeps secrets out of `~/.config/thclaws/line.json`.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_HOST: &str = "0.0.0.0";
pub const DEFAULT_PORT: u16 = 8646;
pub const DEFAULT_SLOW_THRESHOLD_SECS: u64 = 45;
pub const WEBHOOK_PATH: &str = "/line/webhook";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Display-only; thClaws does not validate. Used by GUI to show
    /// the URL the user pastes into LINE Developers Console.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(default = "default_threshold_secs")]
    pub slow_response_threshold_secs: u64,
    #[serde(default)]
    pub allowed_users_csv: String,
    #[serde(default)]
    pub allowed_groups_csv: String,
    #[serde(default)]
    pub allowed_rooms_csv: String,
    /// LIFF app ID for the in-LINE WebView chat surface. Empty
    /// disables the LIFF endpoint. Set via LINE Developers Console
    /// → LIFF → "Add LIFF app" → endpoint URL pointing at
    /// `<public_url>/liff`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liff_id: Option<String>,
}

fn default_host() -> String {
    DEFAULT_HOST.into()
}
fn default_port() -> u16 {
    DEFAULT_PORT
}
fn default_threshold_secs() -> u64 {
    DEFAULT_SLOW_THRESHOLD_SECS
}

impl Default for DirectConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.into(),
            port: DEFAULT_PORT,
            public_url: None,
            slow_response_threshold_secs: DEFAULT_SLOW_THRESHOLD_SECS,
            allowed_users_csv: String::new(),
            allowed_groups_csv: String::new(),
            allowed_rooms_csv: String::new(),
            liff_id: None,
        }
    }
}

impl DirectConfig {
    /// Build from env vars; called when neither GUI form nor on-disk
    /// config supplies values. `THCLAWS_LINE_*` takes precedence over
    /// the shorter `LINE_*` name so a deploy can scope its env namespace.
    pub fn from_env() -> Self {
        let env = |a: &str, b: &str| {
            std::env::var(a)
                .or_else(|_| std::env::var(b))
                .ok()
                .filter(|s| !s.is_empty())
        };
        Self {
            host: env("THCLAWS_LINE_DIRECT_HOST", "LINE_HOST")
                .unwrap_or_else(|| DEFAULT_HOST.into()),
            port: env("THCLAWS_LINE_DIRECT_PORT", "LINE_PORT")
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            public_url: env("THCLAWS_LINE_PUBLIC_URL", "LINE_PUBLIC_URL"),
            slow_response_threshold_secs: std::env::var("THCLAWS_LINE_SLOW_RESPONSE_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_SLOW_THRESHOLD_SECS),
            allowed_users_csv: std::env::var("LINE_ALLOWED_USERS").unwrap_or_default(),
            allowed_groups_csv: std::env::var("LINE_ALLOWED_GROUPS").unwrap_or_default(),
            allowed_rooms_csv: std::env::var("LINE_ALLOWED_ROOMS").unwrap_or_default(),
            liff_id: env("THCLAWS_LINE_LIFF_ID", "LINE_LIFF_ID"),
        }
    }

    pub fn threshold(&self) -> Duration {
        Duration::from_secs(self.slow_response_threshold_secs)
    }

    pub fn webhook_display_url(&self) -> String {
        match self.public_url.as_deref() {
            Some(u) => format!("{}{}", u.trim_end_matches('/'), WEBHOOK_PATH),
            None => format!("http://{}:{}{}", self.host, self.port, WEBHOOK_PATH),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_threshold_is_45s() {
        let c = DirectConfig::default();
        assert_eq!(c.threshold().as_secs(), 45);
    }

    #[test]
    fn webhook_url_with_public_url_uses_it() {
        let c = DirectConfig {
            public_url: Some("https://line.example.com/".into()),
            ..DirectConfig::default()
        };
        assert_eq!(c.webhook_display_url(), "https://line.example.com/line/webhook");
    }

    #[test]
    fn webhook_url_without_public_falls_back_to_host_port() {
        let c = DirectConfig {
            host: "0.0.0.0".into(),
            port: 8646,
            ..DirectConfig::default()
        };
        assert_eq!(c.webhook_display_url(), "http://0.0.0.0:8646/line/webhook");
    }

    #[test]
    fn from_env_parses_thclaws_prefix() {
        // Use a port unique enough to not collide with other env-dependent tests.
        std::env::set_var("THCLAWS_LINE_DIRECT_PORT", "19999");
        let c = DirectConfig::from_env();
        std::env::remove_var("THCLAWS_LINE_DIRECT_PORT");
        assert_eq!(c.port, 19999);
    }
}
