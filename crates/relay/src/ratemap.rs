//! Keyed token buckets with a hard entry cap and a sweep for idle keys.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One bucket per key. A bucket holds `capacity` tokens and refills fully
/// over `refill`. A full bucket is indistinguishable from a missing one, so
/// [`RateMap::sweep`] drops those, and new keys are refused once `max_entries`
/// keys are tracked.
pub struct RateMap<K> {
    capacity: f64,
    per_sec: f64,
    max_entries: usize,
    buckets: Mutex<HashMap<K, (f64, Instant)>>,
}

impl<K: Eq + Hash> RateMap<K> {
    pub fn new(capacity: u32, refill: Duration, max_entries: usize) -> Self {
        let capacity = capacity.max(1) as f64;
        let per_sec = capacity / refill.as_secs_f64().max(f64::MIN_POSITIVE);
        Self { capacity, per_sec, max_entries, buckets: Mutex::new(HashMap::new()) }
    }

    /// Takes one token for `key`. False when the bucket is empty or the map
    /// is full and `key` is new.
    pub fn allow(&self, key: K) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("rate map lock poisoned");
        if !buckets.contains_key(&key) && buckets.len() >= self.max_entries { return false; }
        let (tokens, last) = buckets.entry(key).or_insert((self.capacity, now));
        *tokens = (*tokens + now.duration_since(*last).as_secs_f64() * self.per_sec).min(self.capacity);
        *last = now;
        if *tokens < 1.0 { return false; }
        *tokens -= 1.0;
        true
    }

    /// Removes buckets that have refilled completely. Returns how many.
    pub fn sweep(&self, now: Instant) -> usize {
        let mut buckets = self.buckets.lock().expect("rate map lock poisoned");
        let before = buckets.len();
        buckets.retain(|_, (tokens, last)| *tokens + now.saturating_duration_since(*last).as_secs_f64() * self.per_sec < self.capacity);
        before - buckets.len()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize { self.buckets.lock().expect("rate map lock poisoned").len() }
}

#[cfg(test)]
mod tests {
    use super::RateMap;
    use std::time::{Duration, Instant};

    #[test]
    fn bucket_empties_then_refills() {
        let map = RateMap::new(2, Duration::from_millis(100), 10);
        assert!(map.allow(1)); assert!(map.allow(1)); assert!(!map.allow(1));
        assert!(map.allow(2));
        std::thread::sleep(Duration::from_millis(60));
        assert!(map.allow(1));
    }

    #[test]
    fn frequent_attempts_still_refill() {
        // Fractional refill must accumulate rather than truncate to zero.
        let map = RateMap::new(1, Duration::from_millis(40), 10);
        assert!(map.allow(()));
        let start = Instant::now();
        let mut allowed = false;
        while start.elapsed() < Duration::from_millis(200) {
            if map.allow(()) { allowed = true; break; }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(allowed);
    }

    #[test]
    fn cap_refuses_new_keys_until_swept() {
        let map = RateMap::new(1, Duration::from_millis(20), 2);
        assert!(map.allow(1)); assert!(map.allow(2));
        assert!(!map.allow(3), "map is full");
        assert_eq!(map.sweep(Instant::now()), 0, "drained buckets are kept");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(map.sweep(Instant::now()), 2);
        assert_eq!(map.len(), 0);
        assert!(map.allow(3));
    }
}
