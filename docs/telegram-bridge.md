# Telegram bot bridge

Bridge a Telegram bot (BotFather token) into the live thClaws session — DMs
and allowed group messages drive real agent turns; replies come back via
the Bot API. Inline-keyboard approvals route through the same permission
gate as LINE / GUI prompts.

Design references: [openclaw/openclaw `extensions/telegram/`][openclaw] and
[NousResearch/hermes-agent `gateway/platforms/telegram.py`][hermes]. The
thClaws implementation mirrors their semantics (allowlist on numeric IDs,
`tg:` / `telegram:` prefix tolerance, `requireMention` for groups) while
keeping the surface area small.

[openclaw]: https://github.com/openclaw/openclaw/tree/main/extensions/telegram
[hermes]: https://github.com/NousResearch/hermes-agent/blob/main/gateway/platforms/telegram.py

## What this is

- A second messaging surface for the agent, alongside LINE. One process
  can run both bridges concurrently.
- Long-polling default — zero infrastructure required, thClaws hits
  `https://api.telegram.org` from the local machine on a configurable
  `getUpdates` cadence (default 30 s).
- Webhook optional — for production deploys with a public HTTPS endpoint
  in front, thClaws binds `:8647/telegram/webhook` and verifies the
  `X-Telegram-Bot-Api-Secret-Token` header.
- Approval prompts arrive as inline-keyboard messages
  (`✅ Approve` / `🚫 Deny`) with `callback_data = tool:allow:<request_id>`
  / `tool:deny:<request_id>`. Tap the button → decision goes back to the
  agent.

## What this is NOT

- No `sendMessageDraft` streaming. The agent's final assistant text is
  sent as one (or more — chunked at 4000 chars per bubble) plain message.
  An `editMessageText` streaming mode is a future option but is not v1.
- No private-chat topic / forum-topic mode. Each chat is a single session
  lane — pairing one bot to one agent session.
- No mass discovery / pairing flow. The bot's allowlist must contain the
  numeric Telegram user ID of every authorised sender before any DM or
  group message is forwarded.

## Architecture

```
Telegram Platform ──update──▶ [transport]   ──route──▶ allowlist gate ──▶ TelegramSink
                              long_poll OR                                   │
                              webhook (:8647)                                ▼
                                                                       ShellInput
                                                                       ::TelegramMessage
                                                                            │
                                                                            ▼
                                                                    shared_session
                                                                    worker turn
                                                                            │
                                                       sendMessage(chat_id) ◀ oneshot
```

Module map (Rust):

| File                                       | Role                                                       |
| ------------------------------------------ | ---------------------------------------------------------- |
| [`telegram/mode.rs`](../crates/core/src/telegram/mode.rs)             | `LongPoll` / `Webhook` enum (snake_case serde)             |
| [`telegram/config.rs`](../crates/core/src/telegram/config.rs)         | `TelegramConfig` (CSV allowlists, mode, host/port)         |
| [`telegram/types.rs`](../crates/core/src/telegram/types.rs)           | `Update`, `Message`, `Chat`, `CallbackQuery` (tolerant)    |
| [`telegram/client.rs`](../crates/core/src/telegram/client.rs)         | `TelegramClient` — sendMessage / getUpdates / etc.         |
| [`telegram/allowlist.rs`](../crates/core/src/telegram/allowlist.rs)   | User + chat allowlist, tolerates `tg:` / `telegram:`       |
| [`telegram/chunk.rs`](../crates/core/src/telegram/chunk.rs)           | 4000-char chunked split, MAX 20 bubbles, UTF-8 safe        |
| [`telegram/long_poll.rs`](../crates/core/src/telegram/long_poll.rs)   | `getUpdates` loop with offset + cancel + backoff           |
| [`telegram/webhook.rs`](../crates/core/src/telegram/webhook.rs)       | axum POST + `X-Telegram-Bot-Api-Secret-Token` verify       |
| [`telegram/dispatch.rs`](../crates/core/src/telegram/dispatch.rs)     | Shared allowlist gate (both transports)                    |
| [`telegram/sink.rs`](../crates/core/src/telegram/sink.rs)             | `TelegramSink` → `ShellInput::TelegramMessage` + reply     |
| [`telegram/approver.rs`](../crates/core/src/telegram/approver.rs)     | `TelegramApprover` (inline-keyboard approvals)             |
| [`telegram/spawn.rs`](../crates/core/src/telegram/spawn.rs)           | Orchestrate transport + secrets + `getMe`                  |

## Prerequisites

