//! LINE webhook wire shapes. Deliberately tolerant — unknown event
//! types fall into `Event::Other` so a LINE platform addition does
//! not break the bridge.

use super::allowlist::Source;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct WebhookBody {
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Message {
        #[serde(rename = "webhookEventId", default)]
        webhook_event_id: String,
        #[serde(rename = "replyToken", default)]
        reply_token: String,
        source: Source,
        message: MessagePayload,
    },
    Postback {
        #[serde(rename = "webhookEventId", default)]
        webhook_event_id: String,
        #[serde(rename = "replyToken", default)]
        reply_token: String,
        source: Source,
        postback: PostbackPayload,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MessagePayload {
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostbackPayload {
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShowResponsePostback {
    pub action: String,
    pub request_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_message_event() {
        let body = r#"{
            "destination": "Uxxx",
            "events": [{
                "type": "message",
                "webhookEventId": "evt1",
                "replyToken": "tok1",
                "source": {"type":"user","userId":"U123"},
                "message": {"type":"text","text":"hi"}
            }]
        }"#;
        let b: WebhookBody = serde_json::from_str(body).unwrap();
        assert_eq!(b.events.len(), 1);
        match &b.events[0] {
            Event::Message {
                webhook_event_id,
                reply_token,
                message,
                ..
            } => {
                assert_eq!(webhook_event_id, "evt1");
                assert_eq!(reply_token, "tok1");
                match message {
                    MessagePayload::Text { text } => assert_eq!(text, "hi"),
                    _ => panic!(),
                }
            }
            _ => panic!("expected Message event"),
        }
    }

    #[test]
    fn parses_postback_event() {
        let body = r#"{"events":[{
            "type":"postback",
            "webhookEventId":"evt2",
            "replyToken":"tok2",
            "source":{"type":"user","userId":"U1"},
            "postback":{"data":"{\"action\":\"show_response\",\"request_id\":\"abc\"}"}
        }]}"#;
        let b: WebhookBody = serde_json::from_str(body).unwrap();
        match &b.events[0] {
            Event::Postback { postback, .. } => {
                let p: ShowResponsePostback = serde_json::from_str(&postback.data).unwrap();
                assert_eq!(p.action, "show_response");
                assert_eq!(p.request_id, "abc");
            }
            _ => panic!("expected Postback event"),
        }
    }

    #[test]
    fn unknown_event_falls_into_other() {
        let body = r#"{"events":[{"type":"follow","timestamp":1,"source":{"type":"user","userId":"U1"},"replyToken":"x"}]}"#;
        let b: WebhookBody = serde_json::from_str(body).unwrap();
        assert!(matches!(b.events[0], Event::Other));
    }

    #[test]
    fn non_text_message_falls_into_other() {
        let body = r#"{"events":[{
            "type":"message",
            "webhookEventId":"e","replyToken":"t",
            "source":{"type":"user","userId":"U1"},
            "message":{"type":"sticker","stickerId":"1"}
        }]}"#;
        let b: WebhookBody = serde_json::from_str(body).unwrap();
        match &b.events[0] {
            Event::Message { message, .. } => assert!(matches!(message, MessagePayload::Other)),
            _ => panic!(),
        }
    }
}
