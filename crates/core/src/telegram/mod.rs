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
pub mod dedup;
pub mod dispatch;
pub mod errors;
pub mod long_poll;
pub mod mode;
pub mod spawn;
pub mod types;
pub mod webhook;

// Task-local chat-id used by per-chat approval routing. The
// `ShellInput::TelegramMessage` worker arm enters a scope keyed by
// the message's `chat_id`; `TelegramApprover::approve` reads from
// this scope so the approval prompt lands in the chat that
// originated the tool call (not whichever chat was last active
// across all users). Falls back to the global `last_chat_id` when
// the approval is fired outside a Telegram-driven turn (e.g. GUI).
tokio::task_local! {
    pub static CURRENT_CHAT_ID: i64;
}

// `sink` bridges into `shared_session::ShellInput` which is gui-gated
// (CLI builds have no worker channel). The webhook + long-poll
// transports remain available in CLI builds for testing.
#[cfg(feature = "gui")]
pub mod sink;

pub use config::TelegramConfig;
pub use errors::TelegramError;
pub use mode::TelegramMode;
