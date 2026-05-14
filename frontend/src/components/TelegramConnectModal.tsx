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
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [testChatId, setTestChatId] = useState("");
  const [testResult, setTestResult] = useState<{
    kind: "ok" | "fail";
    text: string;
  } | null>(null);
  const [testBusy, setTestBusy] = useState(false);

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
        } else {
          setError((msg.error as string) ?? "Telegram setup failed");
        }
      } else if (msg.type === "telegram_disconnect_result") {
        setBusy(false);
      } else if (msg.type === "telegram_test_result") {
        setTestBusy(false);
        if (msg.ok) {
          const uname = msg.bot_username as string | undefined;
          const id = msg.bot_id as number | undefined;
          const fname = msg.first_name as string | undefined;
          setTestResult({
            kind: "ok",
            text: `✅ Token valid — @${uname ?? "?"} (id ${id ?? "?"}${
              fname ? `, "${fname}"` : ""
            })`,
          });
        } else {
          setTestResult({
            kind: "fail",
            text: `❌ ${(msg.error as string) ?? "test failed"}`,
          });
        }
      } else if (msg.type === "telegram_send_test_result") {
        setTestBusy(false);
        if (msg.ok) {
          setTestResult({
            kind: "ok",
            text: `✅ Test message sent to chat ${msg.chat_id as number}.`,
          });
        } else {
          setTestResult({
            kind: "fail",
            text: `❌ ${(msg.error as string) ?? "send failed"}`,
          });
        }
      }
    });
    return unsub;
  }, []);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [onClose]);

  const handleSubmit = () => {
    if (!botToken.trim()) {
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
      bot_token: botToken.trim(),
      webhook_secret_token: webhookSecret.trim(),
      mode,
      webhook_host: webhookHost.trim() || "0.0.0.0",
      webhook_port: mode === "webhook" ? port : 8647,
      webhook_public_url: webhookPublicUrl.trim() || null,
      allowed_users: allowedUsers.trim(),
      allowed_chats: allowedChats.trim(),
      require_mention_in_groups: requireMention,
    });
  };

  const handleDisconnect = () => {
    setBusy(true);
    send({ type: "telegram_disconnect" });
  };

  const handleTestGetMe = () => {
    setTestResult(null);
    setTestBusy(true);
    send({ type: "telegram_test" });
  };

  const handleSendTest = () => {
    const cid = parseInt(testChatId, 10);
    if (!Number.isFinite(cid)) {
      setTestResult({
        kind: "fail",
        text: "❌ chat_id must be a number (your Telegram numeric user id).",
      });
      return;
    }
    setTestResult(null);
    setTestBusy(true);
    send({
      type: "telegram_send_test",
      chat_id: cid,
      text: "🧪 test message from thClaws — if you see this, bidirectional send works.",
    });
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

          {/* Test panel — appears whenever a token is stored in the
              keychain. getMe + send-test buttons surface the most
              common setup mistakes (revoked token, wrong chat_id)
              before they show up as silent drops in production. */}
          <div
            className="rounded p-2 space-y-2"
            style={{
              background: "var(--bg-primary)",
              border: "1px solid var(--border)",
            }}
          >
            <div
              className="text-xs font-semibold"
              style={{ color: "var(--text-primary)" }}
            >
              🧪 Test thClaws ↔ Telegram
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <button
                onClick={handleTestGetMe}
                disabled={testBusy}
                className="px-2 py-1 rounded text-xs"
                style={{
                  background: "var(--bg-secondary)",
                  border: "1px solid var(--border)",
                }}
              >
                {testBusy ? "…" : "Test getMe (verify token)"}
              </button>
              <input
                type="text"
                value={testChatId}
                onChange={(e) => setTestChatId(e.target.value)}
                placeholder="chat_id (your tg user id)"
                className="px-2 py-1 rounded font-mono text-xs flex-1 min-w-[140px]"
                style={{
                  background: "var(--bg-secondary)",
                  border: "1px solid var(--border)",
                  color: "var(--text-primary)",
                }}
              />
              <button
                onClick={handleSendTest}
                disabled={testBusy || !testChatId.trim()}
                className="px-2 py-1 rounded text-xs"
                style={{
                  background: "var(--bg-secondary)",
                  border: "1px solid var(--border)",
                }}
              >
                Send test
              </button>
            </div>
            {testResult && (
              <div
                className="text-xs p-2 rounded"
                style={{
                  background:
                    testResult.kind === "ok"
                      ? "rgba(80,200,120,0.12)"
                      : "rgba(255,80,80,0.12)",
                  color:
                    testResult.kind === "ok"
                      ? "var(--success, #50c878)"
                      : "var(--error, #f08080)",
                  fontFamily: "var(--font-mono, monospace)",
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                }}
              >
                {testResult.text}
              </div>
            )}
          </div>

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
            <div className="text-xs mb-1" style={{ color: "var(--text-secondary)" }}>
              Bot token (from @BotFather)
            </div>
            <input
              type="password"
              value={botToken}
              onChange={(e) => setBotToken(e.target.value)}
              placeholder="123456:ABC-DEF..."
              className="w-full px-2 py-1 rounded font-mono text-xs"
              style={{
                background: "var(--bg-primary)",
                border: "1px solid var(--border)",
                color: "var(--text-primary)",
              }}
            />
          </label>

          {/* Advanced — collapsed by default. Default (collapsed) =
              long-poll mode, empty allowlist (open mode — forward all
              DMs to this bot), require @mention in groups = true. */}
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
                · long-poll · allowlist empty (open)
              </span>
            )}
          </button>

          {showAdvanced && (
            <>
              <div
                className="flex items-start gap-2 p-2 rounded text-xs"
                style={{
                  background: "rgba(209,154,102,0.12)",
                  color: "var(--warning, #d19a66)",
                }}
              >
                <AlertCircle size={12} className="mt-0.5 shrink-0" />
                <span>
                  Empty allowlist = <strong>open mode</strong> — any Telegram
                  user who discovers <code>@{status.bot_username ?? "bot"}</code>{" "}
                  can DM and drive thClaws. Keep the bot username private, or
                  add numeric user IDs to lock down.
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
