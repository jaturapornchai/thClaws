# LINE Self-Hosted Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a second LINE bridge mode (`self_hosted`) where the user runs their own LINE OA + webhook, alongside the existing relay-based `hosted` mode. Reply API only — no Push API anywhere.

**Architecture:** New `LineMode { Hosted, SelfHosted }` enum on `LineConfig`. Existing `line/` modules (protocol, session, filter, approver core) remain untouched for hosted parity. New `line/direct/` submodule owns webhook server, signature verify, allowlist, dedup, reply-token store, slow-response postback cache, and a Reply-API-only `DirectLineClient`. `bootstrap::spawn()` branches on mode. `LineApprover` gains a self_hosted defer path that denies when no valid `replyToken` exists. GUI modal extends with mode toggle.

**Tech Stack:** Rust + axum 0.8 (already in repo) + hmac 0.12 (new dep) + sha2 0.10 + base64 0.22 + reqwest 0.12 (all existing) · React 19 frontend (Vite, Tailwind).

**Decisions captured from clarification (do not re-litigate):**
1. Layout: submodule `crates/core/src/line/direct/` + `line/mode.rs`; reuse `LineSession`/`LineFilter` traits.
2. GUI: extend existing `LineConnectModal.tsx` with hosted/self_hosted toggle.
3. Secrets: OS keychain primary (`secrets.rs` already wired) · `.env` fallback when `THCLAWS_DISABLE_KEYCHAIN=1`.
4. Approver in self_hosted: deny-safe when no valid `replyToken` (deferred path).
5. Public exposure: thClaws binds port only. User owns subdomain/reverse-proxy. `THCLAWS_LINE_PUBLIC_URL` is display-only.
6. Docs: new `docs/line-self-hosted.md` (don't bloat README).
7. Dependency: add `hmac = "0.12"` (audited, RustCrypto, ~10 KB).

---

## File structure

### Create

```
crates/core/src/line/mode.rs                     — LineMode enum + serde
crates/core/src/line/direct/mod.rs               — re-exports
crates/core/src/line/direct/config.rs            — DirectConfig struct
crates/core/src/line/direct/errors.rs            — DirectLineError enum
crates/core/src/line/direct/signature.rs         — verify HMAC-SHA256
crates/core/src/line/direct/allowlist.rs         — Allowlist (users/groups/rooms)
crates/core/src/line/direct/dedup.rs             — bounded LRU webhookEventId
crates/core/src/line/direct/reply_store.rs       — ReplyTokenStore 50s TTL
crates/core/src/line/direct/slow_response.rs     — SlowResponseCache + state enum
crates/core/src/line/direct/chunk.rs             — split_for_line()
crates/core/src/line/direct/client.rs            — DirectLineClient (Reply API only)
crates/core/src/line/direct/server.rs            — axum router + handler
crates/core/src/line/direct/spawn.rs             — task spawn entry
crates/core/src/line/direct/types.rs             — wire shapes (Event, Source, Message…)
docs/line-self-hosted.md                          — setup guide
```

### Modify

```
crates/core/Cargo.toml                            — add hmac 0.12
crates/core/src/line/mod.rs                       — declare mode + direct submodules
crates/core/src/line/config.rs                    — add mode + direct: Option<DirectConfig>
crates/core/src/line/bootstrap.rs                 — branch spawn on mode
crates/core/src/line/approver.rs                  — add SelfHostedDeferringApprover path
crates/core/src/shared_session.rs                 — wire LineMessage for self_hosted slow-response cache
crates/core/src/ipc.rs                            — extend line_pair handler for self_hosted form
frontend/src/components/LineConnectModal.tsx      — mode toggle + self_hosted form
frontend/src/components/Sidebar.tsx               — pill respects mode field
```

### Untouched (hosted parity guarantees)

```
crates/core/src/line/protocol.rs                  — WsEnvelope shapes frozen
crates/core/src/line/client.rs                    — hosted relay client unchanged
crates/core/src/line/session.rs                   — hosted LineSession unchanged
crates/core/src/line/filter.rs                    — filter_for_line() reused as-is
```

---

## Task ordering rationale

Tasks 1–3 land config + mode enum + Cargo dep — non-functional, zero risk to hosted. Tasks 4–10 build pure-Rust units bottom-up (signature → allowlist → dedup → reply store → chunk → slow response → client) each with full tests, no integration yet. Task 11 wires axum server. Tasks 12–14 wire spawn + shared_session + approver. Tasks 15–17 finish GUI + docs + final integration test.

This order means: every task either fully passes tests in isolation OR is a thin wiring layer over already-tested pieces. Hosted mode never sees broken state.

---

## Reuse policy (DRY)

When a task asks for code that mirrors an existing pattern in hosted mode (e.g. config save/load atomic rename), copy the **idea**, not the code — adapt to direct's needs. Don't introduce shared traits across modes prematurely (YAGNI); revisit only if Phase-2 features land.


---

## Task 1: Add `hmac` dependency

**Files:**
- Modify: `crates/core/Cargo.toml` — `[dependencies]` section

- [ ] **Step 1: Add the dep**

In `crates/core/Cargo.toml` under `[dependencies]` (alphabetical block where `sha2` already lives):

```toml
hmac = "0.12"
```

- [ ] **Step 2: Verify it builds**

Run: `cargo check -p thclaws-core`
Expected: build succeeds, new dep resolved.

- [ ] **Step 3: Commit**

```bash
git add crates/core/Cargo.toml Cargo.lock
git commit -m "chore: add hmac 0.12 for self-hosted LINE webhook signature verify"
```

---

## Task 2: Add `LineMode` enum

**Files:**
- Create: `crates/core/src/line/mode.rs`
- Modify: `crates/core/src/line/mod.rs` (add `pub mod mode;` + re-export)

- [ ] **Step 1: Write the failing test**

Create `crates/core/src/line/mode.rs`:

```rust
//! `LineMode` — selects between the relay-mediated "hosted" bridge
//! and the user-owned "self_hosted" webhook bridge. Stored on
//! `LineConfig`, used by `bootstrap::spawn` to pick the runtime.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LineMode {
    /// Existing flow — pair via thClaws relay, connect WS to
    /// `wss://line.thclaws.ai/ws`. Replies go through the relay,
    /// which proxies to LINE.
    #[default]
    Hosted,
    /// User runs their own LINE OA + reverse-proxied webhook into
    /// thClaws's local port. thClaws verifies LINE signatures and
    /// calls LINE Reply API directly. No Push API ever.
    SelfHosted,
}

