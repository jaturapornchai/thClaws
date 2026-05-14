//! Self-hosted LINE bridge — user owns the LINE OA + webhook;
//! thClaws verifies signatures and calls LINE Reply API directly.
//! No Push API anywhere in this module — by design.

pub mod allowlist;
pub mod chunk;
pub mod client;
pub mod config;
pub mod dedup;
pub mod errors;
pub mod liff;
pub mod reply_store;
pub mod server;
pub mod signature;
// `sink` bridges into `shared_session` which is gui-gated, so this
// module is gui-gated too. CLI builds compile direct webhook + client
// but not the worker forwarder.
#[cfg(feature = "gui")]
pub mod sink;
pub mod slow_response;
pub mod spawn;
pub mod types;
