//! Boot a `LineSession` from a stored `LineConfig` and own its
//! lifetime for the worker.
//!
//! Phase 2: replaces the Phase-1.3 placeholder `EchoHandler` with
//! `WorkerForwardHandler`, which routes each inbound LINE message
//! into the worker's `ShellInput::LineMessage` channel. The worker
//! drives `Agent::run_turn`, captures the final assistant text,
//! and answers via a `oneshot::Sender`; this handler returns that
//! captured text so the existing `LineSession::SessionSink` posts
//! the LINE reply unchanged.
//!
//! `LineSessionHandle` is what the worker stashes — it bundles
//! the cancel token (for `/disconnect`) with a status snapshot
//! the IPC layer can render.

use std::sync::mpsc;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::oneshot;

use super::approver::LineApprover;
use super::client::LineClient;
use super::config::LineConfig;
use super::direct::config::DirectConfig;
use super::direct::sink::DirectSink;
use super::direct::spawn::DirectHandle;
use super::mode::LineMode;
use super::session::{LineMessageHandler, LineSession};
use crate::cancel::CancelToken;

/// Phase 2 handler: forward LINE messages to the worker via the
/// `ShellInput::LineMessage` channel, wait for the captured agent
/// reply, return it so the `LineSession` posts the LINE response.
///
/// On worker channel closure (very unlikely — worker dropped) we
/// return a "thClaws worker unavailable" fallback so the LINE user
/// at least sees *something* instead of dead silence.
struct WorkerForwardHandler {
    input_tx: mpsc::Sender<crate::shared_session::ShellInput>,
}

#[async_trait]
impl LineMessageHandler for WorkerForwardHandler {
    async fn handle_message(&self, text: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        // Worker's input channel is `std::sync::mpsc`, which is
        // synchronous + unbounded, so `send()` returns immediately
        // without `.await`. The matching oneshot::Receiver below
        // IS async — that's what makes us wait for the turn.
        if self
            .input_tx
            .send(crate::shared_session::ShellInput::LineMessage { text, respond: tx })
            .is_err()
        {
            return Some("⚠️ thClaws worker is unavailable; restart thClaws and try again.".into());
        }
        match rx.await {
            Ok(s) if !s.trim().is_empty() => Some(s),
            // Empty / dropped sender — agent finished but produced
            // no assistant text (e.g. tool-only turn). Surface a
            // gentle hint rather than absolute silence.
            _ => Some("(thClaws agent finished the turn without a text reply.)".into()),
        }
    }
}

/// Live LINE-bridge handle stored on the worker. Dropping it
/// alone won't cancel the session — fire `cancel.cancel()` first
/// (the IPC `line_disconnect` arm does this).
pub struct LineSessionHandle {
    pub cancel: CancelToken,
    pub status: LineStatus,
    /// JoinHandle so the worker can await graceful shutdown if
    /// it ever needs to. Not surfaced via IPC.
    pub join: tokio::task::JoinHandle<()>,
    /// Shared approver — the agent's `ApprovalSink` swaps to this
    /// while LINE is connected. Same instance the LineSession
    /// holds, so postbacks / text replies resolve the same set
    /// of pending decisions.
    pub approver: Arc<LineApprover>,
    /// Hosted-mode relay client. `None` in self-hosted mode (no
    /// relay endpoint to call). Callers that fan out chat-bridge
    /// events check `.is_some()` first.
    pub client: Option<Arc<LineClient>>,
    /// Self-hosted webhook handle. `None` in hosted mode. Owned so
    /// the worker can keep the listening port alive for the
    /// session lifetime.
    pub direct: Option<DirectHandle>,
    /// Which bridge runtime is active.
    pub mode: LineMode,
}