impl LineMode {
    pub fn is_self_hosted(self) -> bool {
        matches!(self, LineMode::SelfHosted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_hosted_for_back_compat() {
        assert_eq!(LineMode::default(), LineMode::Hosted);
        assert!(!LineMode::default().is_self_hosted());
    }

    #[test]
    fn serde_round_trip_snake_case() {
        let h = serde_json::to_string(&LineMode::Hosted).unwrap();
        assert_eq!(h, "\"hosted\"");
        let s = serde_json::to_string(&LineMode::SelfHosted).unwrap();
        assert_eq!(s, "\"self_hosted\"");
        let back: LineMode = serde_json::from_str("\"self_hosted\"").unwrap();
        assert_eq!(back, LineMode::SelfHosted);
    }

    #[test]
    fn unknown_mode_string_fails_parse() {
        let r: Result<LineMode, _> = serde_json::from_str("\"foo\"");
        assert!(r.is_err());
    }
}
```

- [ ] **Step 2: Wire the module**

In `crates/core/src/line/mod.rs`, after the existing `pub mod config;` line, add:

```rust
pub mod mode;
pub use mode::LineMode;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p thclaws-core --lib line::mode`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/line/mode.rs crates/core/src/line/mod.rs
git commit -m "feat(line): add LineMode enum (hosted | self_hosted)"
```

---

## Task 3: Extend `LineConfig` with `mode` (back-compat)

**Files:**
- Modify: `crates/core/src/line/config.rs`

Goal: add `mode: LineMode` defaulting to `Hosted` so existing `line.json` files on disk parse unchanged.

- [ ] **Step 1: Write the failing test**

Append to `crates/core/src/line/config.rs` `mod tests`:

```rust
#[test]
fn legacy_line_json_without_mode_parses_as_hosted() {
    // Existing installs have line.json that predates the field.
    let legacy = r#"{"binding_token":"abc"}"#;
    let cfg: LineConfig = serde_json::from_str(legacy).unwrap();
    assert_eq!(cfg.mode, crate::line::mode::LineMode::Hosted);
}

#[test]
fn self_hosted_round_trips() {
    let cfg = LineConfig {
        binding_token: String::new(),
        mode: crate::line::mode::LineMode::SelfHosted,
        ..Default::default()
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: LineConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.mode, crate::line::mode::LineMode::SelfHosted);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p thclaws-core --lib line::config::tests::legacy_line_json_without_mode_parses_as_hosted`
Expected: COMPILE ERROR — `mode` field doesn't exist.

- [ ] **Step 3: Add the field**

In `crates/core/src/line/config.rs`, the `LineConfig` struct (around line 36):

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LineConfig {
    /// HS256 JWT issued by the relay's `POST /pair`. Empty in self_hosted mode.
    pub binding_token: String,
    /// Which bridge runtime to use. Defaults to `Hosted` so existing
    /// on-disk configs parse unchanged.
    #[serde(default)]
    pub mode: crate::line::mode::LineMode,
    /// Override URL for the relay (hosted mode only).
    #[serde(default)]
    pub server_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p thclaws-core --lib line::config`
Expected: all tests pass, including 2 new ones.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/line/config.rs
git commit -m "feat(line): LineConfig.mode field (default Hosted for back-compat)"
```

---

## Task 4: Signature verification (HMAC-SHA256)

**Files:**
- Create: `crates/core/src/line/direct/signature.rs`
- Create: `crates/core/src/line/direct/mod.rs` (declare submodule)
- Modify: `crates/core/src/line/mod.rs` (add `pub mod direct;`)

- [ ] **Step 1: Write the failing test**

Create `crates/core/src/line/direct/signature.rs`:

```rust
//! LINE webhook signature verification.
//!
//! LINE Platform signs every webhook delivery with
//! X-Line-Signature: base64(HMAC-SHA256(channel_secret, raw_body)).
//! Constant-time compare so a forged body cannot be probed via timing.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn verify(body: &[u8], header_value: &str, secret: &[u8]) -> bool {
    if secret.is_empty() {
        return false;
    }
    let expected = match base64::engine::general_purpose::STANDARD.decode(header_value) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn sign(body: &[u8], secret: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
    }

    #[test]
    fn verify_accepts_valid_signature() {
        let secret = b"channel-secret-abc";
        let body = br#"{"events":[]}"#;
        let sig = sign(body, secret);
        assert!(verify(body, &sig, secret));
    }

    #[test]
    fn verify_rejects_wrong_secret() {
        let body = br#"{"events":[]}"#;
        let sig = sign(body, b"real");
        assert!(!verify(body, &sig, b"forged"));
    }

    #[test]
    fn verify_rejects_tampered_body() {
        let secret = b"s";
        let sig = sign(br#"{"a":1}"#, secret);
        assert!(!verify(br#"{"a":2}"#, &sig, secret));
    }

    #[test]
    fn verify_rejects_non_base64_header() {
        assert!(!verify(b"body", "not::valid::base64::!!!", b"s"));
    }

    #[test]
    fn verify_rejects_empty_secret() {
        assert!(!verify(b"x", "anything", b""));
    }

    #[test]
    fn verify_rejects_empty_header() {
        assert!(!verify(b"x", "", b"s"));
    }
}
```

- [ ] **Step 2: Wire the submodule**

Create `crates/core/src/line/direct/mod.rs`:

```rust
//! Self-hosted LINE bridge — user owns the LINE OA + webhook;
//! thClaws verifies signatures and calls LINE Reply API directly.
//! No Push API anywhere in this module — by design.

pub mod signature;
```

In `crates/core/src/line/mod.rs`, after `pub mod mode;`:

```rust
pub mod direct;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p thclaws-core --lib line::direct::signature`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/line/direct/ crates/core/src/line/mod.rs
git commit -m "feat(line/direct): HMAC-SHA256 signature verify"
```

---

## Task 5: Allowlist

**Files:**
- Create: `crates/core/src/line/direct/allowlist.rs`
- Modify: `crates/core/src/line/direct/mod.rs` (add `pub mod allowlist;`)

- [ ] **Step 1: Write the failing test + impl**

Create `crates/core/src/line/direct/allowlist.rs`:

```rust
//! Per-source allowlist. Events whose source id is not in the
//! configured set are rejected before reaching agent logic. Empty
//! set for a type means "deny all of that type" — fail-closed.

use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    pub users: HashSet<String>,
    pub groups: HashSet<String>,
    pub rooms: HashSet<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Source {
    User {
        #[serde(rename = "userId", default)]
        user_id: Option<String>,
    },
    Group {
        #[serde(rename = "groupId", default)]
        group_id: Option<String>,
        #[serde(rename = "userId", default)]
        user_id: Option<String>,
    },
    Room {
        #[serde(rename = "roomId", default)]
        room_id: Option<String>,
        #[serde(rename = "userId", default)]
        user_id: Option<String>,
    },
}

impl Source {
    pub fn chat_id(&self) -> Option<&str> {
        match self {
            Source::User { user_id } => user_id.as_deref(),
            Source::Group { group_id, .. } => group_id.as_deref(),
            Source::Room { room_id, .. } => room_id.as_deref(),
        }
    }
}

impl Allowlist {
    pub fn from_csv(users: &str, groups: &str, rooms: &str) -> Self {
        fn parse(s: &str) -> HashSet<String> {
            s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
        }
        Self { users: parse(users), groups: parse(groups), rooms: parse(rooms) }
    }

    pub fn is_allowed(&self, source: &Source) -> bool {
        match source {
            Source::User { user_id: Some(id) } => self.users.contains(id),
            Source::Group { group_id: Some(id), .. } => self.groups.contains(id),
            Source::Room { room_id: Some(id), .. } => self.rooms.contains(id),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_allowed_when_listed() {
        let al = Allowlist::from_csv("U123,U456", "", "");
        assert!(al.is_allowed(&Source::User { user_id: Some("U123".into()) }));
    }

    #[test]
    fn user_denied_when_not_listed() {
        let al = Allowlist::from_csv("U123", "", "");
        assert!(!al.is_allowed(&Source::User { user_id: Some("UXXX".into()) }));
    }

    #[test]
    fn group_path() {
        let al = Allowlist::from_csv("", "G1", "");
        assert!(al.is_allowed(&Source::Group { group_id: Some("G1".into()), user_id: None }));
    }

    #[test]
    fn room_path() {
        let al = Allowlist::from_csv("", "", "R1");
        assert!(al.is_allowed(&Source::Room { room_id: Some("R1".into()), user_id: None }));
    }

    #[test]
    fn missing_id_denied() {
        let al = Allowlist::from_csv("U1", "G1", "R1");
        assert!(!al.is_allowed(&Source::User { user_id: None }));
    }

    #[test]
    fn empty_list_denies_all() {
        let al = Allowlist::default();
        assert!(!al.is_allowed(&Source::User { user_id: Some("U1".into()) }));
    }

    #[test]
    fn csv_trims_and_skips_blanks() {
        let al = Allowlist::from_csv(" U1 , ,U2,", "", "");
        assert_eq!(al.users.len(), 2);
    }

    #[test]
    fn deserialize_source_user() {
        let s: Source = serde_json::from_str(r#"{"type":"user","userId":"U123"}"#).unwrap();
        assert_eq!(s.chat_id(), Some("U123"));
    }
}
```

- [ ] **Step 2: Wire**

Append to `crates/core/src/line/direct/mod.rs`:

```rust
pub mod allowlist;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p thclaws-core --lib line::direct::allowlist`
Expected: 8 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/line/direct/allowlist.rs crates/core/src/line/direct/mod.rs
git commit -m "feat(line/direct): allowlist + Source enum"
```

---

## Task 6: Dedup (bounded LRU on `webhookEventId`)

**Files:**
- Create: `crates/core/src/line/direct/dedup.rs`
- Modify: `crates/core/src/line/direct/mod.rs`

LINE may redeliver. Drop events whose `webhookEventId` we've seen recently. Bounded LRU prevents unbounded growth from a chatty channel.

- [ ] **Step 1: Write the test + impl**

Create `crates/core/src/line/direct/dedup.rs`:

```rust
//! Bounded LRU dedup keyed by LINE `webhookEventId`. ~1000 entries;
//! evicts oldest 10% when at capacity. Cheap O(1) check.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

const CAP: usize = 1024;
const EVICT_BATCH: usize = 128;

#[derive(Default)]
struct Inner {
    seen: HashMap<String, ()>,
    order: VecDeque<String>,
}

#[derive(Default)]
pub struct DedupStore {
    inner: Mutex<Inner>,
}

impl DedupStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if `id` is new (and now recorded); `false` if
    /// already seen.
    pub fn check_and_record(&self, id: &str) -> bool {
        if id.is_empty() {
            return true; // no id = can't dedup; always pass through
        }
        let mut g = self.inner.lock().unwrap();
        if g.seen.contains_key(id) {
            return false;
        }
        if g.order.len() >= CAP {
            for _ in 0..EVICT_BATCH {
                if let Some(old) = g.order.pop_front() {
                    g.seen.remove(&old);
                }
            }
        }
        g.seen.insert(id.to_string(), ());
        g.order.push_back(id.to_string());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sighting_passes() {
        let d = DedupStore::new();
        assert!(d.check_and_record("abc"));
    }

    #[test]
    fn second_sighting_rejected() {
        let d = DedupStore::new();
        assert!(d.check_and_record("abc"));
        assert!(!d.check_and_record("abc"));
    }

    #[test]
    fn distinct_ids_independent() {
        let d = DedupStore::new();
        assert!(d.check_and_record("a"));
        assert!(d.check_and_record("b"));
        assert!(!d.check_and_record("a"));
    }

    #[test]
    fn empty_id_always_passes() {
        let d = DedupStore::new();
        assert!(d.check_and_record(""));
        assert!(d.check_and_record(""));
    }

    #[test]
    fn evicts_oldest_when_full() {
        let d = DedupStore::new();
        // fill exactly to CAP
        for i in 0..CAP {
            assert!(d.check_and_record(&format!("e{}", i)));
        }
        // CAP+1 triggers eviction batch
        assert!(d.check_and_record("new"));
        // earliest entries may be evicted; "e0" likely gone, so re-record succeeds
        assert!(d.check_and_record("e0"));
        // most recent entry still present
        assert!(!d.check_and_record("new"));
    }
}
```

- [ ] **Step 2: Wire**

Append to `crates/core/src/line/direct/mod.rs`:

```rust
pub mod dedup;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p thclaws-core --lib line::direct::dedup`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/line/direct/dedup.rs crates/core/src/line/direct/mod.rs
git commit -m "feat(line/direct): bounded-LRU dedup on webhookEventId"
```

---

## Task 7: ReplyToken store (50s TTL, single-consume)

**Files:**
- Create: `crates/core/src/line/direct/reply_store.rs`
- Modify: `crates/core/src/line/direct/mod.rs`

LINE replyTokens expire ~60s after delivery; we cap at 50s to give the network slack. Single-consume: once popped for a reply, gone — caller must not retry with the same token.

- [ ] **Step 1: Write the test + impl**

Create `crates/core/src/line/direct/reply_store.rs`:

```rust
//! ReplyToken store with TTL + single-consume semantics.
//!
//! - One token per chat_id; new inbound message overwrites old token
//!   (the old one is invalidated by LINE anyway).
//! - `take()` pops + checks expiry in one atomic step.
//! - `take_if_valid_for_request()` is used by the slow-response path:
//!   when a postback arrives, we want the NEW replyToken from that
//!   postback event, not the cached one — see slow_response.rs.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const TOKEN_TTL: Duration = Duration::from_secs(50);

#[derive(Default)]
pub struct ReplyTokenStore {
    inner: Mutex<HashMap<String, (String, Instant)>>,
}

impl ReplyTokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a fresh reply token for `chat_id`. Overwrites any prior token.
    pub fn put(&self, chat_id: &str, token: String) {
        let mut g = self.inner.lock().unwrap();
        g.insert(chat_id.to_string(), (token, Instant::now()));
    }

    /// Pop the token if still within TTL. Returns `Some(token)` once;
    /// subsequent calls return `None` for that chat_id until a new
    /// inbound message refreshes it.
    pub fn take(&self, chat_id: &str) -> Option<String> {
        let mut g = self.inner.lock().unwrap();
        let (token, recorded) = g.remove(chat_id)?;
        if recorded.elapsed() <= TOKEN_TTL {
            Some(token)
        } else {
            None
        }
    }

    /// Peek without consuming. Used to decide whether to send the
    /// slow-response button (which requires the original token).
    pub fn has_valid(&self, chat_id: &str) -> bool {
        let g = self.inner.lock().unwrap();
        g.get(chat_id)
            .map(|(_, recorded)| recorded.elapsed() <= TOKEN_TTL)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_take_returns_token() {
        let s = ReplyTokenStore::new();
        s.put("U1", "tok-1".into());
        assert_eq!(s.take("U1").as_deref(), Some("tok-1"));
    }

    #[test]
    fn second_take_returns_none() {
        let s = ReplyTokenStore::new();
        s.put("U1", "tok".into());
        assert!(s.take("U1").is_some());
        assert!(s.take("U1").is_none());
    }

    #[test]
    fn new_put_overwrites_old() {
        let s = ReplyTokenStore::new();
        s.put("U1", "old".into());
        s.put("U1", "new".into());
        assert_eq!(s.take("U1").as_deref(), Some("new"));
    }

    #[test]
    fn distinct_chats_independent() {
        let s = ReplyTokenStore::new();
        s.put("U1", "a".into());
        s.put("U2", "b".into());
        assert_eq!(s.take("U1").as_deref(), Some("a"));
        assert_eq!(s.take("U2").as_deref(), Some("b"));
    }

    #[test]
    fn has_valid_after_put_true() {
        let s = ReplyTokenStore::new();
        s.put("U1", "x".into());
        assert!(s.has_valid("U1"));
    }

    #[test]
    fn has_valid_after_take_false() {
        let s = ReplyTokenStore::new();
        s.put("U1", "x".into());
        let _ = s.take("U1");
        assert!(!s.has_valid("U1"));
    }

    // Note: actual TTL expiry test deferred to integration suite
    // (would need to either sleep 50s or inject a clock; both add
    // complexity without behavioural coverage gain — the TTL value
    // is a constant, not a code path).
}
```

- [ ] **Step 2: Wire**

Append to `crates/core/src/line/direct/mod.rs`:

```rust
pub mod reply_store;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p thclaws-core --lib line::direct::reply_store`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/line/direct/reply_store.rs crates/core/src/line/direct/mod.rs
git commit -m "feat(line/direct): ReplyTokenStore with 50s TTL"
```

---

## Task 8: Chunking (≤5 messages × ≤4500 chars/bubble)

**Files:**
- Create: `crates/core/src/line/direct/chunk.rs`
- Modify: `crates/core/src/line/direct/mod.rs`

LINE Reply API: max 5 messages per call; each text message ≤ 5000 chars (we use 4500 as safe bubble size). Split on paragraph, then line, then space, then hard-break. Truncate with `…` if 5-bubble budget exhausted.

- [ ] **Step 1: Write the test + impl**

Create `crates/core/src/line/direct/chunk.rs`:

```rust
//! Split a long agent reply into LINE-shaped chunks.
//!
//! Limits (from LINE Messaging API):
//!   - max 5 text messages per Reply call
//!   - 5000 char hard ceiling per bubble; we use 4500 as safe size
//!     to leave room for any inline metadata the agent emits

pub const MAX_MESSAGES_PER_REPLY: usize = 5;
pub const SAFE_BUBBLE_CHARS: usize = 4500;
const TRUNCATE_MARKER: &str = "\n…";

pub fn split_for_line(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    let mut out: Vec<String> = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() && out.len() < MAX_MESSAGES_PER_REPLY {
        // Fast path: short enough to fit in one bubble
        if remaining.chars().count() <= SAFE_BUBBLE_CHARS {
            out.push(remaining.to_string());
            break;
        }
        // Find a split point at SAFE_BUBBLE_CHARS preferring \n\n,
        // then \n, then space.
        let cut = pick_cut(remaining, SAFE_BUBBLE_CHARS);
        let (head, tail) = split_at_char(remaining, cut);
        out.push(head.to_string());
        remaining = tail.trim_start();
    }

    // If still remaining after we filled MAX_MESSAGES, mark truncation.
    if !remaining.is_empty() && out.len() == MAX_MESSAGES_PER_REPLY {
        if let Some(last) = out.last_mut() {
            // Reserve space for marker
            let limit = SAFE_BUBBLE_CHARS.saturating_sub(TRUNCATE_MARKER.chars().count());
            if last.chars().count() > limit {
                *last = take_chars(last, limit);
            }
            last.push_str(TRUNCATE_MARKER);
        }
    }
    out
}

fn pick_cut(s: &str, target: usize) -> usize {
    // Search backward from `target` for a paragraph / line / space.
    let chars: Vec<(usize, char)> = s.char_indices().take(target).collect();
    // Try \n\n
    for i in (0..chars.len()).rev() {
        if i + 1 < chars.len() && chars[i].1 == '\n' && chars[i + 1].1 == '\n' {
            return i + 1;
        }
    }
    // Try single \n
    for i in (0..chars.len()).rev() {
        if chars[i].1 == '\n' {
            return i + 1;
        }
    }
    // Try space
    for i in (0..chars.len()).rev() {
        if chars[i].1 == ' ' {
            return i + 1;
        }
    }
    // Hard cut at `target` chars
    target
}

fn split_at_char(s: &str, char_idx: usize) -> (&str, &str) {
    let byte_idx = s.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(s.len());
    s.split_at(byte_idx)
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_empty_vec() {
        assert!(split_for_line("").is_empty());
    }

    #[test]
    fn short_message_single_bubble() {
        let v = split_for_line("hi");
        assert_eq!(v, vec!["hi"]);
    }

    #[test]
    fn under_limit_single_bubble() {
        let text = "a".repeat(SAFE_BUBBLE_CHARS);
        let v = split_for_line(&text);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].chars().count(), SAFE_BUBBLE_CHARS);
    }

    #[test]
    fn slightly_over_limit_splits_to_two() {
        let text = "a".repeat(SAFE_BUBBLE_CHARS + 100);
        let v = split_for_line(&text);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn prefers_paragraph_boundary() {
        let para = "x".repeat(100);
        let text = format!("{}\n\n{}", para, "y".repeat(SAFE_BUBBLE_CHARS));
        let v = split_for_line(&text);
        assert!(v.len() >= 2);
        // first bubble ends after the paragraph break
        assert!(v[0].ends_with('\n'));
    }

    #[test]
    fn caps_at_five_messages_with_truncate_marker() {
        // 6 bubbles worth of content
        let text = "z".repeat(SAFE_BUBBLE_CHARS * 6);
        let v = split_for_line(&text);
        assert_eq!(v.len(), MAX_MESSAGES_PER_REPLY);
        assert!(v.last().unwrap().ends_with(TRUNCATE_MARKER));
    }

    #[test]
    fn utf8_safe_thai_split() {
        // Thai text crossing the boundary must not split a codepoint
        let chunk = "ก".repeat(SAFE_BUBBLE_CHARS);
        let text = format!("{}{}", chunk, "ข".repeat(100));
        let v = split_for_line(&text);
        assert!(v.len() >= 1);
        // bubbles are valid UTF-8 by construction (Rust String)
        for b in &v {
            assert!(!b.is_empty());
        }
    }
}
```

- [ ] **Step 2: Wire**

Append to `crates/core/src/line/direct/mod.rs`:

```rust
pub mod chunk;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p thclaws-core --lib line::direct::chunk`
Expected: 7 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/line/direct/chunk.rs crates/core/src/line/direct/mod.rs
git commit -m "feat(line/direct): split_for_line — 5 bubbles x 4500 chars cap"
```

---

## Task 9: `DirectConfig` + errors

**Files:**
- Create: `crates/core/src/line/direct/config.rs`
- Create: `crates/core/src/line/direct/errors.rs`
- Modify: `crates/core/src/line/direct/mod.rs`
- Modify: `crates/core/src/line/config.rs` — add `direct: Option<DirectConfig>` field

`DirectConfig` carries only NON-secret operational settings (host, port, public_url display, allowlist, threshold). Secrets (`channel_access_token`, `channel_secret`) load lazily from keychain → env fallback at request time so they are never persisted to `line.json`.

- [ ] **Step 1: Errors module**

`crates/core/src/line/direct/errors.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DirectLineError {
    #[error("no valid reply token for this chat (expired, used, or never received)")]
    NoValidReplyToken,
    #[error("LINE Reply API returned {status}: {body}")]
    ReplyApi { status: u16, body: String },
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("payload exceeds LINE limits and chunking failed")]
    PayloadTooLarge,
    #[error("missing channel_access_token (set LINE_CHANNEL_ACCESS_TOKEN or keychain)")]
    MissingAccessToken,
    #[error("missing channel_secret (set LINE_CHANNEL_SECRET or keychain)")]
    MissingSecret,
}
```

- [ ] **Step 2: Config struct**

`crates/core/src/line/direct/config.rs`:

```rust
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
}

fn default_host() -> String { DEFAULT_HOST.into() }
fn default_port() -> u16 { DEFAULT_PORT }
fn default_threshold_secs() -> u64 { DEFAULT_SLOW_THRESHOLD_SECS }

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
        }
    }
}

impl DirectConfig {
    /// Build from env vars; called when neither GUI nor on-disk
    /// config supplies values. Pattern: `THCLAWS_LINE_*` overrides
    /// `LINE_*`.
    pub fn from_env() -> Self {
        let env = |a: &str, b: &str| std::env::var(a).or_else(|_| std::env::var(b)).ok();
        Self {
            host: env("THCLAWS_LINE_DIRECT_HOST", "LINE_HOST").unwrap_or_else(|| DEFAULT_HOST.into()),
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
```

Plus inline tests:
- `from_env_picks_up_thclaws_prefix` (set `THCLAWS_LINE_DIRECT_PORT=9999`, check parse)
- `webhook_display_url_with_public_url` returns canonical
- `webhook_display_url_fallback_to_host_port`
- `default_threshold_is_45s`

- [ ] **Step 3: Wire into LineConfig**

In `crates/core/src/line/config.rs`, add field after `mode`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub direct: Option<crate::line::direct::config::DirectConfig>,
```

- [ ] **Step 4: Wire submodule**

In `crates/core/src/line/direct/mod.rs`:

```rust
pub mod config;
pub mod errors;
```

- [ ] **Step 5: Run tests + commit**

Run: `cargo test -p thclaws-core --lib line::direct`
Expected: prior tests still green + 4 new config tests pass.

```bash
git add crates/core/src/line/direct/config.rs crates/core/src/line/direct/errors.rs crates/core/src/line/direct/mod.rs crates/core/src/line/config.rs
git commit -m "feat(line/direct): DirectConfig + DirectLineError"
```

---

## Task 10: Slow-response postback cache

**Files:**
- Create: `crates/core/src/line/direct/slow_response.rs`
- Modify: `crates/core/src/line/direct/mod.rs`

State machine:

```
register(chat_id) → PENDING [request_id]
  ├─ set_ready(request_id, text)    → READY
  ├─ set_error(request_id, msg)     → ERROR
  └─ (timeout/disconnect)           → entry remains; pruned by reaper
on_postback(request_id)
  ├─ READY     → take text, transition DELIVERED
  ├─ ERROR     → take msg,  transition DELIVERED
  ├─ PENDING   → return "still working" hint, leave entry
  └─ DELIVERED → return "already replied" hint
```

- [ ] **Step 1: Write the implementation + tests**

Create `crates/core/src/line/direct/slow_response.rs`:

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Pending,
    Ready(String),
    Delivered,
    Error(String),
}

struct Entry {
    state: State,
    chat_id: String,
    created: Instant,
}

#[derive(Default)]
pub struct SlowResponseCache {
    inner: Mutex<HashMap<String, Entry>>,
}

/// Outcome returned to the postback handler so it knows what to reply with.
#[derive(Debug)]
pub enum PostbackResult {
    Ready(String),
    Error(String),
    StillPending,
    AlreadyDelivered,
    Unknown,
}

impl SlowResponseCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, chat_id: &str) -> String {
        let request_id = Uuid::new_v4().to_string();
        let mut g = self.inner.lock().unwrap();
        g.insert(
            request_id.clone(),
            Entry { state: State::Pending, chat_id: chat_id.into(), created: Instant::now() },
        );
        request_id
    }

