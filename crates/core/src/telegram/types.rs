//! Telegram Bot API wire shapes. Tolerant — we only deserialize the
//! fields we actually use. Unknown / future fields are dropped.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct ApiResponse<T> {
    pub ok: bool,
    #[serde(default = "Option::default")]
    pub result: Option<T>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub error_code: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub channel_post: Option<Message>,
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub message_id: i64,
    #[serde(default)]
    pub from: Option<User>,
    pub chat: Chat,
    #[serde(default)]
    pub text: Option<String>,
    /// Reply target — used to detect when a user replies directly to
    /// the bot in a group.
    #[serde(default)]
    pub reply_to_message: Option<Box<Message>>,
    /// MessageEntity[] — used to detect `@mention` of the bot in
    /// group chats so `require_mention_in_groups` works.
    #[serde(default)]
    pub entities: Vec<MessageEntity>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageEntity {
    #[serde(rename = "type")]
    pub typ: String,
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
    /// "private" | "group" | "supergroup" | "channel"
    #[serde(rename = "type")]
    pub typ: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

impl Chat {
    pub fn is_private(&self) -> bool {
        self.typ == "private"
    }
    pub fn is_group(&self) -> bool {
        self.typ == "group" || self.typ == "supergroup"
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub message: Option<Message>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dm_message() {
        let raw = r#"{
            "update_id": 100,
            "message": {
                "message_id": 1,
                "from": {"id": 42, "is_bot": false, "first_name": "Alice"},
                "chat": {"id": 42, "type": "private"},
                "text": "hi"
            }
        }"#;
        let u: Update = serde_json::from_str(raw).unwrap();
        assert_eq!(u.update_id, 100);
        let m = u.message.unwrap();
        assert_eq!(m.text.as_deref(), Some("hi"));
        assert!(m.chat.is_private());
    }

    #[test]
    fn parse_callback_query() {
        let raw = r#"{
            "update_id": 200,
            "callback_query": {
                "id": "cb-1",
                "from": {"id": 42, "is_bot": false},
                "data": "allow:req-abc"
            }
        }"#;
        let u: Update = serde_json::from_str(raw).unwrap();
        let cb = u.callback_query.unwrap();
        assert_eq!(cb.id, "cb-1");
        assert_eq!(cb.data.as_deref(), Some("allow:req-abc"));
    }

    #[test]
    fn api_response_error_shape() {
        let raw = r#"{"ok": false, "error_code": 401, "description": "Unauthorized"}"#;
        let r: ApiResponse<Update> = serde_json::from_str(raw).unwrap();
        assert!(!r.ok);
        assert_eq!(r.description.as_deref(), Some("Unauthorized"));
        assert!(r.result.is_none());
    }

    #[test]
    fn group_message_with_mention_entity() {
        let raw = r#"{
            "update_id": 300,
            "message": {
                "message_id": 5,
                "from": {"id": 42, "is_bot": false},
                "chat": {"id": -100123, "type": "supergroup", "title": "QA"},
                "text": "@thclawsbot hello",
                "entities": [{"type": "mention", "offset": 0, "length": 12}]
            }
        }"#;
        let u: Update = serde_json::from_str(raw).unwrap();
        let m = u.message.unwrap();
        assert!(m.chat.is_group());
        assert_eq!(m.entities.len(), 1);
        assert_eq!(m.entities[0].typ, "mention");
    }
}
