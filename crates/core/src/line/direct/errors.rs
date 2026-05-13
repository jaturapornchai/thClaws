//! Errors specific to the self-hosted LINE bridge.
//!
//! `NoValidReplyToken` is the central refusal — it stands in
//! everywhere Push API would have been used in a Push-permitted
//! design. Returning this error (instead of falling back to Push)
//! is the load-bearing constraint of self_hosted mode.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DirectLineError {
    #[error("no valid reply token for this chat (expired, used, or never received)")]
    NoValidReplyToken,
    #[error("LINE Reply API returned {status}: {body}")]
    ReplyApi { status: u16, body: String },
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("missing channel_access_token (set LINE_CHANNEL_ACCESS_TOKEN or keychain)")]
    MissingAccessToken,
    #[error("missing channel_secret (set LINE_CHANNEL_SECRET or keychain)")]
    MissingSecret,
}
