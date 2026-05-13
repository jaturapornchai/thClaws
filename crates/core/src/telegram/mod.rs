//! Telegram Bot API bridge. Long-polling default + optional webhook.
//!
//! Architecture mirrors `crate::line::direct` but is materially simpler:
//!   - No reply token — sendMessage works whenever you have a chat_id.
//!   - No slow-response postback button — replies can arrive at any time.
//!   - Background notifications + approvals via inline keyboard work
//!     by design (Telegram's Bot API allows unsolicited messages to
//!     any chat the bot has been invited to / a user has DM'd).
//!
//! Default = long-polling: thClaws stays local-only, no public URL
//! needed. Webhook = optional for production deploys that want lower
//! latency (requires public HTTPS + setWebhook with secret_token).

pub mod allowlist;
pub mod approver;
pub mod chunk;
pub mod client;
pub mod config;
pub mod dispatch;
pub mod errors;
pub mod long_poll;
pub mod mode;
pub mod spawn;
pub mod types;
pub mod webhook;

// `sink` bridges into `shared_session::ShellInput` which is gui-gated
// (CLI builds have no worker channel). The webhook + long-poll
// transports remain available in CLI builds for testing.
#[cfg(feature = "gui")]
pub mod sink;

pub use config::TelegramConfig;
pub use errors::TelegramError;
pub use mode::TelegramMode;
