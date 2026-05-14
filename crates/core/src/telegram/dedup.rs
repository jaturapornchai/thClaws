//! Bounded LRU dedup on Telegram `update_id`. Prevents duplicate
//! agent turns when:
//!   * the webhook delivery is retried by Telegram after a 5xx /
//!     timeout (the same Update is POSTed again)
//!   * `getUpdates` rewinds offset because we crashed before
//!     committing the last consumed ID (best-effort persistence
//!     belongs to a different layer; this catches the in-process
//!     duplicates regardless)
//!
//! Tiny by design — bounded so a bot under sustained replay attack
//! can't OOM the worker. Once full, the oldest seen update_id is
//! evicted; replays of evicted IDs become indistinguishable from
//! genuinely new updates, but `update_id` is monotonic per bot so a
//! moderate capacity (1024) covers the realistic out-of-order +
//! retry window.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

pub const DEFAULT_CAPACITY: usize = 1024;

pub struct DedupStore {
    inner: Mutex<Inner>,
}

struct Inner {
    seen: HashSet<i64>,
    order: VecDeque<i64>,
    capacity: usize,
}

impl DedupStore {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                seen: HashSet::with_capacity(capacity),
                order: VecDeque::with_capacity(capacity),
                capacity: capacity.max(1),
            }),
        }
    }

    /// Returns `true` if this is the first time we've seen the id
    /// (caller should proceed). `false` means a duplicate — drop.
    pub fn check_and_record(&self, update_id: i64) -> bool {
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if g.seen.contains(&update_id) {
            return false;
        }
        // Evict oldest if at capacity.
        if g.order.len() >= g.capacity {
            if let Some(old) = g.order.pop_front() {
                g.seen.remove(&old);
            }
        }
        g.seen.insert(update_id);
        g.order.push_back(update_id);
        true
    }
}

impl Default for DedupStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_id_passes() {
        let d = DedupStore::new();
        assert!(d.check_and_record(100));
    }

    #[test]
    fn duplicate_id_blocked() {
        let d = DedupStore::new();
        assert!(d.check_and_record(100));
        assert!(!d.check_and_record(100));
        assert!(!d.check_and_record(100));
    }

    #[test]
    fn distinct_ids_each_pass_once() {
        let d = DedupStore::new();
        assert!(d.check_and_record(1));
        assert!(d.check_and_record(2));
        assert!(d.check_and_record(3));
        assert!(!d.check_and_record(2));
        assert!(d.check_and_record(4));
    }

    #[test]
    fn lru_eviction_at_capacity() {
        let d = DedupStore::with_capacity(3);
        assert!(d.check_and_record(1));
        assert!(d.check_and_record(2));
        assert!(d.check_and_record(3));
        // capacity reached. Still dedups the existing ids.
        assert!(!d.check_and_record(1));
        assert!(!d.check_and_record(2));
        assert!(!d.check_and_record(3));
        // Adding 4 evicts the oldest (1).
        assert!(d.check_and_record(4));
        // 1 was evicted → re-recording passes (acceptable for
        // long-poll where update_id is monotonic, so an evicted old
        // id is unlikely to re-arrive anyway).
        assert!(d.check_and_record(1));
        // 1 just got re-inserted, evicting 2. 3 and 4 still in.
        assert!(!d.check_and_record(3));
        assert!(!d.check_and_record(4));
        assert!(d.check_and_record(2)); // re-record allowed
    }

    #[test]
    fn zero_capacity_floor_is_one() {
        let d = DedupStore::with_capacity(0);
        assert!(d.check_and_record(1));
        // Capacity of 1 means each new id evicts the previous.
        assert!(d.check_and_record(2));
        assert!(d.check_and_record(1)); // old, re-recorded
    }
}
