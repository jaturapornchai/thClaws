//! Split a long assistant reply into Telegram-sized chunks.
//!
//! Telegram sendMessage limit: 4096 chars per message. We chunk at
//! 4000 to leave room for any inline metadata + grapheme safety.

pub const SAFE_BUBBLE_CHARS: usize = 4000;
pub const HARD_LIMIT: usize = 4096;
pub const MAX_BUBBLES: usize = 20; // soft cap so an agent loop can't flood
const TRUNCATE_MARKER: &str = "\n…";

pub fn split_for_telegram(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    let mut out: Vec<String> = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() && out.len() < MAX_BUBBLES {
        if remaining.chars().count() <= SAFE_BUBBLE_CHARS {
            out.push(remaining.to_string());
            remaining = "";
            break;
        }
        let cut = pick_cut(remaining, SAFE_BUBBLE_CHARS);
        let (head, tail) = split_at_char(remaining, cut);
        out.push(head.to_string());
        remaining = tail.trim_start();
    }
    if !remaining.is_empty() && out.len() == MAX_BUBBLES {
        if let Some(last) = out.last_mut() {
            let limit = SAFE_BUBBLE_CHARS.saturating_sub(TRUNCATE_MARKER.chars().count());
            if last.chars().count() > limit {
                *last = last.chars().take(limit).collect();
            }
            last.push_str(TRUNCATE_MARKER);
        }
    }
    out
}

fn pick_cut(s: &str, target: usize) -> usize {
    let chars: Vec<(usize, char)> = s.char_indices().take(target).collect();
    for i in (0..chars.len()).rev() {
        if i + 1 < chars.len() && chars[i].1 == '\n' && chars[i + 1].1 == '\n' {
            return i + 1;
        }
    }
    for i in (0..chars.len()).rev() {
        if chars[i].1 == '\n' {
            return i + 1;
        }
    }
    for i in (0..chars.len()).rev() {
        if chars[i].1 == ' ' {
            return i + 1;
        }
    }
    target
}

fn split_at_char(s: &str, char_idx: usize) -> (&str, &str) {
    let byte_idx = s
        .char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len());
    s.split_at(byte_idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_yields_empty() {
        assert!(split_for_telegram("").is_empty());
    }

    #[test]
    fn short_message_single_bubble() {
        let v = split_for_telegram("hello");
        assert_eq!(v, vec!["hello"]);
    }

    #[test]
    fn under_limit_single() {
        let t = "a".repeat(SAFE_BUBBLE_CHARS);
        let v = split_for_telegram(&t);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn over_limit_splits() {
        let t = "a".repeat(SAFE_BUBBLE_CHARS + 200);
        let v = split_for_telegram(&t);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn caps_at_max_bubbles() {
        let t = "z".repeat(SAFE_BUBBLE_CHARS * (MAX_BUBBLES + 5));
        let v = split_for_telegram(&t);
        assert_eq!(v.len(), MAX_BUBBLES);
        assert!(v.last().unwrap().ends_with(TRUNCATE_MARKER));
    }

    #[test]
    fn thai_utf8_safe() {
        let t = "ก".repeat(SAFE_BUBBLE_CHARS + 50);
        let v = split_for_telegram(&t);
        assert!(v.len() >= 2);
        for b in &v {
            assert!(!b.is_empty());
            // Every bubble well below the Telegram hard limit.
            assert!(b.chars().count() <= HARD_LIMIT);
        }
    }
}
