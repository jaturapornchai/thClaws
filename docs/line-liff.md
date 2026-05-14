# LINE LIFF — in-LINE chat WebView for thClaws

Adds an **in-LINE chat surface** on top of the existing LINE OA bridge.
Tapping a LIFF link in LINE opens a WebView pointed at thClaws; the
user types in the WebView and the agent streams text back live.

## Architecture

```
User taps LIFF link in LINE
        │
        ▼
LINE in-app browser → GET https://<public>/liff
        │
        ▼
liff.html (embedded) loads:
  • LINE LIFF SDK (CDN) → liff.init(liffId) → getProfile + getIDToken
  • WebSocket to /liff/ws

WS handshake:
  client → { type: "auth", id_token, user_id, display_name }
  server → verify id_token via https://api.line.me/oauth2/v2.1/verify
           → check user_id ∈ Allowed user IDs (same allowlist as webhook)
           → { type: "auth_ok" } OR { type: "denied", reason }

Per turn:
  client → { type: "prompt", text }
  server → ShellInput::Line(text) → events_tx broadcast → forward:
            { type: "delta", text }      ← AssistantTextDelta (streaming)
            { type: "tool",  tool_name } ← ToolCallStart
            { type: "error", text }      ← ErrorText
            { type: "done" }             ← TurnDone
```

All routes live inside the same axum server that already serves
`/line/webhook` — no extra port, same public URL, same Tailscale
Funnel mapping.

## Prerequisites

1. LINE self-hosted bridge configured (see
   [line-self-hosted.md](line-self-hosted.md)).
2. Public HTTPS endpoint pointing at thClaws (Tailscale / Cloudflare
   Tunnel / your reverse proxy).
3. Webhook already verified in LINE Developers Console.

## Set up the LIFF app

1. Open [LINE Developers Console](https://developers.line.biz/console/)
   → your Messaging API channel → **LIFF** tab → **Add**.
2. Fill in:
   - **LIFF app name**: e.g. `thClaws chat`
   - **Size**: `Full` (recommended)
   - **Endpoint URL**: `https://<your-public-host>/liff`
     (e.g. `https://jeadi9.tail42fa16.ts.net/liff`)
   - **Scope**: tick `profile` + `openid`
   - **Bot link feature**: `On (Aggressive)`
3. Click **Add** → copy the **LIFF ID** (e.g. `1234567890-abcXYZ`).

## Wire into thClaws

1. GUI → ⚙ → **Line Connect…** → **Self-hosted** tab.
2. Paste the LIFF ID into the new **LIFF ID** field.
3. Make sure your numeric LINE user IDs are in **Allowed user IDs**
   (same allowlist gates LIFF + webhook).
4. **Save & Connect**. Modal closes; LIFF endpoint becomes active.

## Try it

Open the LIFF link in LINE:

```
https://liff.line.me/<your-liff-id>
```

The WebView opens, asks for profile permission once, then shows
the chat. Type, press Enter → reply streams in real time.

Desktop-browser smoke check on the underlying URL works too:

```
https://<your-public-host>/liff
```

In a desktop browser the LIFF SDK init fails (not inside LINE's
WebView), the page reverts to **anonymous mode**, and the auth
handshake fails the allowlist gate — by design. Only the LIFF SDK
path can mint a valid `id_token`.

## Allowlist semantics

| Surface | Gate |
|---------|------|
| LINE webhook / bot DM | `Allowed user IDs` + `Allowed groups` + `Allowed rooms` |
| LIFF WebView WS | `Allowed user IDs` (verified `sub` from id_token) |

Same CSV across both surfaces.

## Streaming model

WS subscribes to the worker's `events_tx` **after** auth. Every
`AssistantTextDelta` forwards as a `delta` frame within
milliseconds → text appears on the phone as the model produces
it, not after the full reply finishes.

Same broadcast feeds the GUI chat panel: a single agent turn
appears in **both** surfaces simultaneously if both are open.

## Limitations

- **One worker, shared turn**. LIFF prompts join the process-wide
  session. For per-user isolation run additional thClaws workers
  (out of scope for v1).
- **Text only** — no image / file uploads from LIFF chat yet.
- **WebSocket drops on background**: LINE's in-app browser
  suspends the WebView when the user backgrounds the app. The
  client auto-reconnects when it returns to foreground.
- **Tool result lines** show as `› tool_name` — actual output
  arrives via subsequent `delta` frames the model emits.

## Troubleshooting

| Symptom | Likely cause |
|---------|--------------|
| Page shows `anonymous (no LIFF)` | LIFF ID not configured OR running in desktop browser |
| WS connects then closes with `denied` | User id not in `Allowed user IDs` |
| `verify rejected` in server log | id_token expired (refresh) or wrong LIFF channel |
| 404 on `/liff` | `liff_id` was empty at bridge connect — LIFF routes not registered |
| WS times out at handshake | Tunnel HTTPS cert not ready yet or Funnel path wrong |
