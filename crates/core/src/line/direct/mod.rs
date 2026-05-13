//! Self-hosted LINE bridge — user owns the LINE OA + webhook;
//! thClaws verifies signatures and calls LINE Reply API directly.
//! No Push API anywhere in this module — by design.

pub mod allowlist;
pub mod chunk;
pub mod dedup;
pub mod reply_store;
pub mod signature;
