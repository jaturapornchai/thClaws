//! Telegram bridge errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TelegramError {
    #[error("missing bot token (set TELEGRAM_BOT_TOKEN or keychain)")]
    MissingBotToken,
    #[error("missing webhook secret_token (set TELEGRAM_WEBHOOK_SECRET_TOKEN or keychain)")]
    MissingWebhookSecretToken,
    #[error("Telegram API returned {status}: {body}")]
    Api { status: u16, body: String },
    #[error("Telegram API ok=false: {description}")]
    NotOk { description: String },
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("invalid config: {0}")]
    Config(String),
}
