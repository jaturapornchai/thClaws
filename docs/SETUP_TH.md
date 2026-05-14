# คู่มือเชื่อม thClaws กับ Telegram + LINE OA + LINE LIFF

คู่มือภาษาไทย step-by-step สำหรับ branch
[`feature/telegram-integration`](https://github.com/jaturapornchai/thClaws/tree/feature/telegram-integration).

เอกสารอ้างอิงภาษาอังกฤษ (มีรายละเอียดสถาปัตยกรรม + troubleshooting):
- [docs/telegram-bridge.md](telegram-bridge.md)
- [docs/line-self-hosted.md](line-self-hosted.md)
- [docs/line-liff.md](line-liff.md)

---

## ภาพรวม

| ระบบ | ใช้เมื่อไหร่ | ลำดับการตั้ง |
|---|---|---|
| **Telegram bot** | bot บน Telegram, DM/group | 1 |
| **LINE OA (webhook)** | LINE official account, DM/group | 2 |
| **LINE LIFF** | in-LINE WebView chat (streaming) | 3 — ทำหลัง LINE OA |

ทุกระบบใช้ **public HTTPS endpoint เดียวกัน** — ผ่าน Tailscale Funnel.

---

## ขั้น 0 — Tailscale Funnel (เปิดครั้งเดียว ใช้กับทุกระบบ)

ติดตั้ง + login (รายละเอียดเต็มใน
[ขั้นเซ็ตอัพ](#cheat-sheet-tailscale)):

```powershell
winget install tailscale.tailscale
tailscale up --unattended
```

ขั้น Tailscale admin: เปิด ACL ให้มี `funnel` attribute + เปิด
**MagicDNS** + **HTTPS Certificates** ใน DNS tab. แล้ว enable Serve
feature ที่ <https://login.tailscale.com/f/serve?node=YOUR-NODE>.

รัน mappings ครั้งเดียว:

```powershell
$ts = 'C:\Program Files\Tailscale\tailscale.exe'
& $ts serve --bg --https=443 --set-path=/line/webhook     http://localhost:8646/line/webhook
& $ts serve --bg --https=443 --set-path=/telegram/webhook http://localhost:8647/telegram/webhook
& $ts serve --bg --https=443 --set-path=/liff             http://localhost:8646/liff
& $ts serve --bg --https=443 --set-path=/liff/ws          http://localhost:8646/liff/ws
& $ts funnel --bg 443
```

ดู URL ของลุงจืด:
```powershell
& $ts funnel status
```
จะเห็น `https://<hostname>.<tailnet>.ts.net` — เก็บไว้ใช้ทั้ง 3 ระบบ.

---

## ระบบ 1 — Telegram bot

### 1.1 สร้าง bot
1. เปิด Telegram → DM `@BotFather` → `/newbot` → ตั้งชื่อ + username
2. Copy **bot token** (เลข:ตัวอักษร เช่น `8716356521:AAH7...`)

### 1.2 หา user ID ของตัวเอง
DM `@userinfobot` หรือ `@getidsbot` → จะตอบเลข user id (~10 หลัก).

### 1.3 ตั้งใน thClaws
1. เปิด thClaws GUI → ⚙ → **Telegram Connect…**
2. กรอก **Bot token** + กด ▸ **Advanced**:
   - Mode: **Long-polling** (default — ไม่ต้องตั้ง public URL ก็ได้)
   - Allowed user IDs: เลข user id จากข้อ 1.2
   - หรือถ้าทดสอบเร็วๆ ติ๊ก ☑ **Allow open mode** (อนุญาตทุกคนที่
     เจอ bot — ใช้กับ private bot เท่านั้น)
   - ติ๊ก ☑ **Auto-approve all tools** ถ้าไม่ต้องการ Approve/Deny
3. **Save & Connect** → modal ปิด, sidebar ขึ้น pill `Telegram ·
   @yourbot`

### 1.4 ทดสอบ
DM bot → ส่ง "hi" → agent ตอบกลับ.

ถ้าใช้ Webhook mode (production): ใส่ Public URL =
`https://<host>.ts.net` ใน Advanced. thClaws เรียก `setWebhook` ให้
อัตโนมัติ.

---

## ระบบ 2 — LINE OA (self-hosted webhook)

### 2.1 สร้าง Messaging API channel
1. [LINE Developers Console](https://developers.line.biz/console/) →
   provider → **Create a Messaging API channel**
2. กรอกข้อมูลพื้นฐาน → สร้าง
3. tab **Messaging API**:
   - Copy **Channel access token (long-lived)** — กด Issue ถ้ายังไม่มี
4. tab **Basic settings**:
   - Copy **Channel secret**

### 2.2 ตั้งใน thClaws
1. ⚙ → **Line Connect…** → tab **Self-hosted (your LINE OA)**
2. กรอก:
   - Channel access token: paste จากข้อ 2.1
   - Channel secret: paste
   - Listen host/port: เปลี่ยนได้, default `0.0.0.0:8646`
   - **Public URL**: `https://<host>.ts.net`
   - **Allowed user IDs**: LINE user id ของลุงจืด (`U...` 33 ตัว)
   - Threshold: 45 (default)
   - ☑ **Auto-approve all tools** (optional)
   - **LIFF ID**: ปล่อยว่างก่อน (กลับมาใส่ในระบบ 3)
3. **Save & Connect**

### 2.3 หา LINE user ID ของตัวเอง
ไป LINE Developers Console → channel → **Basic settings** → scroll
หา **"Your user ID"** (`U...` 33 ตัว). กลับมาใส่ในข้อ 2.2 →
Reconnect.

### 2.4 ตั้ง webhook ใน LINE console
1. tab **Messaging API** → **Webhook URL**:
   `https://<host>.ts.net/line/webhook`
2. กด **Verify** → ต้อง ✓ (200 OK)
3. Toggle Webhook = **On**
4. Disable **Auto-reply messages** (ไม่งั้น LINE จะส่ง sticker
   auto ครอบ reply ของ thClaws)

### 2.5 ทดสอบ
LINE app → ค้น bot ตาม channel name → Add friend → DM "hi" →
agent ตอบกลับ. ถ้า tool ต้องการ approval (และไม่ได้ติ๊ก auto-
approve) → Quick Reply chips `✅ Approve` / `🚫 Deny` ส่งมาให้
กดใน chat.

---

## ระบบ 3 — LINE LIFF (in-LINE WebView chat)

ทำหลังจากระบบ 2 ใช้งานได้แล้ว.

### 3.1 สร้าง LIFF app
1. ที่ channel เดิม → tab **LIFF** → **Add**
2. กรอก:
   - LIFF app name: เช่น `thClaws chat`
   - Size: **Full**
   - **Endpoint URL**: `https://<host>.ts.net/liff` ← สำคัญ! ใช้
     URL จาก Tailscale, ไม่ใช่โดเมนอื่น
   - Scopes: ☑ `profile` ☑ `openid` (ต้องมีทั้งสอง)
   - Bot link feature: **On (Aggressive)**
3. **Add** → copy **LIFF ID** (เลข-ตัวอักษร เช่น `2002575936-...`)

### 3.2 ใส่ใน thClaws
1. ⚙ → **Line Connect…** → Self-hosted
2. ฟิลด์ **LIFF ID** → paste
3. **Save & Connect**

### 3.3 (เลือกได้) สร้าง Rich Menu
ไป LINE OA Manager → Rich Menu → สร้าง:
- Action ของปุ่ม: **Link** → `https://liff.line.me/<LIFF-ID>`
  (ไม่ใช่ public URL — ต้องผ่าน `liff.line.me`)

### 3.4 ทดสอบ
1. เปิด LINE → bot → กด rich menu → WebView เปิด → ขออนุญาต
   profile ครั้งแรก
2. ห้อง chat แบบ thClaws ปรากฏ (header ดอท + input)
3. พิมพ์ "hello" → agent ตอบ streaming — text ทยอยขึ้นจนจบ

### 3.5 desktop smoke test
เปิดใน Chrome: `https://<host>.ts.net/liff` → เห็นหน้า chat = backend
ทำงาน. แต่จะติด "denied" ที่ auth handshake — ปกติ เพราะ desktop
browser ไม่มี LIFF SDK id_token.

---

## Cheat sheet Tailscale

URL ถาวร: `https://<hostname>.<tailnet>.ts.net`

| คำสั่ง | ใช้เมื่อ |
|---|---|
| `tailscale funnel status` | ดู mapping ปัจจุบัน |
| `tailscale serve status` | ดู serve config |
| `tailscale serve reset` | ล้าง serve mappings (ระวัง — ลบทั้งหมด) |
| `tailscale funnel --bg 443` | เปิด funnel public (หลัง serve config) |
| `tailscale funnel --bg off 443` | ปิด funnel แต่ serve คง |

ถ้า funnel หลุดหลัง serve เปลี่ยน:
```powershell
& 'C:\Program Files\Tailscale\tailscale.exe' funnel --bg 443
```

---

## Troubleshooting รวม

| อาการ | ต้นเหตุ | แก้ |
|---|---|---|
| `bad signature` 401 ตอน LINE Verify | channel secret ผิด | ใส่ใหม่ใน Self-hosted modal → Save & Connect |
| `bad secret_token` 401 (Telegram webhook) | Webhook secret_token ใน thClaws ไม่ตรงกับที่ `setWebhook` | re-Save & Connect Telegram |
| `drop: NoValidReplyToken` ใน log | agent ตอบช้า > 50s → reply token หมดอายุ | (1) ติ๊ก Auto-approve → ลด agent latency, (2) slow-response button จะส่งเอง — กด "รับคำตอบ" ใน LINE |
| `approval denied: no fresh reply token` | ไม่มี user DM bot ก่อน → no token in store | DM bot ก่อน 1 ครั้ง |
| Telegram bot ไม่ตอบ | ลุงจืดส่งให้ bot คนละตัว — เช็คชื่อ bot ใน sidebar pill ของ thClaws |
| LIFF เปิดแล้วเห็น "Account Suspended" | Endpoint URL ใน LIFF console ชี้ไปโดเมนอื่น (ที่ suspended) | LINE console → LIFF → Edit endpoint = `https://<host>.ts.net/liff` |
| LIFF เปิดแล้ว 404 | Tailscale Funnel ยังไม่ map `/liff` | รันคำสั่ง serve mapping จาก [ขั้น 0](#ขั้น-0--tailscale-funnel-เปิดครั้งเดียว-ใช้กับทุกระบบ) |
| `auto_approve_all=false` → ทุก tool ถูก deny ใน self-hosted | ไม่มี approval prompt path (ก่อน) → ผมเพิ่ม Quick Reply approval แล้ว, ลอง pull latest |

---

## Security defaults (สรุป)

ทุกระบบรัน **fail-closed** by default:

| Behaviour | Default | Opt-out flag |
|---|---|---|
| Empty allowlist | Deny all | `allow_open_mode` |
| Group auth (Telegram) | Need BOTH chat_id AND user_id | `allow_any_user_in_group` |
| Mention gate (group, getMe fails) | Drop | `require_mention_in_groups=false` |
| Tool approval | LineGated (prompt user) | `auto_approve_all=true` |
| Webhook signature mismatch | 401 | (n/a) |
| Duplicate update_id | Drop | (n/a) |
| LIFF id_token verify fails | Deny | (n/a) |

**Bot tokens / channel secrets อยู่ใน OS keychain เท่านั้น** — ไฟล์
`~/.config/thclaws/*.json` ไม่เก็บ secrets. URLs ที่มี `bot<TOKEN>`
ถูก redact ก่อน log/IPC.

---

## ผังภาพรวม (text)

```
              ┌─ Telegram bot (long-poll/webhook) ───┐
              │                                       │
LINE user ────┼─ LINE OA (POST /line/webhook) ───────┤
              │                                       ├──► thClaws agent
              │                                       │       (worker + tools)
              └─ LIFF WebView (WS /liff/ws) ─────────┘
                              ▲
                              │  events_tx broadcast
                              │  (GUI chat + LIFF stream share)
                              ▼
                       Tailscale Funnel
                       (single HTTPS endpoint)
```