    pub fn set_ready(&self, request_id: &str, text: String) {
        let mut g = self.inner.lock().unwrap();
        if let Some(e) = g.get_mut(request_id) {
            if matches!(e.state, State::Pending) {
                e.state = State::Ready(text);
            }
        }
    }

    pub fn set_error(&self, request_id: &str, msg: String) {
        let mut g = self.inner.lock().unwrap();
        if let Some(e) = g.get_mut(request_id) {
            if matches!(e.state, State::Pending) {
                e.state = State::Error(msg);
            }
        }
    }

    pub fn on_postback(&self, request_id: &str) -> PostbackResult {
        let mut g = self.inner.lock().unwrap();
        let Some(entry) = g.get_mut(request_id) else { return PostbackResult::Unknown };
        match std::mem::replace(&mut entry.state, State::Delivered) {
            State::Ready(text) => PostbackResult::Ready(text),
            State::Error(msg) => PostbackResult::Error(msg),
            State::Pending => {
                entry.state = State::Pending; // restore (no transition)
                PostbackResult::StillPending
            }
            State::Delivered => {
                entry.state = State::Delivered;
                PostbackResult::AlreadyDelivered
            }
        }
    }

    /// Periodic reaper — drop DELIVERED/ERROR entries older than 1h,
    /// PENDING older than 24h. Worker calls this on a timer.
    pub fn reap(&self) {
        let mut g = self.inner.lock().unwrap();
        g.retain(|_, e| {
            let age = e.created.elapsed().as_secs();
            match &e.state {
                State::Pending => age < 24 * 3600,
                _ => age < 3600,
            }
        });
    }
}
```

Add `uuid = { version = "1", features = ["v4"] }` to `Cargo.toml` if not already present (note: thClaws likely already uses uuid — verify; if missing, fold into Task 1's dep additions).

Tests:
- `register_returns_unique_request_ids`
- `set_ready_transitions_pending_to_ready`
- `set_ready_noop_after_delivered`
- `on_postback_ready_returns_text_and_marks_delivered`
- `on_postback_pending_returns_still_pending_no_transition`
- `on_postback_delivered_returns_already_delivered`
- `on_postback_unknown_id_returns_unknown`
- `set_error_path`

- [ ] **Step 2: Wire + test + commit**

In `mod.rs`: `pub mod slow_response;`
Run: `cargo test -p thclaws-core --lib line::direct::slow_response`
Commit: `feat(line/direct): slow-response postback cache (PENDING/READY/DELIVERED/ERROR)`

---

## Task 11: `DirectLineClient` (Reply API only)

**Files:**
- Create: `crates/core/src/line/direct/client.rs`
- Modify: `crates/core/src/line/direct/mod.rs`

Strict rule: this file MUST NOT contain `push`, `message/push`, or `api.line.me/v2/bot/message/push` anywhere. A test asserts the file contents.

- [ ] **Step 1: Implementation**

`crates/core/src/line/direct/client.rs`:

```rust
use super::chunk::split_for_line;
use super::errors::DirectLineError;
use serde::Serialize;

pub const REPLY_URL: &str = "https://api.line.me/v2/bot/message/reply";

#[derive(Serialize)]
struct TextMessage<'a> {
    #[serde(rename = "type")]
    typ: &'a str, // "text"
    text: &'a str,
}

