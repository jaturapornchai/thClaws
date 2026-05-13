# LINE OA — self-hosted mode

thClaws supports two ways to bridge LINE OA traffic into the agent:

| | **Hosted** | **Self-hosted** |
|---|---|---|
| Who owns the LINE OA | thClaws | You |
| Who runs the webhook | thClaws relay | You (this guide) |
| API used to talk to LINE | relay routes through LINE Messaging API | LINE **Reply API only — no Push API** |
| Background notifications | (relay-mediated) | Not supported — no inbound event = no reply token |
| Setup time | 1 minute (pairing code) | 30 – 60 minutes (LINE Dev Console + your own subdomain) |
| Data path | LINE Platform → thClaws relay → your laptop | LINE Platform → your subdomain → your laptop |

This page covers the self-hosted path.

## What this is NOT

**No Push API. Ever.** The self-hosted bridge calls only
`https://api.line.me/v2/bot/message/reply`. That endpoint requires a
fresh `replyToken` returned with every webhook event. If you have no
token (because the user hasn’t messaged the bot recently), the bridge
returns `NoValidReplyToken` and drops the response with a sanitized
log line — it will not fall back to push.

Practical consequences:

- **No cron / scheduled notifications via LINE.** A `/schedule` job
  cannot DM the user when it finishes.
- **No "agent finished overnight" notice.** If the user is not in
  the chat thread, they will not learn the answer until they send
  a new message.
- **Slow responses use a Template Button postback** — see below.

If you need push semantics, run the hosted bridge instead.

## Architecture

```
   LINE Platform
        │
        │ HTTPS POST /line/webhook (signed with channel secret)
        ▼
   Your public subdomain
        │
        │ HTTP (or HTTPS terminated locally)
        ▼
   Your reverse proxy / tunnel
        │
        │ HTTP
        ▼
   thClaws  (binds 0.0.0.0:8646 by default)
        │
        │ async dispatch to agent
        ▼
   Agent run-turn → assistant text
        │
        │ POST https://api.line.me/v2/bot/message/reply
        ▼
   LINE Platform → user’s LINE chat
```

thClaws does NOT terminate TLS for you. You are responsible for the
public HTTPS endpoint. Common ways to provide it:

- **Cloudflare Tunnel** (`cloudflared tunnel --url http://localhost:8646`):
  free, persistent URL, no port-forward, works behind CGNAT.
- **Caddy / nginx with Let’s Encrypt** on a VPS that reverse-proxies
  back to your thClaws host (requires you to own the VPS + DNS).
- **ngrok / localtunnel**: fine for dev; URL changes per restart in
  the free tier.
- **Direct port-forward + DDNS + Let’s Encrypt**: advanced, exposes
  your laptop.

## Prerequisites

1. A LINE Developers Console account with a **Messaging API channel**.
2. The channel’s **Channel access token** (long-lived) and
   **Channel secret**.
3. A public HTTPS URL that proxies to your thClaws host on TCP 8646
   (or whatever you bind).
4. thClaws running with the GUI build (`thclaws`, not `thclaws-cli`)
   so the connect modal is available. CLI-only self-hosted setup is
   future work.

## Setup steps

### 1. Configure the LINE channel

In the LINE Developers Console:

1. Open your Messaging API channel.
2. Disable **Auto-reply messages** (Settings → Messaging API), or
   they will collide with thClaws replies.
3. Disable **Greeting message** if you don’t want it on every
   first contact.
4. Note the **Channel access token (long-lived)** and **Channel
   secret**.
5. Leave the **Webhook URL** field blank for now — you’ll fill it in
   step 4 once the tunnel is up.
6. Set **Use webhook** to ON.

### 2. Start thClaws

Launch the GUI build. The sidebar still shows the LINE pill as
**Disconnected**. Open the **Line Connect** modal.

### 3. Pick the Self-hosted toggle

The modal has two radios at the top: **Hosted (thClaws relay)** and
**Self-hosted (your LINE OA)**. Pick Self-hosted.

Fill in:

| Field | Notes |
|---|---|
| Channel access token | Pasted from LINE Developers Console. Stored in the OS keychain — never written to `~/.config/thclaws/line.json`. |
| Channel secret | Same — keychain only. |
| Listen host | `0.0.0.0` to accept LAN/tunnel traffic; `127.0.0.1` if your tunnel/proxy is local. |
| Listen port | `8646` is the default. |
| Public URL | Optional display hint. The modal previews the URL you’ll paste into LINE. |
| Allowed user IDs | Comma-separated LINE user IDs. Empty list = nothing accepted (fail-closed). |
| Allowed groups / rooms | Same shape, for group / multi-person chats. |
| Slow-response threshold | Seconds before the postback button fires. Default 45. |

Click **Save & Connect**.

thClaws:

- writes secrets to keychain,
- saves the non-secret settings to `~/.config/thclaws/line.json`,
- binds the listener,
- swaps the agent into self-hosted mode.

The sidebar pill flips to **Connected (self-hosted)**.

### 4. Point LINE at the webhook

Start your tunnel:

```bash
cloudflared tunnel --url http://localhost:8646
```

cloudflared prints a public URL like
`https://random-words.trycloudflare.com`. Append `/line/webhook` and
paste that into LINE Developers Console → **Webhook URL**, then hit
**Verify**. LINE should respond `Success`.

If verification fails, check:

