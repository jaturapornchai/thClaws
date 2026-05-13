//! Bounded LRU dedup keyed by LINE `webhookEventId`. ~1024 entries;
//! evicts oldest 128 when at capacity.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

const CAP: usize = 1024;
const EVICT_BATCH: usize = 128;

#[derive(Default)]
struct Inner {
    seen: HashMap<String, ()>,
    order: VecDeque<String>,
}

#[derive(Default)]
pub struct DedupStore {
    inner: Mutex<Inner>,
}

impl DedupStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if `id` is new (now recorded); `false` if seen.
    /// Empty id is treated as "cannot dedup" → always passes through.
    pub fn check_and_record(&self, id: &str) -> bool {
        if id.is_empty() {
            return true;
        }
        let mut g = self.inner.lock().unwrap();
        if g.seen.contains_key(id) {
            return false;
        }
        if g.order.len() >= CAP {
            for _ in 0..EVICT_BATCH {
                if let Some(old) = g.order.pop_front() {
                    g.seen.remove(&old);
                }
            }
        }
        g.seen.insert(id.to_string(), ());
        g.order.push_back(id.to_string());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sighting_passes() {
        let d = DedupStore::new();
        assert!(d.check_and_record("abc"));
    }

    #[test]
    fn second_sighting_rejected() {
        let d = DedupStore::new();
        assert!(d.check_and_record("abc"));
        assert!(!d.check_and_record("abc"));
    }

    #[test]
    fn distinct_ids_independent() {
        let d = DedupStore::new();
        assert!(d.check_and_record("a"));
        assert!(d.check_and_record("b"));
        assert!(!d.check_and_record("a"));
    }

    #[test]
    fn empty_id_always_passes() {
        let d = DedupStore::new();
        assert!(d.check_and_record(""));
        assert!(d.check_and_record(""));
    }

    #[test]
    fn evicts_oldest_when_full() {
        let d = DedupStore::new();
        for i in 0..CAP {
            assert!(d.check_and_record(&format!("e{}", i)));
        }
        assert!(d.check_and_record("new"));
        assert!(d.check_and_record("e0"));
        assert!(!d.check_and_record("new"));
    }
}
