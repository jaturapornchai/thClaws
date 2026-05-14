import { useEffect, useState } from "react";
import { X, MessageCircle, CheckCircle2, AlertCircle, Info } from "lucide-react";
import { send, subscribe } from "../hooks/useIPC";

/// Pair-then-status modal for the LINE bridge.
///
/// Two modes:
/// - **Hosted** — pair via the thClaws relay using an 8-char code.
///   Original Phase-1.3 flow; unchanged.
/// - **Self-hosted** — user runs their own LINE OA. Provides channel
///   credentials + allowlist; thClaws listens on a local port and
///   the user fronts it with their own subdomain + reverse proxy.
///   Reply API only — no Push.

type Status = {
  state: "connected" | "disconnected";
  mode?: "hosted" | "self_hosted";
  server_url: string;
  pending_approvals: number;
};

type Mode = "hosted" | "self_hosted";

export function LineConnectModal({ onClose }: { onClose: () => void }) {
  const [status, setStatus] = useState<Status>({
    state: "disconnected",
    server_url: "",
    pending_approvals: 0,
  });
  const [mode, setMode] = useState<Mode>("hosted");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Self-hosted form state
  const [accessToken, setAccessToken] = useState("");
  const [channelSecret, setChannelSecret] = useState("");
  const [bindHost, setBindHost] = useState("0.0.0.0");
  const [bindPort, setBindPort] = useState("8646");
  const [publicUrl, setPublicUrl] = useState("");
  const [allowedUsers, setAllowedUsers] = useState("");
  const [allowedGroups, setAllowedGroups] = useState("");
  const [allowedRooms, setAllowedRooms] = useState("");
  const [threshold, setThreshold] = useState("45");
  const [autoApproveAll, setAutoApproveAll] = useState(false);

  useEffect(() => {
    const unsub = subscribe((msg) => {
      if (msg.type === "line_status") {
        setStatus({
          state: (msg.state as Status["state"]) ?? "disconnected",
          mode: msg.mode as Status["mode"],
          server_url: (msg.server_url as string) ?? "",
          pending_approvals: (msg.pending_approvals as number) ?? 0,
        });
      } else if (msg.type === "line_pair_result") {
        setBusy(false);
        if (msg.ok) {
          setCode("");
          setError(null);
        } else {
          setError((msg.error as string) ?? "pairing failed");
        }
      } else if (msg.type === "line_self_hosted_setup_result") {
        setBusy(false);
        if (msg.ok) {
          setError(null);
          // Clear secret inputs so they don't linger in DOM.
          setAccessToken("");
          setChannelSecret("");
        } else {
          setError((msg.error as string) ?? "self-hosted setup failed");
        }
      } else if (msg.type === "line_disconnect_ack") {
        setBusy(false);
      }
    });
    send({ type: "line_status" });
    return unsub;
  }, []);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [onClose]);

  const handleHostedConnect = () => {
    const trimmed = code.trim().toUpperCase();
    if (trimmed.length === 0) return;
    setError(null);
    setBusy(true);
    send({ type: "line_pair", code: trimmed });
  };

  const handleSelfHostedSubmit = () => {
    const port = parseInt(bindPort, 10);
    if (!accessToken.trim() || !channelSecret.trim() || !Number.isFinite(port)) {
      setError("Access token, channel secret, and port are required.");
      return;
    }
    setError(null);
    setBusy(true);
    send({
      type: "line_self_hosted_setup",
      access_token: accessToken.trim(),
      channel_secret: channelSecret.trim(),
      host: bindHost.trim() || "0.0.0.0",
      port,
      public_url: publicUrl.trim() || null,
      allowed_users: allowedUsers.trim(),
      allowed_groups: allowedGroups.trim(),
      allowed_rooms: allowedRooms.trim(),
      slow_response_threshold_secs: parseInt(threshold, 10) || 45,
      auto_approve_all: autoApproveAll,
    });
  };

  const handleDisconnect = () => {
    setBusy(true);
    send({ type: "line_disconnect" });
  };

  const webhookHint = publicUrl.trim()
    ? `${publicUrl.replace(/\/+$/, "")}/line/webhook`
    : `http://${bindHost || "0.0.0.0"}:${bindPort || "8646"}/line/webhook`;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ background: "rgba(0,0,0,0.5)" }}
      onClick={onClose}
    >
      <div
        className="rounded-lg shadow-2xl"
        style={{
          background: "var(--bg-primary)",
          border: "1px solid var(--border)",
          width: "520px",
          maxWidth: "95vw",
          maxHeight: "90vh",
          overflowY: "auto",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div
          className="flex items-center justify-between px-4 py-3 border-b"
          style={{ borderColor: "var(--border)" }}
        >
          <div className="flex items-center gap-2">
            <MessageCircle size={16} style={{ color: "var(--accent)" }} />
            <span
              className="font-semibold text-sm"
              style={{ color: "var(--text-primary)" }}
            >
              Line Connect
            </span>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded hover:bg-white/10"
            style={{ color: "var(--text-secondary)" }}
            title="Close (Esc)"
          >
            <X size={14} />
          </button>
        </div>

        <div className="px-4 py-4 space-y-4">
          {status.state === "connected" ? (
            <ConnectedView
              status={status}
              busy={busy}
              onDisconnect={handleDisconnect}
            />
          ) : (
            <>
              <ModeToggle mode={mode} setMode={setMode} />
              {mode === "hosted" ? (
                <HostedForm
                  code={code}
                  setCode={setCode}
                  busy={busy}
                  error={error}
                  onConnect={handleHostedConnect}
                />
              ) : (
                <SelfHostedForm
                  accessToken={accessToken}
                  setAccessToken={setAccessToken}
                  channelSecret={channelSecret}
                  setChannelSecret={setChannelSecret}
                  bindHost={bindHost}
                  setBindHost={setBindHost}
                  bindPort={bindPort}
                  setBindPort={setBindPort}
                  publicUrl={publicUrl}
                  setPublicUrl={setPublicUrl}
                  allowedUsers={allowedUsers}
                  setAllowedUsers={setAllowedUsers}
                  allowedGroups={allowedGroups}
                  setAllowedGroups={setAllowedGroups}
                  allowedRooms={allowedRooms}
                  setAllowedRooms={setAllowedRooms}
                  threshold={threshold}
                  setThreshold={setThreshold}
                  autoApproveAll={autoApproveAll}
                  setAutoApproveAll={setAutoApproveAll}
                  webhookHint={webhookHint}
                  busy={busy}
                  error={error}
                  onSubmit={handleSelfHostedSubmit}
                />
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function ModeToggle({ mode, setMode }: { mode: Mode; setMode: (m: Mode) => void }) {
  return (
    <div
      className="flex gap-2 p-1 rounded"
      style={{ background: "var(--bg-secondary)", border: "1px solid var(--border)" }}
    >
      <button
        onClick={() => setMode("hosted")}
        className="flex-1 px-3 py-1.5 rounded text-xs font-semibold"
        style={{
          background: mode === "hosted" ? "var(--accent)" : "transparent",
          color: mode === "hosted" ? "var(--accent-fg, #ffffff)" : "var(--text-secondary)",
        }}
      >
        Hosted (thClaws relay)
      </button>
      <button
        onClick={() => setMode("self_hosted")}
        className="flex-1 px-3 py-1.5 rounded text-xs font-semibold"
        style={{
          background: mode === "self_hosted" ? "var(--accent)" : "transparent",
          color: mode === "self_hosted" ? "var(--accent-fg, #ffffff)" : "var(--text-secondary)",
        }}
      >
        Self-hosted (your LINE OA)
      </button>
    </div>
  );
}

function HostedForm({
  code,
  setCode,
  busy,
  error,
  onConnect,
}: {
  code: string;
  setCode: (s: string) => void;
  busy: boolean;
  error: string | null;
  onConnect: () => void;
}) {
  return (
    <>
      <p className="text-xs" style={{ color: "var(--text-secondary)" }}>
        Send any message to your thClaws LINE OA, then paste the 8-character
        pairing code below.
      </p>
      <div className="space-y-2">
        <label className="block text-xs font-semibold" style={{ color: "var(--text-primary)" }}>
          Pairing code
        </label>
        <input
          type="text"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") onConnect();
          }}
          placeholder="ABCD1234"
          maxLength={8}
          className="w-full px-3 py-2 rounded font-mono text-sm tracking-wider uppercase"
          style={{
            background: "var(--bg-secondary)",
            border: "1px solid var(--border)",
            color: "var(--text-primary)",
          }}
          autoFocus
        />
      </div>
      <ErrorBanner error={error} />
      <div className="flex justify-end">
        <button
          onClick={onConnect}
          disabled={busy || code.trim().length === 0}
          className="px-3 py-1.5 rounded text-xs font-semibold"
          style={{
            background: busy || code.trim().length === 0 ? "var(--bg-secondary)" : "var(--accent)",
            color: "var(--accent-fg, #ffffff)",
            opacity: busy || code.trim().length === 0 ? 0.5 : 1,
          }}
        >
          {busy ? "Connecting…" : "Connect"}
        </button>
      </div>
    </>
  );
}

function SelfHostedForm({
  accessToken,
  setAccessToken,
  channelSecret,
  setChannelSecret,
  bindHost,
  setBindHost,
  bindPort,
  setBindPort,
  publicUrl,
  setPublicUrl,
  allowedUsers,
  setAllowedUsers,
  allowedGroups,
  setAllowedGroups,
  allowedRooms,
  setAllowedRooms,
  threshold,
  setThreshold,
  autoApproveAll,
  setAutoApproveAll,
  webhookHint,
  busy,
  error,
  onSubmit,
}: {
  accessToken: string;
  setAccessToken: (s: string) => void;
  channelSecret: string;
  setChannelSecret: (s: string) => void;
  bindHost: string;
  setBindHost: (s: string) => void;
  bindPort: string;
  setBindPort: (s: string) => void;
  publicUrl: string;
  setPublicUrl: (s: string) => void;
  allowedUsers: string;
  setAllowedUsers: (s: string) => void;
  allowedGroups: string;
  setAllowedGroups: (s: string) => void;
  allowedRooms: string;
  setAllowedRooms: (s: string) => void;
  threshold: string;
  setThreshold: (s: string) => void;
  autoApproveAll: boolean;
  setAutoApproveAll: (v: boolean) => void;
  webhookHint: string;
  busy: boolean;
  error: string | null;
  onSubmit: () => void;
}) {
  return (
    <>
      <div
        className="flex items-start gap-2 text-xs px-3 py-2 rounded"
        style={{ background: "var(--bg-secondary)", border: "1px solid var(--border)" }}
      >
        <Info size={14} className="shrink-0 mt-0.5" style={{ color: "var(--accent)" }} />
        <span style={{ color: "var(--text-secondary)" }}>
          <strong style={{ color: "var(--text-primary)" }}>Reply API only — no Push.</strong>{" "}
          thClaws binds the port locally; you front it with your own subdomain + reverse
          proxy. Paste{" "}
          <code style={{ color: "var(--text-primary)" }}>{webhookHint}</code>{" "}
          into the LINE Developers Console webhook field once your tunnel is up.
        </span>
      </div>

      <Field label="Channel access token" required>
        <input
          type="password"
          value={accessToken}
          onChange={(e) => setAccessToken(e.target.value)}
          placeholder="long-lived token"
          className="w-full px-3 py-2 rounded font-mono text-xs"
          style={inputStyle}
          autoComplete="off"
        />
      </Field>
      <Field label="Channel secret" required>
        <input
          type="password"
          value={channelSecret}
          onChange={(e) => setChannelSecret(e.target.value)}
          placeholder="HMAC verification secret"
          className="w-full px-3 py-2 rounded font-mono text-xs"
          style={inputStyle}
          autoComplete="off"
        />
      </Field>
      <div className="grid grid-cols-2 gap-2">
        <Field label="Listen host">
          <input
            type="text"
            value={bindHost}
            onChange={(e) => setBindHost(e.target.value)}
            placeholder="0.0.0.0"
            className="w-full px-3 py-2 rounded font-mono text-xs"
            style={inputStyle}
          />
        </Field>
        <Field label="Listen port">
          <input
            type="number"
            value={bindPort}
            onChange={(e) => setBindPort(e.target.value)}
            placeholder="8646"
            className="w-full px-3 py-2 rounded font-mono text-xs"
            style={inputStyle}
          />
        </Field>
      </div>
      <Field label="Public URL (optional — display hint only)">
        <input
          type="text"
          value={publicUrl}
          onChange={(e) => setPublicUrl(e.target.value)}
          placeholder="https://line.your-domain.com"
          className="w-full px-3 py-2 rounded font-mono text-xs"
          style={inputStyle}
        />
      </Field>
      <Field label="Allowed user IDs (comma-separated)">
        <input
          type="text"
          value={allowedUsers}
          onChange={(e) => setAllowedUsers(e.target.value)}
          placeholder="U1234,U5678"
          className="w-full px-3 py-2 rounded font-mono text-xs"
          style={inputStyle}
        />
      </Field>
      <div className="grid grid-cols-2 gap-2">
        <Field label="Allowed groups">
          <input
            type="text"
            value={allowedGroups}
            onChange={(e) => setAllowedGroups(e.target.value)}
            placeholder="G1234"
            className="w-full px-3 py-2 rounded font-mono text-xs"
            style={inputStyle}
          />
        </Field>
        <Field label="Allowed rooms">
          <input
            type="text"
            value={allowedRooms}
            onChange={(e) => setAllowedRooms(e.target.value)}
            placeholder="R1234"
            className="w-full px-3 py-2 rounded font-mono text-xs"
            style={inputStyle}
          />
        </Field>
      </div>
      <Field label="Slow-response threshold (sec)">
        <input
          type="number"
          value={threshold}
          onChange={(e) => setThreshold(e.target.value)}
          placeholder="45"
          className="w-full px-3 py-2 rounded font-mono text-xs"
          style={inputStyle}
        />
      </Field>

      <label className="flex items-start gap-2 cursor-pointer">
        <input
          type="checkbox"
          checked={autoApproveAll}
          onChange={(e) => setAutoApproveAll(e.target.checked)}
          className="mt-0.5"
        />
        <span className="text-xs">
          <strong style={{ color: "var(--warning, #d19a66)" }}>
            Auto-approve all tools
          </strong>{" "}
          — skip in-chat Approve/Deny. Every tool call (Bash, Edit,
          Write, etc.) runs without confirmation while this bridge
          is connected.
        </span>
      </label>

      <ErrorBanner error={error} />

      <div className="flex justify-end">
        <button
          onClick={onSubmit}
          disabled={busy}
          className="px-3 py-1.5 rounded text-xs font-semibold"
          style={{
            background: busy ? "var(--bg-secondary)" : "var(--accent)",
            color: "var(--accent-fg, #ffffff)",
            opacity: busy ? 0.5 : 1,
          }}
        >
          {busy ? "Setting up…" : "Save & Connect"}
        </button>
      </div>
    </>
  );
}

function Field({
  label,
  required,
  children,
}: {
  label: string;
  required?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-1">
      <label className="block text-xs font-semibold" style={{ color: "var(--text-primary)" }}>
        {label}
        {required && <span style={{ color: "var(--danger, #e06c75)" }}> *</span>}
      </label>
      {children}
    </div>
  );
}

const inputStyle = {
  background: "var(--bg-secondary)",
  border: "1px solid var(--border)",
  color: "var(--text-primary)",
};

function ErrorBanner({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <div
      className="flex items-start gap-2 text-xs px-3 py-2 rounded"
      style={{
        background: "var(--bg-secondary)",
        color: "var(--danger, #e06c75)",
        border: "1px solid var(--border)",
      }}
    >
      <AlertCircle size={14} className="shrink-0 mt-0.5" />
      <span>{error}</span>
    </div>
  );
}

function ConnectedView({
  status,
  busy,
  onDisconnect,
}: {
  status: Status;
  busy: boolean;
  onDisconnect: () => void;
}) {
  const modeLabel = status.mode === "self_hosted" ? "self-hosted" : "hosted";
  return (
    <>
      <div
        className="flex items-start gap-2 text-xs px-3 py-2 rounded"
        style={{ background: "var(--bg-secondary)", border: "1px solid var(--border)" }}
      >
        <CheckCircle2
          size={14}
          className="shrink-0 mt-0.5"
          style={{ color: "var(--success, #98c379)" }}
        />
        <div className="space-y-1">
          <div style={{ color: "var(--text-primary)" }}>
            <strong>Connected ({modeLabel}).</strong> Send a message to your LINE OA to verify.
          </div>
          {status.server_url && (
            <div
              className="font-mono"
              style={{ color: "var(--text-secondary)", fontSize: "10px" }}
            >
              {status.server_url}
            </div>
          )}
        </div>
      </div>
      <div className="flex justify-end">
        <button
          onClick={onDisconnect}
          disabled={busy}
          className="px-3 py-1.5 rounded text-xs font-semibold"
          style={{
            background: "var(--bg-secondary)",
            border: "1px solid var(--border)",
            color: "var(--danger, #e06c75)",
            opacity: busy ? 0.5 : 1,
          }}
        >
          {busy ? "Disconnecting…" : "Disconnect"}
        </button>
      </div>
    </>
  );
}
