//! `LineMode` — selects between the relay-mediated "hosted" bridge
//! and the user-owned "self_hosted" webhook bridge. Stored on
//! `LineConfig`, used by `bootstrap::spawn` to pick the runtime.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LineMode {
    /// Existing flow — pair via thClaws relay, connect WS to
    /// `wss://line.thclaws.ai/ws`. Replies go through the relay,
    /// which proxies to LINE.
    #[default]
    Hosted,
    /// User runs their own LINE OA + reverse-proxied webhook into
    /// thClaws's local port. thClaws verifies LINE signatures and
    /// calls LINE Reply API directly. No Push API ever.
    SelfHosted,
}

impl LineMode {
    pub fn is_self_hosted(self) -> bool {
        matches!(self, LineMode::SelfHosted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_hosted_for_back_compat() {
        assert_eq!(LineMode::default(), LineMode::Hosted);
        assert!(!LineMode::default().is_self_hosted());
    }

    #[test]
    fn serde_round_trip_snake_case() {
        let h = serde_json::to_string(&LineMode::Hosted).unwrap();
        assert_eq!(h, "\"hosted\"");
        let s = serde_json::to_string(&LineMode::SelfHosted).unwrap();
        assert_eq!(s, "\"self_hosted\"");
        let back: LineMode = serde_json::from_str("\"self_hosted\"").unwrap();
        assert_eq!(back, LineMode::SelfHosted);
    }

    #[test]
    fn unknown_mode_string_fails_parse() {
        let r: Result<LineMode, _> = serde_json::from_str("\"foo\"");
        assert!(r.is_err());
    }
}