#[derive(Serialize)]
struct ReplyPayload<'a> {
    #[serde(rename = "replyToken")]
    reply_token: &'a str,
    messages: Vec<TextMessage<'a>>,
}

pub struct DirectLineClient {
    http: reqwest::Client,
    access_token: String,
}

impl DirectLineClient {
    pub fn new(access_token: String) -> Result<Self, DirectLineError> {
        if access_token.is_empty() {
            return Err(DirectLineError::MissingAccessToken);
        }
        let http = reqwest::Client::builder()
            .user_agent(concat!("thclaws-core/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| DirectLineError::Http(e.to_string()))?;
        Ok(Self { http, access_token })
    }

    /// Reply to a single inbound event. `reply_token` must be from
    /// the SAME event's webhook payload — LINE rejects replays.
    pub async fn reply_text(&self, reply_token: &str, text: &str) -> Result<(), DirectLineError> {
        if reply_token.is_empty() {
            return Err(DirectLineError::NoValidReplyToken);
        }
        let bubbles = split_for_line(text);
        if bubbles.is_empty() {
            return Ok(()); // nothing to send is fine
        }
        let messages: Vec<TextMessage> =
            bubbles.iter().map(|b| TextMessage { typ: "text", text: b }).collect();
        let payload = ReplyPayload { reply_token, messages };

        let resp = self
            .http
            .post(REPLY_URL)
            .bearer_auth(&self.access_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| DirectLineError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(DirectLineError::ReplyApi { status, body });
        }
        Ok(())
    }

    /// Reply using LINE Template Buttons. Used by slow-response path
    /// to send a "รับคำตอบ" button before reply_token expires.
    /// `postback_data` becomes the JSON-encoded payload on tap.
    pub async fn reply_template_button(
        &self,
        reply_token: &str,
        text: &str,
        button_label: &str,
        postback_data: &str,
    ) -> Result<(), DirectLineError> {
        if reply_token.is_empty() {
            return Err(DirectLineError::NoValidReplyToken);
        }
        let alt_text: String = text.chars().take(400).collect();
        let template_text: String = text.chars().take(160).collect();
        let payload = serde_json::json!({
            "replyToken": reply_token,
            "messages": [{
                "type": "template",
                "altText": alt_text,
                "template": {
                    "type": "buttons",
                    "text": template_text,
                    "actions": [{
                        "type": "postback",
                        "label": button_label,
                        "data": postback_data
                    }]
                }
            }]
        });
        let resp = self
            .http
            .post(REPLY_URL)
            .bearer_auth(&self.access_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| DirectLineError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(DirectLineError::ReplyApi { status, body });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_url_is_canonical_line_endpoint() {
        assert_eq!(REPLY_URL, "https://api.line.me/v2/bot/message/reply");
    }

    #[test]
    fn empty_access_token_rejected() {
        assert!(matches!(
            DirectLineClient::new(String::new()),
            Err(DirectLineError::MissingAccessToken)
        ));
    }

    #[tokio::test]
    async fn empty_reply_token_returns_no_valid_token() {
        let client = DirectLineClient::new("t".into()).unwrap();
        let r = client.reply_text("", "hi").await;
        assert!(matches!(r, Err(DirectLineError::NoValidReplyToken)));
    }

    #[test]
    fn source_does_not_contain_push_endpoints() {
        // Guard against accidental Push API additions.
        let src = include_str!("client.rs");
        assert!(!src.contains("message/push"), "Push API endpoint must not appear");
        assert!(!src.contains("push.line.me"), "Push host must not appear");
        // PUSH_URL constants — none allowed
        assert!(!src.to_lowercase().contains("push_url"));
    }
}
```

- [ ] **Step 2: Wire + test + commit**

`mod.rs`: `pub mod client;`
Run: `cargo test -p thclaws-core --lib line::direct::client`
Commit: `feat(line/direct): DirectLineClient — Reply API only, no Push`

---

## Task 12: Webhook server (axum)

**Files:**
- Create: `crates/core/src/line/direct/types.rs` — event wire shapes
- Create: `crates/core/src/line/direct/server.rs` — axum router + handler
- Modify: `crates/core/src/line/direct/mod.rs`

The handler must:
1. Read raw body bytes (signature is over raw bytes)
2. Verify `X-Line-Signature` → 401 on mismatch
3. Parse JSON → 400 on malformed
4. For each event:
   a. Dedup on `webhookEventId`
   b. Allowlist check on `source`
   c. Stash `replyToken` (for message events)
   d. Dispatch to handler trait (ShellInput::LineMessage producer in real wiring)

The handler returns 200 OK ASAP — never blocks on agent work. Agent reply flows back through a separate channel.

- [ ] **Step 1: Types**

`crates/core/src/line/direct/types.rs`:

```rust
use super::allowlist::Source;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct WebhookBody {
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Message {
        #[serde(rename = "webhookEventId", default)]
        webhook_event_id: String,
        #[serde(rename = "replyToken", default)]
        reply_token: String,
        source: Source,
        message: MessagePayload,
    },
    Postback {
        #[serde(rename = "webhookEventId", default)]
        webhook_event_id: String,
        #[serde(rename = "replyToken", default)]
        reply_token: String,
        source: Source,
        postback: PostbackPayload,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MessagePayload {
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostbackPayload {
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShowResponsePostback {
    pub action: String,
    pub request_id: String,
}
```

- [ ] **Step 2: Server**

`crates/core/src/line/direct/server.rs`:

```rust
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};

use super::allowlist::Allowlist;
use super::config::{DirectConfig, WEBHOOK_PATH};
use super::dedup::DedupStore;
use super::reply_store::ReplyTokenStore;
use super::signature::verify;
use super::slow_response::SlowResponseCache;
use super::types::{Event, MessagePayload, ShowResponsePostback, WebhookBody};

/// Trait the runtime supplies — converts an inbound LINE text/postback
/// into action. Decouples server from worker + slow-response wiring.
#[async_trait::async_trait]
pub trait DirectEventSink: Send + Sync + 'static {
    /// New text message arrived. Implementor stashes reply_token,
    /// kicks the slow-response timer, dispatches to agent.
    async fn on_message(&self, chat_id: String, reply_token: String, text: String);
    /// Postback arrived. Implementor decides what to do based on
    /// action (show_response, ignore, etc).
    async fn on_show_response_postback(&self, chat_id: String, reply_token: String, request_id: String);
}

pub struct DirectServerState {
    pub channel_secret: Vec<u8>,
    pub allowlist: Allowlist,
    pub dedup: Arc<DedupStore>,
    pub reply_store: Arc<ReplyTokenStore>,
    pub slow_cache: Arc<SlowResponseCache>,
    pub sink: Arc<dyn DirectEventSink>,
}

pub fn router(state: Arc<DirectServerState>, _cfg: &DirectConfig) -> Router {
    Router::new()
        .route(WEBHOOK_PATH, post(handle_webhook))
        .route("/line/health", get(handle_health))
        .with_state(state)
}

async fn handle_health() -> &'static str {
    "ok"
}

async fn handle_webhook(
    State(state): State<Arc<DirectServerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let sig = headers
        .get("x-line-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify(&body, sig, &state.channel_secret) {
        return (StatusCode::UNAUTHORIZED, "bad signature").into_response();
    }
    let parsed: WebhookBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad body").into_response(),
    };
    for event in parsed.events {
        dispatch_event(state.clone(), event).await;
    }
    (StatusCode::OK, "").into_response()
}

async fn dispatch_event(state: Arc<DirectServerState>, event: Event) {
    match event {
        Event::Message { webhook_event_id, reply_token, source, message } => {
            if !state.dedup.check_and_record(&webhook_event_id) { return }
            if !state.allowlist.is_allowed(&source) { return }
            let MessagePayload::Text { text } = message else { return };
            let Some(chat_id) = source.chat_id() else { return };
            state.reply_store.put(chat_id, reply_token.clone());
            state.sink.on_message(chat_id.to_string(), reply_token, text).await;
        }
        Event::Postback { webhook_event_id, reply_token, source, postback } => {
            if !state.dedup.check_and_record(&webhook_event_id) { return }
            if !state.allowlist.is_allowed(&source) { return }
            let Some(chat_id) = source.chat_id() else { return };
            // Slow-response postback is JSON: {"action":"show_response","request_id":"..."}
            if let Ok(p) = serde_json::from_str::<ShowResponsePostback>(&postback.data) {
                if p.action == "show_response" {
                    state.sink.on_show_response_postback(chat_id.to_string(), reply_token, p.request_id).await;
                }
            }
        }
        Event::Other => {}
    }
}
```

Add deps if not present: `async-trait`. (Hosted client.rs already uses `async_trait::async_trait`, so it should be in `Cargo.toml`.)

- [ ] **Step 3: Integration tests**

Add to `server.rs` `#[cfg(test)]`:

- `bad_signature_returns_401` — POST with wrong sig header
- `malformed_json_returns_400` — POST with valid sig but garbage body
- `unknown_user_dropped_silently` — POST valid sig + body with source not in allowlist; assert sink not called
- `duplicate_webhook_event_dropped` — POST same webhookEventId twice; assert sink called once
- `text_message_dispatches_to_sink_with_reply_token` — happy path
- `show_response_postback_dispatches_to_sink` — postback w/ action=show_response

Tests use a fake `DirectEventSink` recording calls in `Arc<Mutex<Vec<...>>>`. axum router built directly; calls dispatched via `tower::ServiceExt::oneshot`.

- [ ] **Step 4: Wire + commit**

`mod.rs`: `pub mod types; pub mod server;`
Run: `cargo test -p thclaws-core --lib line::direct::server`
Commit: `feat(line/direct): axum webhook server with signature + allowlist + dedup`

---

## Task 13: `direct::spawn` — orchestrate webhook + reply pipeline

**Files:**
- Create: `crates/core/src/line/direct/spawn.rs`
- Modify: `crates/core/src/line/direct/mod.rs`

Brings together: `DirectConfig` → secrets load → build `DirectServerState` → bind axum listener → spawn task → return handle.

- [ ] **Step 1: Implementation skeleton**

`crates/core/src/line/direct/spawn.rs`:

```rust
use std::net::SocketAddr;
use std::sync::Arc;

use super::allowlist::Allowlist;
use super::client::DirectLineClient;
use super::config::DirectConfig;
use super::dedup::DedupStore;
use super::errors::DirectLineError;
use super::reply_store::ReplyTokenStore;
use super::server::{router, DirectEventSink, DirectServerState};
use super::slow_response::SlowResponseCache;
use crate::cancel::CancelToken;

pub struct DirectHandle {
    pub cancel: CancelToken,
    pub join: tokio::task::JoinHandle<()>,
    pub client: Arc<DirectLineClient>,
    pub reply_store: Arc<ReplyTokenStore>,
    pub slow_cache: Arc<SlowResponseCache>,
    pub bind_addr: SocketAddr,
}

/// Resolve secrets from keychain (preferred) → env fallback.
fn load_secret(key: &str) -> Result<String, DirectLineError> {
    // Real impl uses crate::secrets module — mirror the pattern in
    // crates/core/src/secrets.rs (keychain → env fallback, gated by
    // THCLAWS_DISABLE_KEYCHAIN). Here we sketch the contract.
    if let Ok(v) = std::env::var(key) {
        if !v.is_empty() { return Ok(v); }
    }
    // TODO in impl: crate::secrets::keychain_get("thclaws-line", key)
    Err(if key.contains("SECRET") {
        DirectLineError::MissingSecret
    } else {
        DirectLineError::MissingAccessToken
    })
}

pub async fn spawn(
    config: DirectConfig,
    sink: Arc<dyn DirectEventSink>,
) -> Result<DirectHandle, DirectLineError> {
    let access_token = load_secret("LINE_CHANNEL_ACCESS_TOKEN")?;
    let secret = load_secret("LINE_CHANNEL_SECRET")?;

    let client = Arc::new(DirectLineClient::new(access_token)?);
    let reply_store = Arc::new(ReplyTokenStore::new());
    let slow_cache = Arc::new(SlowResponseCache::new());
    let dedup = Arc::new(DedupStore::new());

    let state = Arc::new(DirectServerState {
        channel_secret: secret.into_bytes(),
        allowlist: Allowlist::from_csv(
            &config.allowed_users_csv,
            &config.allowed_groups_csv,
            &config.allowed_rooms_csv,
        ),
        dedup,
        reply_store: reply_store.clone(),
        slow_cache: slow_cache.clone(),
        sink,
    });

    let app = router(state, &config);
    let bind_addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e: std::net::AddrParseError| DirectLineError::Http(e.to_string()))?;

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|e| DirectLineError::Http(e.to_string()))?;

    let cancel = CancelToken::new();
    let cancel_for_task = cancel.clone();
    let join = tokio::spawn(async move {
        let shutdown = async move { cancel_for_task.cancelled().await };
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
        {
            eprintln!("[line/direct] server stopped: {e}");
        }
    });

    Ok(DirectHandle { cancel, join, client, reply_store, slow_cache, bind_addr })
}
```

- [ ] **Step 2: Wire + commit**

`mod.rs`: `pub mod spawn;`
Run: `cargo check -p thclaws-core` (no test added here — spawn needs real port binding; covered in shared_session integration test in Task 14)
Commit: `feat(line/direct): spawn — orchestrate webhook + Reply API client`

---

## Task 14: `shared_session.rs` wiring — slow-response orchestration

**Files:**
- Modify: `crates/core/src/shared_session.rs`
- Modify: `crates/core/src/line/bootstrap.rs` (branch on mode)

The shared session must:
1. Implement `DirectEventSink` over the worker's `ShellInput::LineMessage` channel
2. On `on_message`: stash reply_token, register slow-response entry (PENDING), kick a delayed task that fires the slow-response button if the agent hasn't finished by threshold, await agent reply via oneshot
3. When agent reply arrives:
   - If reply_token still valid AND no slow-response button sent yet → reply immediately
   - If slow-response button already sent → `slow_cache.set_ready()`, do not reply
   - If reply_token gone or used → log `[line/direct] drop: NoValidReplyToken (chat_id=<hash>)` (no sensitive data, hash chat_id with SHA256→first 8 hex chars)
4. On `on_show_response_postback`: call `slow_cache.on_postback(request_id)`:
   - `Ready(text)` → `client.reply_text(new_reply_token, text)` (use NEW token from postback event)
   - `Error(msg)` → `client.reply_text(new_reply_token, "งานถูกยกเลิก: ...")`
   - `StillPending` → `client.reply_text(new_reply_token, "ยังทำงานอยู่ กรุณารอสักครู่")`
   - `AlreadyDelivered` → `client.reply_text(new_reply_token, "ส่งคำตอบแล้ว ✅")`
   - `Unknown` → drop silently

- [ ] **Step 1: Implementation skeleton**

Inside `shared_session.rs`, add module `mod line_direct_sink;` or inline:

```rust
pub struct DirectSink {
    input_tx: mpsc::Sender<ShellInput>,
    client: Arc<DirectLineClient>,
    reply_store: Arc<ReplyTokenStore>,
    slow_cache: Arc<SlowResponseCache>,
    threshold: Duration,
}

#[async_trait::async_trait]
impl DirectEventSink for DirectSink {
    async fn on_message(&self, chat_id: String, reply_token: String, text: String) {
        // 1. register PENDING entry in slow_cache
        let request_id = self.slow_cache.register(&chat_id);
        // 2. fire delayed slow-response button task
        let slow_client = self.client.clone();
        let slow_token = reply_token.clone();
        let slow_cache = self.slow_cache.clone();
        let req = request_id.clone();
        let chat_id2 = chat_id.clone();
        let threshold = self.threshold;
        let button_handle = tokio::spawn(async move {
            tokio::time::sleep(threshold).await;
            // Only fire if entry still PENDING
            if matches!(
                slow_cache.peek_state(&req),
                Some(super::line::direct::slow_response::State::Pending)
            ) {
                let data = serde_json::json!({"action":"show_response","request_id":req}).to_string();
                let _ = slow_client.reply_template_button(
                    &slow_token,
                    "กำลังคิดอยู่ กดรับคำตอบเมื่อพร้อม",
                    "รับคำตอบ",
                    &data,
                ).await;
                // Mark "button sent" — token now consumed, do not reply directly
                slow_cache.mark_button_sent(&req);
            }
        });
        // 3. send to agent (non-blocking)
        let (tx, rx) = oneshot::channel();
        let _ = self.input_tx.send(ShellInput::LineMessage { text, respond: tx });
        // 4. await agent answer in a detached task
        let client = self.client.clone();
        let slow_cache = self.slow_cache.clone();
        let store = self.reply_store.clone();
        let cid = chat_id.clone();
        let rt_clone = reply_token.clone();
        let req2 = request_id.clone();
        tokio::spawn(async move {
            let answer = rx.await.unwrap_or_default();
            // Cancel slow-response timer if it hasn't fired
            button_handle.abort();
            if slow_cache.was_button_sent(&req2) {
                // Cache for postback
                slow_cache.set_ready(&req2, answer);
            } else if store.take(&cid).is_some() {
                // Token still valid → reply directly
                if let Err(e) = client.reply_text(&rt_clone, &answer).await {
                    eprintln!("[line/direct] reply failed: {e}");
                }
                slow_cache.set_ready(&req2, String::new()); // mark consumed
            } else {
                // Drop with sanitized log
                eprintln!("[line/direct] drop: NoValidReplyToken chat={}", hash_chat(&cid));
            }
        });
    }

    async fn on_show_response_postback(&self, _chat_id: String, reply_token: String, request_id: String) {
        use super::line::direct::slow_response::PostbackResult;
        let text = match self.slow_cache.on_postback(&request_id) {
            PostbackResult::Ready(t) => t,
            PostbackResult::Error(m) => format!("งานถูกยกเลิก: {m}"),
            PostbackResult::StillPending => "ยังทำงานอยู่ กรุณารอสักครู่".into(),
            PostbackResult::AlreadyDelivered => "ส่งคำตอบแล้ว ✅".into(),
            PostbackResult::Unknown => return,
        };
        let _ = self.client.reply_text(&reply_token, &text).await;
    }
}

fn hash_chat(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(id.as_bytes());
    let out = h.finalize();
    hex::encode(&out[..4])
}
```

Note: `slow_response.rs` needs added helpers `peek_state`, `mark_button_sent`, `was_button_sent` — extend that module with a `button_sent: bool` field on each entry and the three accessors.

- [ ] **Step 2: Branch `bootstrap::spawn` on mode**

In `crates/core/src/line/bootstrap.rs`, the existing `spawn(config, input_tx)` becomes:

```rust
pub fn spawn(
    config: LineConfig,
    input_tx: mpsc::Sender<crate::shared_session::ShellInput>,
) -> LineSessionHandle {
    match config.mode {
        crate::line::mode::LineMode::Hosted => spawn_hosted(config, input_tx),
        crate::line::mode::LineMode::SelfHosted => spawn_self_hosted(config, input_tx),
    }
}
```

`spawn_hosted` is the current body unchanged. `spawn_self_hosted` builds a `DirectSink`, calls `direct::spawn::spawn(direct_config, sink)`, wraps the handle in a `LineSessionHandle`-compatible shape (status field maps to `state: "connected"`, `server_url` derived from `webhook_display_url()`, `pending_approvals: 0` until approver wired).

- [ ] **Step 3: Tests**

- `self_hosted_full_round_trip` integration test in `crates/core/tests/line_direct_e2e.rs`:
  - Mock LINE endpoint via wiremock
  - Spawn direct server on random port
  - POST signed webhook → assert sink invoked
  - Agent replies via oneshot → assert reply HTTP POST hits mock with correct payload
- `slow_response_button_then_postback` integration test:
  - Set threshold to 100ms
  - POST webhook, do not respond on oneshot for 200ms
  - Assert mock receives template-button POST
  - POST signed postback with show_response → assert mock receives reply with cached text using NEW token

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/shared_session.rs crates/core/src/line/bootstrap.rs crates/core/src/line/direct/slow_response.rs crates/core/tests/line_direct_e2e.rs
git commit -m "feat(line/direct): shared_session wiring + slow-response orchestration"
```

---

## Task 15: `LineApprover` — self_hosted deferring path

**Files:**
- Modify: `crates/core/src/line/approver.rs`

Spec: In self_hosted mode, if there's no valid replyToken when an approval is needed, the approver MUST NOT push. It denies the tool call (safe default) and logs `[line/direct] approval denied: NoValidReplyToken`. If a replyToken IS valid for the chat, the approver reuses Quick Reply via `reply_template_button` with `data` shape `tool:allow:<request_id>` / `tool:deny:<request_id>` — same data format the existing parser already handles, so `record_decision_from_postback` doesn't need changes.

- [ ] **Step 1: Add mode-aware branch**

Extend `LineApprover` with optional `direct_client: Option<Arc<DirectLineClient>>` + `reply_store: Option<Arc<ReplyTokenStore>>`. The `approve()` method:

```rust
async fn approve(&self, req: &ApprovalRequest) -> ApprovalDecision {
    // 1. Register pending (existing logic) — keeps text + postback parser path
    let request_id = self.register_pending(req);

    // 2a. Hosted: existing logic — push_with_buttons via relay
    if let Some(client) = &self.hosted_client {
        // ... existing
        return self.await_decision(request_id, self.timeout).await;
    }

    // 2b. Self-hosted: must have a valid reply token for the chat,
    //     else deny-safe.
    let direct = self.direct_client.as_ref().unwrap();
    let store = self.reply_store.as_ref().unwrap();
    let Some(chat_id) = req.chat_id.as_deref() else {
        eprintln!("[line/direct] approval denied: no chat_id");
        return ApprovalDecision::Deny;
    };
    let Some(token) = store.take(chat_id) else {
        eprintln!("[line/direct] approval denied: NoValidReplyToken");
        return ApprovalDecision::Deny;
    };
    let label_allow = format!("✅ อนุญาต");
    let data = format!("tool:allow:{}", request_id);
    if let Err(e) = direct.reply_template_button(
        &token, &req.prompt, &label_allow, &data
    ).await {
        eprintln!("[line/direct] approval prompt failed: {e}");
        return ApprovalDecision::Deny;
    }
    self.await_decision(request_id, self.timeout).await
}
```

- [ ] **Step 2: Tests**

- `selfhosted_approve_denies_when_no_token`
- `selfhosted_approve_sends_template_button_when_token_valid` (mock client)
- `selfhosted_approve_denies_on_send_failure`

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/line/approver.rs
git commit -m "feat(line/direct): LineApprover defer-deny + reply-token-only prompt"
```

---

## Task 16: GUI — mode toggle in `LineConnectModal`

**Files:**
- Modify: `frontend/src/components/LineConnectModal.tsx`
- Modify: `frontend/src/components/Sidebar.tsx`

The modal grows a radio at the top:

```
( ) Hosted (use thClaws relay)
( ) Self-hosted (your own LINE OA)
```

Hosted form = existing pairing code input (unchanged behavior).

Self-hosted form:
- `Channel Access Token` (password input, masked)
- `Channel Secret` (password input, masked)
- `Listen host` (default `0.0.0.0`)
- `Listen port` (default `8646`)
- `Public webhook URL` (read-only, computed: `<public_url>/line/webhook` or `http://<host>:<port>/line/webhook`)
- `Allowed user IDs` (textarea, CSV)
- `Allowed group IDs` (textarea, CSV)
- `Allowed room IDs` (textarea, CSV)
- `Slow-response threshold (s)` (number, default 45)
- Help text: **"thClaws binds this port locally. You are responsible for exposing it as a public HTTPS URL (subdomain + reverse proxy). Paste the public URL + `/line/webhook` into LINE Developers Console."**
- Buttons: `Save & Connect` / `Cancel`

Submit dispatches new IPC: `line_self_hosted_setup` with the form payload (secrets go to keychain via the existing `secrets.rs` keychain helper from the worker side; modal sends them plaintext over the local IPC channel since it's same-process).

Sidebar pill: when `state === "connected"` AND `mode === "self_hosted"` (new field on `LineStatus`), label changes from display_name → `"self-hosted · <port>"`. No relay picture/name fetch.

- [ ] **Step 1: TypeScript types**

Add to `Sidebar.tsx` type:

```ts
type LineStatus = {
  state: "connected" | "disconnected" | "connecting";
  mode?: "hosted" | "self_hosted";   // NEW
  server_url: string;
  pending_approvals: number;
  display_name?: string;
  picture_url?: string;
  bind_addr?: string;                 // NEW (self_hosted only)
};
```

- [ ] **Step 2: Modal split component**

Refactor `LineConnectModal.tsx` into:
- top-level radio (`mode` state)
- `<HostedForm onPair={...} />` (existing logic moved)
- `<SelfHostedForm onSubmit={(payload) => ipc.invoke("line_self_hosted_setup", payload)} />`

Common shell: title, close button, error banner.

- [ ] **Step 3: Manual test checklist**

After build (`npm run build` + `cargo build`):
- Open GUI → sidebar shows "Disconnected"
- Click LINE pill → modal opens with Hosted selected (back-compat)
- Switch to Self-hosted → form appears
- Fill in fake values → submit → expect error toast (no real LINE creds)
- Sidebar pill remains "Disconnected"
- Set real `LINE_CHANNEL_ACCESS_TOKEN` + `LINE_CHANNEL_SECRET` env, restart, fill non-secret fields → submit → sidebar pill shows "self-hosted · 8646"

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/LineConnectModal.tsx frontend/src/components/Sidebar.tsx
git commit -m "feat(gui): self-hosted LINE mode toggle + form"
```

---

## Task 17: `docs/line-self-hosted.md`

**Files:**
- Create: `docs/line-self-hosted.md`

Outline (write each section in full prose):

1. **What this is** — Reply-API-only LINE bridge for users who own their channel.
2. **Why two modes** — hosted = thClaws relay (try-it-quick); self_hosted = your channel, your data, your subdomain.
3. **What this is NOT** — no Push API, ever. Background/cron messages don't work. User must tap a button if a response is slow.
4. **Architecture** — diagram (LINE Platform → user's public subdomain → user's reverse proxy → thClaws `:8646/line/webhook` → agent → Reply API).
5. **Prerequisites** — LINE Developers Console account, channel access token + channel secret, public HTTPS endpoint (responsibility of user; not provided by thClaws).
6. **Recommended exposure methods** (table) — Cloudflare Tunnel · Caddy + Let's Encrypt · ngrok (dev only) · port forward.
7. **Setup steps**:
   a. Create channel in LINE Developers Console
   b. Enable webhook, paste your public URL + `/line/webhook`
   c. Disable "Auto-reply messages" (LINE bot setting)
   d. Run thClaws GUI → Sidebar → Connect LINE → Self-hosted
   e. Paste channel access token, channel secret, allowed user IDs
   f. Save → thClaws binds `0.0.0.0:8646` (or your override)
   g. Test by sending DM to your channel; expect agent reply within seconds
8. **Slow-response UX** — explain the postback button: when agent takes > 45s, you get "กำลังคิดอยู่ กดรับคำตอบเมื่อพร้อม" with a "รับคำตอบ" button. Tap it to fetch the cached response. If you don't tap, the response is lost (no Push fallback).
9. **Limitations** — laptop sleep = downtime; cron/notify not supported; subdomain + HTTPS = your job; reply token expires 60s (we use 50s); chunked replies (5 bubbles × 4500 chars) for long answers.
10. **Troubleshooting**:
    - `401 bad signature` → channel secret mismatch
    - `400 bad body` → not from LINE
    - No reply received → check allowlist, check container/process running, check public URL reaches your machine
    - Agent timeout → see slow-response UX
11. **Security checklist** — channel secret in keychain (preferred) or `.env` mode 600; allowlist always populated in production; webhook path behind HTTPS only; never log replyToken or channel secret.

- [ ] **Step 1: Write the doc**

Use prose, no marketing fluff. Cite exact env var names and file paths.

- [ ] **Step 2: Link from README**

In `README.md`, in the "What makes it different" or features section, add one bullet: `**LINE OA bridge — hosted or self-hosted.** Try via the thClaws relay (5-min setup) or run against your own LINE channel with full data sovereignty. See [docs/line-self-hosted.md](docs/line-self-hosted.md).`

- [ ] **Step 3: Commit**

```bash
git add docs/line-self-hosted.md README.md
git commit -m "docs: self-hosted LINE mode setup guide"
```

---

## Final validation — RESULT / IMPACT / RISK / TEST / NEXT STEP

After all 17 tasks land:

### RESULT
- 2 LINE modes coexist in one binary: `hosted` (existing relay) and `self_hosted` (new webhook).
- thClaws calls **only** LINE Reply API. No Push API symbol, no `push.line.me`, no `LINE_PUSH_URL` anywhere in `crates/core/src/line/direct/`.
- Slow agent responses → template button → postback flow caches the answer until the user requests it.
- Approver in self_hosted denies tool prompts when no valid reply token exists (safe default; no push).
- Hosted mode behavior unchanged (no protocol drift, no client method removals, no config-file breakage).

### IMPACT
- New surface: ~14 source files + ~25 tests in `crates/core/src/line/direct/` + GUI changes + 1 doc.
- New runtime: axum HTTP server listening on `:8646` only when `LineMode::SelfHosted`. Idle cost in hosted mode = zero.
- New dep: `hmac 0.12` (~10 KB, RustCrypto, no transitive churn).
- IPC additions: `line_self_hosted_setup` (form submit). Existing IPC unchanged.

### RISK
- **Public exposure is user's responsibility.** thClaws doesn't validate that the subdomain → port mapping is HTTPS-terminated. Doc must be explicit; sidebar shows local listen address only.
- **Replay window from LINE = ~60s** but token TTL is 50s; brief race where LINE-side dedup hasn't pruned but our store has. Acceptable: replay arrives → signature valid → dedup check rejects (same `webhookEventId`).
- **Slow-response button arrives in the same chat as the user's message.** If the user types another message before tapping, the new event resets reply_token but the old request_id is still PENDING — eventually pruned by reaper (1h for non-PENDING, 24h for PENDING). User experience: their second message gets answered normally; the original eventually resolves via postback or expires.
- **Approver in self_hosted blocks the tool call when no token is around.** Agents needing approval mid-turn must wait for the next inbound user message. Acceptable for v1 — alternative would require Push which is prohibited.

### TEST
Final command:
```bash
cargo test -p thclaws-core --lib line:: 
cargo test -p thclaws-core --test line_direct_e2e
```
Expected: existing 30+ tests + ~50 new tests, all pass. No `push` reference in `direct/` source (asserted by Task 11 Step 1 Step 3 test).

Manual sanity:
- Existing hosted users: pair via relay → unchanged.
- New self-hosted users: GUI flow → message LINE → reply within seconds.
- Slow path: set threshold to 5s in env, ask a 30-second question → see button → tap → see full answer.

### NEXT STEP
1. Operate `cargo test -p thclaws-core line:: --lib` baseline on `main` to record current test count.
2. Execute Task 1 in a feature branch; verify dep tree under `cargo tree -p thclaws-core | grep hmac` shows `hmac 0.12.x` only.
3. Walk Tasks 2 → 17 in order. Each task: commit before moving on. Hosted regression check after Task 14 (`cargo test -p thclaws-core --lib line::session line::client line::approver`).
4. Before merge: run full suite + manual GUI flow for both modes.
5. After merge: monitor `[line/direct]` logs for "NoValidReplyToken drop" frequency — sustained spikes signal users are typing into a closed window (laptop sleeping while OA chat is open).
