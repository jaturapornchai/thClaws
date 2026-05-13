//! Split a long agent reply into LINE-shaped chunks.
//!
//! Limits (from LINE Messaging API):
//!   - max 5 text messages per Reply call
//!   - 5000 char hard ceiling per bubble; we use 4500 as safe size

pub const MAX_MESSAGES_PER_REPLY: usize = 5;
pub const SAFE_BUBBLE_CHARS: usize = 4500;
const TRUNCATE_MARKER: &str = "\n…";

pub fn split_for_line(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    let mut out: Vec<String> = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() && out.len() < MAX_MESSAGES_PER_REPLY {
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

    if !remaining.is_empty() && out.len() == MAX_MESSAGES_PER_REPLY {
        if let Some(last) = out.last_mut() {
            let limit = SAFE_BUBBLE_CHARS.saturating_sub(TRUNCATE_MARKER.chars().count());
            if last.chars().count() > limit {
                *last = take_chars(last, limit);
            }
            last.push_str(TRUNCATE_MARKER);
        }
    }
    out
}

fn pick_cut(s: &str, target: usize) -> usize {
    let chars: Vec<(usize, char)> = s.char_indices().take(target).collect();
    // Try paragraph break
    for i in (0..chars.len()).rev() {
        if i + 1 < chars.len() && chars[i].1 == '\n' && chars[i + 1].1 == '\n' {
            return i + 1;
        }
    }
    // Try single newline
    for i in (0..chars.len()).rev() {
        if chars[i].1 == '\n' {
            return i + 1;
        }
    }
    // Try space
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

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_empty_vec() {
        assert!(split_for_line("").is_empty());
    }

    #[test]
    fn short_message_single_bubble() {
        let v = split_for_line("hi");
        assert_eq!(v, vec!["hi"]);
    }

    #[test]
    fn under_limit_single_bubble() {
        let text = "a".repeat(SAFE_BUBBLE_CHARS);
        let v = split_for_line(&text);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].chars().count(), SAFE_BUBBLE_CHARS);
    }

    #[test]
    fn slightly_over_limit_splits_to_two() {
        let text = "a".repeat(SAFE_BUBBLE_CHARS + 100);
        let v = split_for_line(&text);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn caps_at_five_messages_with_truncate_marker() {
        let text = "z".repeat(SAFE_BUBBLE_CHARS * 6);
        let v = split_for_line(&text);
        assert_eq!(v.len(), MAX_MESSAGES_PER_REPLY);
        assert!(v.last().unwrap().ends_with(TRUNCATE_MARKER));
    }

    #[test]
    fn utf8_safe_thai_split() {
        let chunk = "ก".repeat(SAFE_BUBBLE_CHARS);
        let text = format!("{}{}", chunk, "ข".repeat(100));
        let v = split_for_line(&text);
        assert!(!v.is_empty());
        for b in &v {
            assert!(!b.is_empty());
        }
    }
}
