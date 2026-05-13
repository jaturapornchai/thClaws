//! Per-source allowlist. Events whose source id is not in the
//! configured set are rejected before reaching agent logic. Empty
//! set for a type means "deny all of that type" — fail-closed.

use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    pub users: HashSet<String>,
    pub groups: HashSet<String>,
    pub rooms: HashSet<String>,
}

/// Wire shape of `event.source`. Tolerant — unknown fields ignored;
/// missing id => not allowed.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Source {
    User {
        #[serde(rename = "userId", default)]
        user_id: Option<String>,
    },
    Group {
        #[serde(rename = "groupId", default)]
        group_id: Option<String>,
        #[serde(rename = "userId", default)]
        user_id: Option<String>,
    },
    Room {
        #[serde(rename = "roomId", default)]
        room_id: Option<String>,
        #[serde(rename = "userId", default)]
        user_id: Option<String>,
    },
}

impl Source {
    /// Stable key for the chat — user_id for DM, group_id for group,
    /// room_id for room. Used by reply_store + slow_response cache.
    pub fn chat_id(&self) -> Option<&str> {
        match self {
            Source::User { user_id } => user_id.as_deref(),
            Source::Group { group_id, .. } => group_id.as_deref(),
            Source::Room { room_id, .. } => room_id.as_deref(),
        }
    }
}

impl Allowlist {
    pub fn from_csv(users: &str, groups: &str, rooms: &str) -> Self {
        fn parse(s: &str) -> HashSet<String> {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        }
        Self {
            users: parse(users),
            groups: parse(groups),
            rooms: parse(rooms),
        }
    }

    pub fn is_allowed(&self, source: &Source) -> bool {
        match source {
            Source::User { user_id: Some(id) } => self.users.contains(id),
            Source::Group {
                group_id: Some(id), ..
            } => self.groups.contains(id),
            Source::Room {
                room_id: Some(id), ..
            } => self.rooms.contains(id),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_allowed_when_listed() {
        let al = Allowlist::from_csv("U123,U456", "", "");
        assert!(al.is_allowed(&Source::User {
            user_id: Some("U123".into())
        }));
    }

    #[test]
    fn user_denied_when_not_listed() {
        let al = Allowlist::from_csv("U123", "", "");
        assert!(!al.is_allowed(&Source::User {
            user_id: Some("UXXX".into())
        }));
    }

    #[test]
    fn group_path() {
        let al = Allowlist::from_csv("", "G1", "");
        assert!(al.is_allowed(&Source::Group {
            group_id: Some("G1".into()),
            user_id: None,
        }));
    }

    #[test]
    fn room_path() {
        let al = Allowlist::from_csv("", "", "R1");
        assert!(al.is_allowed(&Source::Room {
            room_id: Some("R1".into()),
            user_id: None,
        }));
    }

    #[test]
    fn missing_id_denied() {
        let al = Allowlist::from_csv("U1", "G1", "R1");
        assert!(!al.is_allowed(&Source::User { user_id: None }));
    }

    #[test]
    fn empty_list_denies_all() {
        let al = Allowlist::default();
        assert!(!al.is_allowed(&Source::User {
            user_id: Some("U1".into()),
        }));
    }

    #[test]
    fn csv_trims_and_skips_blanks() {
        let al = Allowlist::from_csv(" U1 , ,U2,", "", "");
        assert_eq!(al.users.len(), 2);
    }

    #[test]
    fn deserialize_source_user() {
        let s: Source = serde_json::from_str(r#"{"type":"user","userId":"U123"}"#).unwrap();
        assert_eq!(s.chat_id(), Some("U123"));
    }
}