- The tunnel actually reaches port 8646.
- The thClaws listener is healthy at `GET /line/health` → `ok`.
- The channel secret in thClaws matches the Console.

### 5. Send a test message

From your LINE app, DM your channel. The agent runs a turn and
replies via Reply API. Typical end-to-end latency is 1 – 5 seconds
for a short prompt.

## Slow-response UX

A LINE `replyToken` is only valid for ~60 seconds and is single-use.
If the agent takes longer than the configured threshold (default
45 s), the bridge:

1. Uses the original `replyToken` to send a **Template Button**:
   `กำลังคิดอยู่ กดรับคำตอบเมื่อพร้อม` with one button **`รับคำตอบ`**.
2. Caches the eventual agent reply under a generated `request_id`.
3. Waits for the user to tap **`รับคำตอบ`**, which fires a postback
   carrying a **fresh** `replyToken`. The bridge uses that new token
   to send the cached reply.

States the postback can land in:

| State at tap | Reply sent |
|---|---|
| READY (agent done) | The cached assistant text. |
| PENDING (still running) | `ยังทำงานอยู่ กรุณารอสักครู่`. |
| DELIVERED (already replied) | `ส่งคำตอบแล้ว ✅`. |
| ERROR (agent failed) | `งานถูกยกเลิก: <reason>`. |
| Unknown id (e.g. old session) | Silently dropped. |

If the user never taps the button, the response is lost. There is
no push fallback. This is intentional.

## Approval prompts

When the agent runs in `PermissionMode::LineGated` and asks for
approval on a mutating tool, the self-hosted bridge denies the tool
call and logs `[line/direct] approval denied: self_hosted mode does
not route prompts`. The agent should run in `Auto` mode when paired
to self-hosted, or you should accept that mutating tools won’t run
without explicit user input.

Hosted mode keeps the original Quick Reply approval flow.

## Config reference

Settings persisted to `~/.config/thclaws/line.json` (non-secret):

```json
{
  "binding_token": "",
  "mode": "self_hosted",
  "direct": {
    "host": "0.0.0.0",
    "port": 8646,
    "public_url": "https://line.example.com",
    "slow_response_threshold_secs": 45,
    "allowed_users_csv": "U123,U456",
    "allowed_groups_csv": "",
    "allowed_rooms_csv": ""
  }
}
```

Secrets resolved from (highest priority first):

1. `LINE_CHANNEL_ACCESS_TOKEN`, `LINE_CHANNEL_SECRET` environment
   variables.
2. OS keychain accounts `line-self-hosted-access-token` and
   `line-self-hosted-channel-secret`.

Env-only overrides for the operational config:

| Variable | Default |
|---|---|
| `THCLAWS_LINE_DIRECT_HOST` | `0.0.0.0` |
| `THCLAWS_LINE_DIRECT_PORT` | `8646` |
| `THCLAWS_LINE_PUBLIC_URL` | unset |
| `THCLAWS_LINE_SLOW_RESPONSE_THRESHOLD` | `45` |
| `LINE_ALLOWED_USERS` / `LINE_ALLOWED_GROUPS` / `LINE_ALLOWED_ROOMS` | empty |

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| LINE Console "Verify" returns 401 | Channel secret mismatch. Re-paste the value from Console into the modal. |
| Verify returns 400 | Tunnel is reaching the wrong path or non-LINE traffic is hitting `/line/webhook`. Make sure the URL ends with `/line/webhook`. |
| Verify times out | Tunnel isn’t reaching port 8646. Test locally first: `curl http://localhost:8646/line/health` → `ok`. |
| User sends a message, agent runs, no reply arrives | Look for `[line/direct] reply failed` or `NoValidReplyToken` in the thClaws log. The most common cause is the reply token expiring while a slow tool ran without a slow-response button. Lower the threshold. |
| Approval prompt times out + denies | Expected — self-hosted does not route approvals. Switch to `Auto` mode when paired or use hosted mode. |
| Logs say `[line/direct] drop: NoValidReplyToken chat=<hash>` | Reply token already consumed (probably by a slow-response button) and the agent answer arrived after. Cached for postback — user can still tap the button. |

## Security checklist

- Use the OS keychain path (default). The on-disk config never holds
  secrets.
- Always populate at least one allowlist (`LINE_ALLOWED_USERS` etc).
  Empty allowlist = fail-closed but a misconfigured deploy might
  paste in `*` instead — there is no wildcard.
- Bind to `127.0.0.1` if your tunnel/proxy is local. Use `0.0.0.0`
  only when something downstream needs to reach the port from a
  different machine on the LAN.
- Webhook traffic must arrive over HTTPS. LINE Platform refuses to
  POST to `http://` URLs.
- Don’t log the `replyToken` or channel secret. The bridge’s log
  lines redact chat ids to their first 8 hex chars of SHA-256.

## Limits

- LINE Reply API: max 5 messages per call, ≤ 5000 chars per text
  bubble. thClaws chunks at 4500 chars and 5 bubbles, with a
  truncation marker (`…`) on the last bubble if the reply was longer.
- Reply tokens expire ~60 s after delivery. thClaws caps internal
  TTL at 50 s for network slack.
- LINE redelivery: deduplicated by `webhookEventId` (1024-entry LRU).
- Laptop sleeping = downtime. No queue. Run thClaws + tunnel under
  a supervisor (Windows Service, tmux, systemd-user, `pm2`).
