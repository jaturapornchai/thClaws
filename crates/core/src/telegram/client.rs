//! Telegram Bot API HTTP client. Wraps `sendMessage`, `getUpdates`,
//! `answerCallbackQuery`, `setWebhook`, `deleteWebhook`.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::chunk::split_for_telegram;
use super::errors::TelegramError;
use super::types::{ApiResponse, Update};

pub const API_BASE: &str = "https://api.telegram.org";

pub struct TelegramClient {
    http: reqwest::Client,
    bot_token: String,
}

#[derive(Serialize)]
struct SendMessageBody<'a> {
    chat_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_parameters: Option<ReplyParameters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<&'a Value>,
}

#[derive(Serialize)]
struct ReplyParameters {
    message_id: i64,
}

impl TelegramClient {
    pub fn new(bot_token: String) -> Result<Self, TelegramError> {
        if bot_token.is_empty() {
            return Err(TelegramError::MissingBotToken);
        }
        // Long-poll uses up to `timeout` seconds + network slack; keep
        // the per-request timeout well above the configured long-poll
        // timeout. The long_poll loop pins the actual budget per call.
        let http = reqwest::Client::builder()
            .user_agent(concat!("thclaws-core/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(70))
            .build()
            .map_err(|e| TelegramError::Http(e.to_string()))?;
        Ok(Self { http, bot_token })
    }

    pub fn endpoint(&self, method: &str) -> String {
        format!("{}/bot{}/{}", API_BASE, self.bot_token, method)
    }

    /// `sendMessage` — chunk the body to ≤ 4000 chars per bubble.
    /// Replies to `reply_to_message_id` if Some (keeps thread context
    /// in groups).
    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: Option<i64>,
    ) -> Result<(), TelegramError> {
        let bubbles = split_for_telegram(text);
        if bubbles.is_empty() {
            return Ok(());
        }
        for (i, bubble) in bubbles.iter().enumerate() {
            let reply = if i == 0 {
                reply_to_message_id.map(|message_id| ReplyParameters { message_id })
            } else {
                None
            };
            let body = SendMessageBody {
                chat_id,
                text: bubble,
                reply_parameters: reply,
                reply_markup: None,
            };
            self.post_json("sendMessage", &body).await?;
        }
        Ok(())
    }

    /// `sendMessage` with an inline keyboard attached. Used by the
    /// approver path. `keyboard` is the JSON-serialized
    /// `inline_keyboard` value (array of array of button objects).
    pub async fn send_message_with_keyboard(
        &self,
        chat_id: i64,
        text: &str,
        keyboard: &Value,
    ) -> Result<(), TelegramError> {
        let body = SendMessageBody {
            chat_id,
            text,
            reply_parameters: None,
            reply_markup: Some(keyboard),
        };
        self.post_json("sendMessage", &body).await
    }

    /// `getUpdates` — long-polling. Pass the next offset
    /// (max(seen) + 1). Returns the array of new updates.
    pub async fn get_updates(
        &self,
        offset: i64,
        timeout_secs: u64,
        allowed_updates: &[&str],
    ) -> Result<Vec<Update>, TelegramError> {
        let url = self.endpoint("getUpdates");
        let body = serde_json::json!({
            "offset": offset,
            "timeout": timeout_secs,
            "allowed_updates": allowed_updates,
        });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| TelegramError::Http(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| TelegramError::Http(e.to_string()))?;
        if !status.is_success() {
            return Err(TelegramError::Api {
                status: status.as_u16(),
                body: text,
            });
        }
        let parsed: ApiResponse<Vec<Update>> = serde_json::from_str(&text)
            .map_err(|e| TelegramError::Http(format!("parse getUpdates: {e}")))?;
        if !parsed.ok {
            return Err(TelegramError::NotOk {
                description: parsed.description.unwrap_or_default(),
            });
        }
        Ok(parsed.result.unwrap_or_default())
    }

    /// `answerCallbackQuery` — ack an inline-button tap so the
    /// spinner clears on the user's side.
    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> Result<(), TelegramError> {
        let body = serde_json::json!({
            "callback_query_id": callback_query_id,
            "text": text,
        });
        self.post_json("answerCallbackQuery", &body).await
    }

    /// `setWebhook` — register a public URL with Telegram. Used by
    /// the GUI's webhook setup flow.
    pub async fn set_webhook(
        &self,
        url: &str,
        secret_token: &str,
        allowed_updates: &[&str],
    ) -> Result<(), TelegramError> {
        let body = serde_json::json!({
            "url": url,
            "secret_token": secret_token,
            "allowed_updates": allowed_updates,
            "drop_pending_updates": false,
        });
        self.post_json("setWebhook", &body).await
    }

    /// `deleteWebhook` — clear any prior webhook before switching to
    /// long-polling.
    pub async fn delete_webhook(&self) -> Result<(), TelegramError> {
        let body = serde_json::json!({ "drop_pending_updates": false });
        self.post_json("deleteWebhook", &body).await
    }

    /// `getMe` — verify the bot token is valid and learn the bot's
    /// username / id. Returns the parsed `result` object so callers
    /// can surface bot details in a setup wizard. 401 = revoked token;
    /// 404 = malformed token (path doesn't resolve a bot).
    pub async fn get_me(&self) -> Result<Value, TelegramError> {
        let url = self.endpoint("getMe");
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| TelegramError::Http(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| TelegramError::Http(e.to_string()))?;
        if !status.is_success() {
            return Err(TelegramError::Api {
                status: status.as_u16(),
                body: text,
            });
        }
        let parsed: ApiResponse<Value> = serde_json::from_str(&text)
            .map_err(|e| TelegramError::Http(format!("parse getMe: {e}")))?;
        if !parsed.ok {
            return Err(TelegramError::NotOk {
                description: parsed.description.unwrap_or_default(),
            });
        }
        parsed.result.ok_or_else(|| TelegramError::NotOk {
            description: "getMe ok=true but result missing".into(),
        })
    }

    async fn post_json<T: Serialize>(
        &self,
        method: &str,
        body: &T,
    ) -> Result<(), TelegramError> {
        let url = self.endpoint(method);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| TelegramError::Http(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| TelegramError::Http(e.to_string()))?;
        if !status.is_success() {
            return Err(TelegramError::Api {
                status: status.as_u16(),
                body: text,
            });
        }
        let parsed: ApiResponse<Value> = serde_json::from_str(&text)
            .map_err(|e| TelegramError::Http(format!("parse {method}: {e}")))?;
        if !parsed.ok {
            return Err(TelegramError::NotOk {
                description: parsed.description.unwrap_or_default(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_uses_canonical_base() {
        let c = TelegramClient::new("123:abc".into()).unwrap();
        assert_eq!(
            c.endpoint("sendMessage"),
            "https://api.telegram.org/bot123:abc/sendMessage"
        );
    }

    #[test]
    fn empty_token_rejected() {
        assert!(matches!(
            TelegramClient::new(String::new()),
            Err(TelegramError::MissingBotToken)
        ));
    }

    #[tokio::test]
    async fn send_empty_text_is_noop() {
        // Construct client with bogus token — empty bubbles never hit network.
        let c = TelegramClient::new("123:abc".into()).unwrap();
        assert!(c.send_message(42, "", None).await.is_ok());
    }
}