/// Snapshot of the bridge's state. Serialised into the
/// `chat_line_status` IPC payload so the GUI sidebar /
/// LineConnectModal can render an accurate pill.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LineStatus {
    /// `"connected"` once the session task has been spawned.
    /// More granular states (`"connecting"`, `"reconnecting"`)
    /// are Phase 2 work — the WS client logs them internally for
    /// now, but the GUI just sees a boolean.
    pub state: &'static str,
    /// The relay URL the bridge connects to. Always safe to
    /// surface in the UI (no token).
    pub server_url: String,
    /// Number of pending approvals at the time of snapshot.
    /// Lets the sidebar pill flash when LINE is waiting for the
    /// user's tap.
    pub pending_approvals: usize,
}

impl LineStatus {
    pub fn disconnected() -> Self {
        Self {
            state: "disconnected",
            server_url: String::new(),
            pending_approvals: 0,
        }
    }

    pub fn connected(server_url: String) -> Self {
        Self {
            state: "connected",
            server_url,
            pending_approvals: 0,
        }
    }
}

/// Spawn the LINE bridge appropriate for the configured mode.
///
/// `input_tx` is the worker's `ShellInput` channel sender — every
/// inbound LINE message lands as `ShellInput::LineMessage`,
/// runs through the agent loop, and the captured assistant text is
/// shipped back to LINE.
///
/// Returns `Err` only when self-hosted initialisation fails (e.g.
/// missing channel access token / secret, port bind error). Hosted
/// mode is infallible because it only spawns a background WS task.
pub async fn spawn(
    config: LineConfig,
    input_tx: mpsc::Sender<crate::shared_session::ShellInput>,
    events_tx: tokio::sync::broadcast::Sender<crate::shared_session::ViewEvent>,
) -> Result<LineSessionHandle, String> {
    match config.mode {
        LineMode::Hosted => Ok(spawn_hosted(config, input_tx)),
        LineMode::SelfHosted => spawn_self_hosted(config, input_tx, events_tx).await,
    }
}

/// Hosted (relay-based) bridge — original behaviour, unchanged.
fn spawn_hosted(
    config: LineConfig,
    input_tx: mpsc::Sender<crate::shared_session::ShellInput>,
) -> LineSessionHandle {
    let cancel = CancelToken::new();
    let server_url = config.resolved_server_url();
    let handler: Arc<dyn LineMessageHandler> = Arc::new(WorkerForwardHandler { input_tx });

    let client = Arc::new(LineClient::new(config.clone()).with_cancel(cancel.clone()));
    let approver = Arc::new(LineApprover::new(client.clone()));

    let session = Arc::new(
        LineSession::new(config, handler)
            .with_approver(approver.clone())
            .with_cancel(cancel.clone()),
    );
    let cancel_for_task = cancel.clone();
    let join = tokio::spawn(async move {
        if let Err(e) = session.run().await {
            eprintln!("[line] session ended: {e}");
        }
        cancel_for_task.cancel();
    });

    LineSessionHandle {
        cancel,
        status: LineStatus::connected(server_url),
        join,
        approver,
        client: Some(client),
        direct: None,
        mode: LineMode::Hosted,
    }
}

