//! Direct LINE Messaging API client — Reply endpoint ONLY.
//!
//! This module deliberately implements only the reply primitive.
//! The unsolicited-send primitive is not exposed: no method, no URL
//! constant, no host. The grep test at the bottom of this file
//! asserts the prohibited symbols never appear in source. When the
//! caller has no valid `replyToken` it gets
//! `DirectLineError::NoValidReplyToken` — no fallback to anything.

use super::chunk::split_for_line;
use super::errors::DirectLineError;
use serde::Serialize;
use std::time::Duration;

pub const REPLY_URL: &str = "https://api.line.me/v2/bot/message/reply";

#[derive(Serialize)]
struct TextMessage<'a> {
    #[serde(rename = "type")]
    typ: &'a str,
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
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| DirectLineError::Http(e.to_string()))?;
        Ok(Self { http, access_token })
    }

    /// Reply with text. Chunks into up to 5 messages of ≤ 4 500 chars.
    /// Empty reply_token short-circuits to `NoValidReplyToken`.
    pub async fn reply_text(
        &self,
        reply_token: &str,
        text: &str,
    ) -> Result<(), DirectLineError> {
        if reply_token.is_empty() {
            return Err(DirectLineError::NoValidReplyToken);
        }
        let bubbles = split_for_line(text);
        if bubbles.is_empty() {
            return Ok(());
        }
        let messages: Vec<TextMessage> = bubbles
            .iter()
            .map(|b| TextMessage {
                typ: "text",
                text: b,
            })
            .collect();
        let payload = ReplyPayload {
            reply_token,
            messages,
        };
        self.post_reply(&payload).await
    }

    /// Reply with a Template Buttons message containing one postback
    /// action — the load-bearing primitive of the slow-response cache.
    /// `postback_data` is opaque to LINE; we use JSON like
    /// `{"action":"show_response","request_id":"<uuid>"}`.
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
        // Template button text ≤ 160 chars; altText ≤ 400 chars (LINE limits).
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
        self.post_reply(&payload).await
    }

    /// Reply with a text message + Quick Reply chips. Used by the
    /// self-hosted approval flow: send the prompt over the user's
    /// inbound reply token, attach `Approve` / `Deny` chips whose
    /// postback `data` payload is parsed by `LineApprover`.
    /// `items` is a Vec of (label, postback_data) pairs.
    pub async fn reply_with_quick_reply(
        &self,
        reply_token: &str,
        text: &str,
        items: &[(String, String)],
    ) -> Result<(), DirectLineError> {
        if reply_token.is_empty() {
            return Err(DirectLineError::NoValidReplyToken);
        }
        // Cap text at LINE's 5000-char limit for safety.
        let body_text: String = text.chars().take(5000).collect();
        let quick_items: Vec<serde_json::Value> = items
            .iter()
            .map(|(label, data)| {
                serde_json::json!({
                    "type": "action",
                    "action": {
                        "type": "postback",
                        "label": label,
                        "data": data,
                        "displayText": label,
                    }
                })
            })
            .collect();
        let payload = serde_json::json!({
            "replyToken": reply_token,
            "messages": [{
                "type": "text",
                "text": body_text,
                "quickReply": { "items": quick_items }
            }]
        });
        self.post_reply(&payload).await
    }

    async fn post_reply<T: Serialize>(&self, payload: &T) -> Result<(), DirectLineError> {
        let resp = self
            .http
            .post(REPLY_URL)
            .bearer_auth(&self.access_token)
            .json(payload)
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

    #[tokio::test]
    async fn template_button_empty_reply_token_rejected() {
        let client = DirectLineClient::new("t".into()).unwrap();
        let r = client
            .reply_template_button("", "wait", "tap me", "{}")
            .await;
        assert!(matches!(r, Err(DirectLineError::NoValidReplyToken)));
    }

    #[test]
    fn source_does_not_contain_prohibited_endpoint_symbols() {
        // Spec rule: the unsolicited-send LINE endpoint must never
        // appear in self-hosted code. If a future refactor introduces
        // any of these symbols, CI fails here. Needles are built via
        // concat! at compile-time so this assertion file itself does
        // not contain the literal strings (which would self-match).
        let src = include_str!("client.rs");
        let lc = src.to_lowercase();

        let needle_path = concat!("message", "/", "p", "ush");
        let needle_host = concat!("p", "ush.line.me");
        let needle_const = concat!("p", "ush_url");
        let needle_fn = concat!("fn ", "p", "ush(");

        assert!(
            !lc.contains(needle_path),
            "prohibited endpoint path must not appear in client.rs"
        );
        assert!(
            !lc.contains(needle_host),
            "prohibited host must not appear in client.rs"
        );
        assert!(
            !lc.contains(needle_const),
            "prohibited URL constant must not appear in client.rs"
        );
        assert!(
            !lc.contains(needle_fn),
            "prohibited function definition must not appear in client.rs"
        );
    }
}
