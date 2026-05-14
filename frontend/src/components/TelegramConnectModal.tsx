import { useEffect, useState } from "react";
import {
  X,
  Send,
  CheckCircle2,
  AlertCircle,
  Info,
  ChevronDown,
  ChevronRight,
} from "lucide-react";
import { send, subscribe } from "../hooks/useIPC";

/// Telegram bot bridge connect modal. Mirrors LineConnectModal's
/// self-hosted variant (no relay; user owns the bot token).
///
/// Long-poll is the default — zero infrastructure, thClaws hits
/// `api.telegram.org` from the local machine. Webhook is optional
/// for production setups that want lower latency; user is responsible
/// for HTTPS termination + `setWebhook` registration.

type Status = {
  state: "connected" | "disconnected";
  mode?: "long_poll" | "webhook";
  bind_addr?: string;
  bot_username?: string;
};

type Mode = "long_poll" | "webhook";

export function TelegramConnectModal({ onClose }: { onClose: () => void }) {
  const [status, setStatus] = useState<Status>({ state: "disconnected" });
  const [mode, setMode] = useState<Mode>("long_poll");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [botToken, setBotToken] = useState("");
  const [webhookSecret, setWebhookSecret] = useState("");
  const [webhookHost, setWebhookHost] = useState("0.0.0.0");
  const [webhookPort, setWebhookPort] = useState("8647");
  const [webhookPublicUrl, setWebhookPublicUrl] = useState("");
  const [allowedUsers, setAllowedUsers] = useState("");
  const [allowedChats, setAllowedChats] = useState("");
  const [requireMention, setRequireMention] = useState(true);
  const [allowOpenMode, setAllowOpenMode] = useState(false);
  const [allowAnyUserInGroup, setAllowAnyUserInGroup] = useState(false);
  const [autoApproveAll, setAutoApproveAll] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  /// Token preview shown next to the bot-token input when a token is
  /// already saved in the OS keychain. User can type a new token to
  /// override, or leave the field blank and click Connect to reuse
  /// the saved one.
  const [savedTokenPreview, setSavedTokenPreview] = useState<string | null>(
    null
  );

  useEffect(() => {
    const unsub = subscribe((msg) => {
      if (msg.type === "telegram_status") {
        setStatus({
          state: (msg.state as Status["state"]) ?? "disconnected",
          mode: msg.mode as Status["mode"],
          bind_addr: msg.bind_addr as string | undefined,
          bot_username: msg.bot_username as string | undefined,
        });
      } else if (msg.type === "telegram_setup_result") {
        setBusy(false);
        if (msg.ok) {
          setError(null);
          // Clear secret inputs so they don't linger in the DOM.
          setBotToken("");
          setWebhookSecret("");
          // Auto-close on successful Save/Connect — ลุงจืด wants
          // the modal to confirm + dismiss itself once persistence
          // succeeded.
          onClose();
        } else {
          setError((msg.error as string) ?? "Telegram setup failed");
        }
      } else if (msg.type === "telegram_disconnect_result") {
        setBusy(false);
      } else if (msg.type === "telegram_token_status") {
        if (msg.has_token && typeof msg.preview === "string") {
          setSavedTokenPreview(msg.preview as string);
        } else {
          setSavedTokenPreview(null);
        }
      } else if (msg.type === "telegram_config_status") {
        // Pre-fill the form from the saved telegram.json so reopening
        // the modal shows what's actually persisted (was: every field
        // reset to defaults regardless of saved state).
        if (msg.has_config) {
          if (typeof msg.mode === "string") {
            setMode(
              (msg.mode as string) === "webhook" ? "webhook" : "long_poll"
            );
          }
          if (typeof msg.webhook_host === "string") {
            setWebhookHost(msg.webhook_host as string);
          }
          if (typeof msg.webhook_port === "number") {
            setWebhookPort(String(msg.webhook_port as number));
          }
          if (typeof msg.webhook_public_url === "string") {
            setWebhookPublicUrl(msg.webhook_public_url as string);
          }
          if (typeof msg.allowed_users === "string") {
            setAllowedUsers(msg.allowed_users as string);
          }
          if (typeof msg.allowed_chats === "string") {
            setAllowedChats(msg.allowed_chats as string);
          }
          if (typeof msg.require_mention_in_groups === "boolean") {
            setRequireMention(msg.require_mention_in_groups as boolean);
          }
          if (typeof msg.allow_open_mode === "boolean") {
            setAllowOpenMode(msg.allow_open_mode as boolean);
          }
          if (typeof msg.allow_any_user_in_group === "boolean") {
            setAllowAnyUserInGroup(msg.allow_any_user_in_group as boolean);
          }
          if (typeof msg.auto_approve_all === "boolean") {
            setAutoApproveAll(msg.auto_approve_all as boolean);
          }
          // Open Advanced if anything beyond bare defaults is saved
          // so the user can see at-a-glance what's already locked in.
          const nonDefault =
            (msg.allowed_users as string)?.trim().length > 0 ||
            (msg.allowed_chats as string)?.trim().length > 0 ||
            (msg.mode as string) === "webhook" ||
            (msg.allow_open_mode as boolean) === true ||
            (msg.allow_any_user_in_group as boolean) === true;
          if (nonDefault) setShowAdvanced(true);
        }
      }
    });
    send({ type: "telegram_token_status" });
    send({ type: "telegram_config_status" });
    send({ type: "telegram_status" });
    return unsub;
  }, [onClose]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [onClose]);

  const submitSetup = (connect: boolean) => {
    // Empty token + saved token in keychain → reuse the saved one
    // (backend resolves on receipt). Empty token + no saved token →
    // surface the requirement locally for a fast error.
    if (!botToken.trim() && !savedTokenPreview) {
      setError("Bot token is required.");
      return;
    }
    if (mode === "webhook" && !webhookSecret.trim()) {
      setError("Webhook secret_token is required for webhook mode.");
      return;
    }
    const port = parseInt(webhookPort, 10);
    if (mode === "webhook" && !Number.isFinite(port)) {
      setError("Webhook port must be a number.");
      return;
    }
    setError(null);
    setBusy(true);
    send({
      type: "telegram_setup",
      connect,
      bot_token: botToken.trim(),
      webhook_secret_token: webhookSecret.trim(),
      mode,
      webhook_host: webhookHost.trim() || "0.0.0.0",
      webhook_port: mode === "webhook" ? port : 8647,
      webhook_public_url: webhookPublicUrl.trim() || null,
      allowed_users: allowedUsers.trim(),
      allowed_chats: allowedChats.trim(),
      require_mention_in_groups: requireMention,
      allow_open_mode: allowOpenMode,
      allow_any_user_in_group: allowAnyUserInGroup,
      auto_approve_all: autoApproveAll,
    });
  };

  const handleSubmit = () => submitSetup(true);
  const handleSaveOnly = () => submitSetup(false);

  const handleDisconnect = () => {
    setBusy(true);
    send({ type: "telegram_disconnect" });
  };

  const connected = status.state === "connected";

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ background: "rgba(0,0,0,0.5)" }}
      onClick={onClose}
    >
      <div
        className="rounded-md shadow-2xl w-[640px] max-h-[88vh] overflow-y-auto"
        style={{
          background: "var(--bg-secondary)",
          border: "1px solid var(--border)",
          color: "var(--text-primary)",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div
          className="flex items-center justify-between px-4 py-2 border-b"
          style={{ borderColor: "var(--border)" }}
        >
          <div className="flex items-center gap-2">
            <Send size={14} />
            <span className="font-semibold">Telegram Connect</span>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded hover:bg-gray-700"
            aria-label="Close"
          >
            <X size={14} />
          </button>
        </div>

        <div className="p-4 space-y-3 text-sm">
          {connected ? (
            <div
              className="flex items-start gap-2 p-2 rounded"
              style={{ background: "var(--bg-primary)" }}
            >
              <CheckCircle2 size={14} className="mt-0.5 shrink-0" />
              <div className="flex-1">
                <div className="font-medium">Bridge connected</div>
                <div
                  className="text-xs mt-0.5"
                  style={{ color: "var(--text-secondary)" }}
                >
                  Mode: <code>{status.mode}</code>
                  {status.bot_username && (
                    <>
                      {" · "}@{status.bot_username}
                    </>
                  )}
                  {status.bind_addr && (
                    <>
                      {" · "}listen <code>{status.bind_addr}</code>
                    </>
                  )}
                </div>
              </div>
              <button
                onClick={handleDisconnect}
                disabled={busy}
                className="px-2 py-1 rounded text-xs"
                style={{
                  background: "var(--bg-primary)",
                  border: "1px solid var(--border)",
                }}
              >
                Disconnect
              </button>
            </div>
          ) : null}

          {!connected && (
            <div
              className="flex items-start gap-2 p-2 rounded text-xs"
              style={{ background: "var(--bg-primary)" }}
            >
              <Info size={12} className="mt-0.5 shrink-0" />
              <div>
                Create a bot in <a
                  href="https://t.me/BotFather"
                  target="_blank"
                  rel="noreferrer"
                  className="underline"
                >@BotFather</a>{" "}
                with <code>/newbot</code>, copy the token, then paste below.
                Numeric user IDs go in the allowlist (find yours by DMing the
                bot once and watching <code>thclaws logs</code>).
              </div>
            </div>
          )}

          <label className="block">
            <div
              className="text-xs mb-1 flex items-center gap-2"
              style={{ color: "var(--text-secondary)" }}
            >
              <span>Bot token (from @BotFather)</span>
              {savedTokenPreview && (
                <span
                  className="font-mono"
                  style={{ color: "var(--accent)", opacity: 0.85 }}
                  title="Saved in OS keychain. Leave blank to reuse."
                >
                  saved: {savedTokenPreview}
                </span>
              )}
            </div>
            <input
              type="password"
              value={botToken}
              onChange={(e) => setBotToken(e.target.value)}
              placeholder={
                savedTokenPreview
                  ? "(leave blank to reuse saved token, or paste new to override)"
                  : "123456:ABC-DEF..."
              }
              className="w-full px-2 py-1 rounded font-mono text-xs"
              style={{
                background: "var(--bg-primary)",
                border: "1px solid var(--border)",
                color: "var(--text-primary)",
              }}
            />
          </label>

          {/* Advanced — collapsed by default. Default posture is
              FAIL-CLOSED: empty allowlist denies every update; group
              auth requires BOTH chat_id and sender user_id; mention
              gating in groups drops on getMe failure. The flags
              below are explicit opt-ins for the dev / single-owner
              postures that lose those guards. */}
          <button
            type="button"
            onClick={() => setShowAdvanced((v) => !v)}
            className="flex items-center gap-1 text-xs"
            style={{ color: "var(--text-secondary)" }}
          >
            {showAdvanced ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            Advanced{" "}
            {!showAdvanced && (
              <span style={{ color: "var(--text-secondary)", opacity: 0.7 }}>
                · long-poll · fail-closed allowlist
              </span>
            )}
          </button>

          {showAdvanced && (
            <>
              <div
                className="flex items-start gap-2 p-2 rounded text-xs"
                style={{
                  background: "rgba(80,200,120,0.10)",
                  color: "var(--text-primary)",
                }}
              >
                <Info size={12} className="mt-0.5 shrink-0" />
                <span>
                  Default = fail-closed. Empty allowlist denies every
                  inbound update; groups require both the chat_id AND
                  the sender's numeric Telegram user ID; group mention
                  gating drops if <code>getMe</code> fails. Toggle the
                  flags below only when you accept the trade-offs.
                </span>
              </div>

          <div className="space-y-1">
            <div className="text-xs" style={{ color: "var(--text-secondary)" }}>
              Transport mode
            </div>
            <div className="flex gap-3">
              <label className="flex items-center gap-1 cursor-pointer">
                <input
                  type="radio"
                  checked={mode === "long_poll"}
                  onChange={() => setMode("long_poll")}
                />
                <span>Long-polling (default, no public URL)</span>
              </label>
              <label className="flex items-center gap-1 cursor-pointer">
                <input
                  type="radio"
                  checked={mode === "webhook"}
                  onChange={() => setMode("webhook")}
                />
                <span>Webhook (production)</span>
              </label>
            </div>
          </div>

          {mode === "webhook" && (
            <>
              <label className="block">
                <div className="text-xs mb-1" style={{ color: "var(--text-secondary)" }}>
                  Webhook secret_token (sent in X-Telegram-Bot-Api-Secret-Token)
                </div>
                <input
                  type="password"
                  value={webhookSecret}
                  onChange={(e) => setWebhookSecret(e.target.value)}
                  placeholder="any random 32+ char string"
                  className="w-full px-2 py-1 rounded font-mono text-xs"
                  style={{
                    background: "var(--bg-primary)",
                    border: "1px solid var(--border)",
                    color: "var(--text-primary)",
                  }}
                />
              </label>

              <div className="grid grid-cols-3 gap-2">
                <label className="block col-span-2">
                  <div className="text-xs mb-1" style={{ color: "var(--text-secondary)" }}>
                    Bind host
                  </div>
                  <input
                    type="text"
                    value={webhookHost}
                    onChange={(e) => setWebhookHost(e.target.value)}
                    className="w-full px-2 py-1 rounded font-mono text-xs"
                    style={{
                      background: "var(--bg-primary)",
                      border: "1px solid var(--border)",
                      color: "var(--text-primary)",
                    }}
                  />
                </label>
                <label className="block">
                  <div className="text-xs mb-1" style={{ color: "var(--text-secondary)" }}>
                    Port
                  </div>
                  <input
                    type="text"
                    value={webhookPort}
                    onChange={(e) => setWebhookPort(e.target.value)}
                    className="w-full px-2 py-1 rounded font-mono text-xs"
                    style={{
                      background: "var(--bg-primary)",
                      border: "1px solid var(--border)",
                      color: "var(--text-primary)",
                    }}
                  />
                </label>
              </div>

              <label className="block">
                <div className="text-xs mb-1" style={{ color: "var(--text-secondary)" }}>
                  Public URL (optional, for display in sidebar; you still run setWebhook yourself)
                </div>
                <input
                  type="text"
                  value={webhookPublicUrl}
                  onChange={(e) => setWebhookPublicUrl(e.target.value)}
                  placeholder="https://tg.example.com"
                  className="w-full px-2 py-1 rounded font-mono text-xs"
                  style={{
                    background: "var(--bg-primary)",
                    border: "1px solid var(--border)",
                    color: "var(--text-primary)",
                  }}
                />
              </label>
            </>
          )}

          <label className="block">
            <div className="text-xs mb-1" style={{ color: "var(--text-secondary)" }}>
              Allowed user IDs (numeric, comma-separated; supports <code>tg:</code> prefix)
            </div>
            <input
              type="text"
              value={allowedUsers}
              onChange={(e) => setAllowedUsers(e.target.value)}
              placeholder="123456789, 987654321"
              className="w-full px-2 py-1 rounded font-mono text-xs"
              style={{
                background: "var(--bg-primary)",
                border: "1px solid var(--border)",
                color: "var(--text-primary)",
              }}
            />
          </label>

          <label className="block">
            <div className="text-xs mb-1" style={{ color: "var(--text-secondary)" }}>
              Allowed group chat IDs (typically negative numbers, comma-separated)
            </div>
            <input
              type="text"
              value={allowedChats}
              onChange={(e) => setAllowedChats(e.target.value)}
              placeholder="-1001234567890"
              className="w-full px-2 py-1 rounded font-mono text-xs"
              style={{
                background: "var(--bg-primary)",
                border: "1px solid var(--border)",
                color: "var(--text-primary)",
              }}
            />
          </label>

          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={requireMention}
              onChange={(e) => setRequireMention(e.target.checked)}
            />
            <span className="text-xs">
              Require <code>@mention</code> or direct reply in groups
              (matches Telegram&apos;s default privacy mode)
            </span>
          </label>

          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={allowOpenMode}
              onChange={(e) => setAllowOpenMode(e.target.checked)}
            />
            <span className="text-xs">
              <strong style={{ color: "var(--warning, #d19a66)" }}>
                Allow open mode
              </strong>{" "}
              — empty allowlist forwards every DM. Use only for
              single-owner bots whose username is kept private.
            </span>
          </label>

          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={allowAnyUserInGroup}
              onChange={(e) => setAllowAnyUserInGroup(e.target.checked)}
            />
            <span className="text-xs">
              Allow any user in allow-listed groups (skip sender
              user_id check inside groups — chat membership is the
              only gate)
            </span>
          </label>

          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={autoApproveAll}
              onChange={(e) => setAutoApproveAll(e.target.checked)}
            />
            <span className="text-xs">
              <strong style={{ color: "var(--warning, #d19a66)" }}>
                Auto-approve all tools
              </strong>{" "}
              — skip the in-chat Approve/Deny prompt. Every tool call
              (including Bash/Edit/Write) runs without confirmation
              while this bridge is connected.
            </span>
          </label>
            </>
          )}

          {error && (
            <div
              className="flex items-start gap-2 p-2 rounded text-xs"
              style={{
                background: "rgba(255,80,80,0.1)",
                color: "var(--error, #f08080)",
              }}
            >
              <AlertCircle size={12} className="mt-0.5 shrink-0" />
              <span>{error}</span>
            </div>
          )}

          <div className="flex justify-end gap-2 pt-2">
            <button
              onClick={onClose}
              className="px-3 py-1 rounded text-xs"
              style={{
                background: "var(--bg-primary)",
                border: "1px solid var(--border)",
              }}
            >
              Cancel
            </button>
            <button
              onClick={handleSaveOnly}
              disabled={busy}
              className="px-3 py-1 rounded text-xs"
              style={{
                background: "var(--bg-primary)",
                border: "1px solid var(--border)",
                color: "var(--text-primary)",
              }}
              title="Persist settings to telegram.json + keychain without spawning the bridge. Useful for prepping config without immediately going live."
            >
              Save
            </button>
            <button
              onClick={handleSubmit}
              disabled={busy}
              className="px-3 py-1 rounded text-xs"
              style={{
                background: "var(--accent)",
                color: "var(--bg-primary)",
              }}
            >
              {busy ? "Connecting…" : connected ? "Reconnect" : "Connect"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