/// Self-hosted bridge — bind webhook + use LINE Reply API directly.
async fn spawn_self_hosted(
    config: LineConfig,
    input_tx: mpsc::Sender<crate::shared_session::ShellInput>,
    events_tx: tokio::sync::broadcast::Sender<crate::shared_session::ViewEvent>,
) -> Result<LineSessionHandle, String> {
    // Take the on-disk DirectConfig if the GUI populated one,
    // otherwise fall back to env-only configuration. The two paths
    // produce equivalent shapes; env wins when both are set.
    let direct_config = config.direct.clone().unwrap_or_else(DirectConfig::from_env);

    // Build the slow-response cache + reply store BEFORE spawning
    // so we can hand the same Arc<>'s to both the sink and the
    // approver. spawn() also constructs internal copies — those are
    // the ones the server thread writes to. To keep both views
    // consistent, sink + approver consume the handles spawn returns.
    //
    // (Building two separate caches would silently break the
    // postback path — the server would write to one and the sink
    // would read from another.)

    // Construct the sink without the underlying client yet — we'll
    // wire it post-spawn via the trait object swap below.
    // First spawn the server with a placeholder no-op sink so the
    // port binds and we get back the shared Arc<DirectLineClient>;
    // we then replace the sink. Simpler approach: build a temporary
    // sink that forwards into a channel, then re-route. But the
    // simpler-still solution is: build the DirectSink BEFORE
    // spawn by constructing the client + stores here directly and
    // passing them to spawn via constructor injection.
    //
    // To keep the data flow obvious, we replicate that strategy: we
    // Build every shared dependency (client + stores) OUT HERE, then
    // hand them into a single `spawn_with_stores` call. Sink and
    // server then read/write the SAME Arc<ReplyTokenStore> — without
    // this, the older placeholder-then-respawn dance built two
    // disjoint stores and every reply silently dropped with
    // `NoValidReplyToken`.
    let access_token = super::direct::spawn::load_access_token_pub()
        .map_err(|e| format!("self_hosted access_token: {e}"))?;
    let client = Arc::new(
        super::direct::client::DirectLineClient::new(access_token)
            .map_err(|e| format!("self_hosted client init: {e}"))?,
    );
    let reply_store = Arc::new(super::direct::reply_store::ReplyTokenStore::new());
    let slow_cache = Arc::new(super::direct::slow_response::SlowResponseCache::new());
    let dedup = Arc::new(super::direct::dedup::DedupStore::new());

    let approver = Arc::new(LineApprover::for_self_hosted(
        client.clone(),
        reply_store.clone(),
    ));
    let real_sink: Arc<dyn super::direct::server::DirectEventSink> = Arc::new(
        DirectSink::new(
            input_tx.clone(),
            client.clone(),
            reply_store.clone(),
            slow_cache.clone(),
            direct_config.threshold(),
        )
        .with_approver(approver.clone()),
    );
    // Optional LIFF bridge. Only enabled when a LIFF id is configured —
    // otherwise the WS surface stays unregistered. Allowlist is shared
    // with the webhook gate so the same user IDs auth both surfaces.
    let liff_bridge = direct_config.liff_id.as_ref().map(|id| {
        Arc::new(super::direct::liff::LiffBridge {
            input_tx: input_tx.clone(),
            events_tx: events_tx.clone(),
            allowlist: Arc::new(super::direct::allowlist::Allowlist::from_csv(
                &direct_config.allowed_users_csv,
                &direct_config.allowed_groups_csv,
                &direct_config.allowed_rooms_csv,
            )),
            liff_id: Some(id.clone()),
        })
    });

    let direct_handle = super::direct::spawn::spawn_with_stores(
        direct_config.clone(),
        real_sink,
        client.clone(),
        reply_store.clone(),
        slow_cache.clone(),
        dedup.clone(),
        liff_bridge,
    )
    .await
    .map_err(|e| format!("self_hosted spawn: {e}"))?;

    let server_url = direct_config.webhook_display_url();
    let cancel = direct_handle.cancel.clone();
    // `approver` was constructed before `real_sink` so the sink could
    // own an `Arc<LineApprover>` for the postback path. Reuse it as
    // the handle's approver — same instance, no duplicate pending
    // registry.
    // Self-hosted has no separate session loop — the axum server IS
    // the session. We provide a dummy join handle that resolves as
    // soon as the cancel fires so the worker's existing
    // `handle.join` plumbing keeps working.
    let cancel_for_dummy = cancel.clone();
    let join = tokio::spawn(async move {
        cancel_for_dummy.cancelled().await;
    });

    Ok(LineSessionHandle {
        cancel,
        status: LineStatus::connected(server_url),
        join,
        approver,
        client: None,
        direct: Some(direct_handle),
        mode: LineMode::SelfHosted,
    })
}

/// No-op sink used as a placeholder during the spawn dance above.
#[derive(Default)]
struct NoopSink;

#[async_trait]
impl super::direct::server::DirectEventSink for NoopSink {
    async fn on_message(&self, _: String, _: String, _: String) {}
    async fn on_show_response_postback(&self, _: String, _: String, _: String) {}
}
