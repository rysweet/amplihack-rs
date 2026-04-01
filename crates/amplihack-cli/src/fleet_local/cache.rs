//! LRU capture cache for tmux pane output.

use std::collections::VecDeque;

use super::{CAPTURE_CACHE_CAPACITY, CAPTURE_CACHE_ENTRY_MAX_BYTES};

// ── FleetCaptureCache ─────────────────────────────────────────────────────────

/// In-memory LRU cache for tmux capture output.
///
/// - Capacity: [`CAPTURE_CACHE_CAPACITY`] entries (64).
/// - Per-entry size cap: [`CAPTURE_CACHE_ENTRY_MAX_BYTES`] bytes (64 KiB).
/// - Keyed by `session_id`.
///
/// # Notes on serialisation
///
/// Any parent struct that is `Serialize` **must** mark this field
/// `#[serde(skip)]` to prevent accidental serialisation of ephemeral terminal
/// content to disk (SEC-12).
pub struct FleetCaptureCache {
    pub(crate) inner: VecDeque<(String, String)>,
    pub(crate) capacity: usize,
}

impl FleetCaptureCache {
    /// Create a new cache with the default capacity (64 entries).
    pub fn new() -> Self {
        Self {
            inner: VecDeque::new(),
            capacity: CAPTURE_CACHE_CAPACITY,
        }
    }

    /// Number of entries currently stored.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Return `true` if the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Insert or update an entry.
    ///
    /// - Truncates `output` to [`CAPTURE_CACHE_ENTRY_MAX_BYTES`] bytes before
    ///   storing (SEC-10).
    /// - Removes any existing entry for `session_id` before inserting.
    /// - Evicts the **oldest** entry when capacity is reached.
    pub fn insert(&mut self, session_id: String, output: String) {
        // SEC-10: cap at 64 KiB, truncating at a UTF-8 boundary.
        let output = if output.len() > CAPTURE_CACHE_ENTRY_MAX_BYTES {
            // Find the last valid UTF-8 boundary at or before the limit.
            let mut boundary = CAPTURE_CACHE_ENTRY_MAX_BYTES;
            while !output.is_char_boundary(boundary) {
                boundary -= 1;
            }
            output[..boundary].to_string()
        } else {
            output
        };

        // Remove any existing entry for this session.
        self.inner.retain(|(k, _)| k != &session_id);

        // Evict the oldest entry if we are at capacity.
        while self.inner.len() >= self.capacity {
            self.inner.pop_front();
        }

        self.inner.push_back((session_id, output));
    }

    /// Retrieve the capture output for `session_id`, if present.
    pub fn get(&self, session_id: &str) -> Option<&str> {
        self.inner
            .iter()
            .find(|(k, _)| k == session_id)
            .map(|(_, v)| v.as_str())
    }
}

impl Default for FleetCaptureCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleet_capture_cache_empty_on_creation() {
        let cache = FleetCaptureCache::new();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn fleet_capture_cache_insert_and_get() {
        let mut cache = FleetCaptureCache::new();
        cache.insert("session-1".to_string(), "output text".to_string());
        assert_eq!(cache.get("session-1"), Some("output text"));
        assert!(cache.get("session-2").is_none());
    }

    #[test]
    fn fleet_capture_cache_update_existing_entry_no_duplicate() {
        let mut cache = FleetCaptureCache::new();
        cache.insert("session-1".to_string(), "old output".to_string());
        cache.insert("session-1".to_string(), "new output".to_string());
        assert_eq!(cache.len(), 1, "update should not create a duplicate entry");
        assert_eq!(cache.get("session-1"), Some("new output"));
    }

    #[test]
    fn fleet_capture_cache_evicts_oldest_at_capacity() {
        let mut cache = FleetCaptureCache::new();
        for i in 0..CAPTURE_CACHE_CAPACITY {
            cache.insert(format!("session-{i}"), format!("output-{i}"));
        }
        assert_eq!(cache.len(), CAPTURE_CACHE_CAPACITY);

        cache.insert(
            format!("session-{CAPTURE_CACHE_CAPACITY}"),
            format!("output-{CAPTURE_CACHE_CAPACITY}"),
        );
        assert_eq!(
            cache.len(),
            CAPTURE_CACHE_CAPACITY,
            "capacity must not grow"
        );
        assert!(
            cache.get("session-0").is_none(),
            "oldest entry must be evicted"
        );
        assert!(
            cache
                .get(&format!("session-{CAPTURE_CACHE_CAPACITY}"))
                .is_some(),
            "newest entry must be present"
        );
    }

    #[test]
    fn fleet_capture_cache_caps_entry_at_64kib() {
        let mut cache = FleetCaptureCache::new();
        let oversized = "x".repeat(CAPTURE_CACHE_ENTRY_MAX_BYTES + 1024);
        cache.insert("big-session".to_string(), oversized);

        let stored = cache.get("big-session").expect("entry must be stored");
        assert!(
            stored.len() <= CAPTURE_CACHE_ENTRY_MAX_BYTES,
            "stored entry ({} bytes) must not exceed 64 KiB cap",
            stored.len()
        );
    }

    #[test]
    fn fleet_capture_cache_default_matches_new() {
        let a = FleetCaptureCache::new();
        let b = FleetCaptureCache::default();
        assert_eq!(a.capacity, b.capacity);
        assert_eq!(a.len(), b.len());
    }
}