- A bot token from [@BotFather](https://t.me/BotFather) (`/newbot`).
- Numeric Telegram user ID for every authorised sender. Easiest way:
  - DM the bot once after `telegram_setup`, then check `thclaws logs`
    for the `from.id` of the dropped message.
- For webhook mode: a public HTTPS endpoint reverse-proxied to the bind
  address (`0.0.0.0:8647` by default).

## Setup

### GUI

`Settings ⚙` → **Telegram Connect…**. Paste the bot token, leave mode on
**Long-polling** unless you have a public HTTPS proxy. Add numeric user
IDs to **Allowed user IDs**. Add group chat IDs (typically negative, e.g.
`-1001234567890`) to **Allowed group chat IDs** if you want the bot to
respond inside a group. Submit.

The sidebar shows a **Telegram** pill once connected, with the bot
`@username` and transport mode.

### Env / headless

```bash
export TELEGRAM_BOT_TOKEN=123456:ABC-DEF...
export TELEGRAM_ALLOWED_USERS=123456789,987654321
export TELEGRAM_ALLOWED_CHATS=-1001234567890
export TELEGRAM_REQUIRE_MENTION_IN_GROUPS=1
# webhook mode only:
export TELEGRAM_WEBHOOK=1
export TELEGRAM_WEBHOOK_SECRET_TOKEN=any_32_char_random_string
export TELEGRAM_WEBHOOK_HOST=0.0.0.0
export TELEGRAM_WEBHOOK_PORT=8647
```

`TelegramConfig::from_env()` is consumed by the GUI/IPC layer; CLI users
can read it directly from the same env vars.

### Webhook registration

thClaws does **not** call `setWebhook` for you. Once the bridge is
listening, register your public URL with Telegram yourself:

```bash
curl -sS \
  -d url=https://tg.example.com/telegram/webhook \
  -d "secret_token=$TELEGRAM_WEBHOOK_SECRET_TOKEN" \
  -d "allowed_updates=[\"message\",\"channel_post\",\"callback_query\"]" \
  "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/setWebhook"
```

To switch back to long-polling later, hit `deleteWebhook` first or
thClaws's startup `deleteWebhook` (best-effort) will clear it.

## Group behaviour

Telegram bots default to **Privacy Mode** — the bot only sees `@mentions`
and direct replies. thClaws's `require_mention_in_groups` config matches
this: even if you disable privacy via `/setprivacy`, the bridge drops
non-mention group messages until you opt out.

To respond to every message in an allowed group:

1. BotFather → `/setprivacy` → **Disable** for your bot.
2. Remove + re-add the bot to the group (Telegram re-applies the flag).
3. In thClaws's Telegram Connect modal, uncheck
   **Require @mention or direct reply in groups**.

The allowlist remains the only authorisation gate at that point.

## Security checklist

- **Bot token is god-mode.** Anyone with the token can drive thClaws.
  Keep it in the OS keychain (auto via GUI) or `.env` mode 0600. Never
  commit it.
- **Allowlist must be non-empty in production.** Empty user + chat
  allowlist denies everything — the setup form rejects empty allowlists.
- **Webhook secret_token** is shared with `setWebhook`. Generate a fresh
  32+ char random string per deploy.
- **No Push API analogue ever opens unsolicited chats.** Approvals only
  land in chats that have messaged the bot first (the approver tracks
  `last_chat_id` updated by the sink).

## Limitations

- Laptop sleeps → bridge stops. Long-poll resumes on wake; webhook keeps
  receiving but thClaws can't answer until the worker thread spins back
  up.
- One bot ↔ one thClaws process. Multiple thClaws instances on the same
  token will race on `getUpdates` and one will see `409 Conflict`.
- Approval prompts always target the **most recent** allowed chat. If
  user A messages then user B does before the approval timer, the
  approval prompt arrives in B's chat. Acceptable for one-owner bots;
  multi-user deployments should put each user in their own group or
  accept that approvals are out-of-band.
- Long agent answers chunk into ≤20 bubbles × ≤4000 chars. Beyond that
  the tail is truncated with `\n…`.

## Troubleshooting

| Symptom                                              | Fix                                                                         |
| ---------------------------------------------------- | --------------------------------------------------------------------------- |
| `[telegram/long_poll] getUpdates failed: 409`        | Another process is also polling, or a webhook is registered. Delete webhook |
| `[telegram] approval denied: no_chat_known`          | The approver has no chat to send the prompt to — DM the bot once first      |
| Group messages dropped silently                      | `requireMention` is on and the message doesn't `@mention` the bot           |
| `bad secret_token` 401 in webhook logs               | Set `TELEGRAM_WEBHOOK_SECRET_TOKEN` matches the one passed to `setWebhook`  |
| Bot replies in a group never arrive                  | Bot privacy mode is on — `/setprivacy` Disable, then remove + re-add bot    |
