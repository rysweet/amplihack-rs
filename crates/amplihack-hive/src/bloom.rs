//! Bloom filter for compact shard content summaries.
//!
//! Used by the gossip protocol to efficiently compare shard contents
//! between agents. Each agent maintains a bloom filter of its fact IDs.
//! During gossip, agents exchange bloom filters and pull missing facts.
//!
//! - No false negatives: if `might_contain` returns `false`, the item is truly absent.
//! - Trade a small false-positive rate for massive space savings.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Space-efficient probabilistic set membership test.
///
/// Supports [`add`](BloomFilter::add) and [`might_contain`](BloomFilter::might_contain).
/// False positives possible, false negatives impossible.
#[derive(Clone, Debug)]
pub struct BloomFilter {
    bits: Vec<u8>,
    /// Total number of bits in the filter.
    size: usize,
    num_hashes: usize,
    count: usize,
}

impl BloomFilter {
    /// Create a new bloom filter sized for `expected_items` at the given
    /// false-positive rate (default 1%).
    pub fn new(expected_items: usize, false_positive_rate: f64) -> Self {
        let n = expected_items.max(1);
        let fpr = false_positive_rate.clamp(f64::MIN_POSITIVE, 1.0);

        // Optimal bit-array size: m = -n·ln(p) / (ln2)²
        let size = 64.max((-((n as f64) * fpr.ln()) / (2_f64.ln().powi(2))).ceil() as usize);

        // Optimal hash count: k = (m/n)·ln2
        let num_hashes = 1.max(((size as f64 / n as f64) * 2_f64.ln()).round() as usize);

        let bytes = size.div_ceil(8);
        Self {
            bits: vec![0u8; bytes],
            size,
            num_hashes,
            count: 0,
        }
    }

    /// Generate `num_hashes` bit positions using double-hashing (Kirsch–Mitzenmacker).
    fn get_hashes(&self, item: &str) -> Vec<usize> {
        let h1 = hash_with_seed(item, 0);
        let h2 = hash_with_seed(item, 0x9e37_79b9_7f4a_7c15); // golden-ratio-derived seed
        (0..self.num_hashes)
            .map(|i| (h1.wrapping_add((i as u64).wrapping_mul(h2)) % self.size as u64) as usize)
            .collect()
    }

    /// Add an item to the bloom filter.
    pub fn add(&mut self, item: &str) {
        for pos in self.get_hashes(item) {
            self.bits[pos >> 3] |= 1 << (pos & 7);
        }
        self.count += 1;
    }

    /// Test if an item might be in the set.
    ///
    /// Returns `true` if possibly present, `false` if definitely absent.
    pub fn might_contain(&self, item: &str) -> bool {
        self.get_hashes(item)
            .iter()
            .all(|&pos| (self.bits[pos >> 3] & (1 << (pos & 7))) != 0)
    }

    /// Add multiple items at once.
    pub fn add_all(&mut self, items: &[impl AsRef<str>]) {
        for item in items {
            self.add(item.as_ref());
        }
    }

    /// Return items from the slice that are **not** in this filter.
    pub fn missing_from<'a>(&self, items: &[&'a str]) -> Vec<&'a str> {
        items
            .iter()
            .filter(|item| !self.might_contain(item))
            .copied()
            .collect()
    }

    /// Approximate number of items added.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Size of the underlying bit array in bytes.
    pub fn size_bytes(&self) -> usize {
        self.bits.len()
    }

    /// Serialize the bloom filter for network transmission.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.bits.clone()
    }

    /// Deserialize a bloom filter from bytes.
    ///
    /// The `expected_items` and `false_positive_rate` must match the original
    /// parameters used to create the filter.
    pub fn from_bytes(data: &[u8], expected_items: usize, false_positive_rate: f64) -> Self {
        let mut bf = Self::new(expected_items, false_positive_rate);
        let copy_len = data.len().min(bf.bits.len());
        bf.bits[..copy_len].copy_from_slice(&data[..copy_len]);
        bf
    }
}

fn hash_with_seed(item: &str, seed: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    item.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_query() {
        let mut bf = BloomFilter::new(100, 0.01);
        bf.add("hello");
        bf.add("world");
        assert!(bf.might_contain("hello"));
        assert!(bf.might_contain("world"));
        assert!(!bf.might_contain("missing"));
    }

    #[test]
    fn count_tracks_insertions() {
        let mut bf = BloomFilter::new(100, 0.01);
        assert_eq!(bf.count(), 0);
        bf.add("a");
        bf.add("b");
        assert_eq!(bf.count(), 2);
    }

    #[test]
    fn add_all_works() {
        let mut bf = BloomFilter::new(100, 0.01);
        bf.add_all(&["a", "b", "c"]);
        assert!(bf.might_contain("a"));
        assert!(bf.might_contain("b"));
        assert!(bf.might_contain("c"));
        assert_eq!(bf.count(), 3);
    }

    #[test]
    fn add_all_with_strings() {
        let mut bf = BloomFilter::new(100, 0.01);
        let items = vec!["x".to_string(), "y".to_string()];
        bf.add_all(&items);
        assert!(bf.might_contain("x"));
        assert!(bf.might_contain("y"));
    }

    #[test]
    fn missing_from_returns_absent_items() {
        let mut bf = BloomFilter::new(100, 0.01);
        bf.add("present");
        let missing = bf.missing_from(&["present", "absent1", "absent2"]);
        assert!(missing.contains(&"absent1"));
        assert!(missing.contains(&"absent2"));
        assert!(!missing.contains(&"present"));
    }

    #[test]
    fn serialization_roundtrip() {
        let mut bf = BloomFilter::new(100, 0.01);
        bf.add("test-item");
        let bytes = bf.to_bytes();
        let bf2 = BloomFilter::from_bytes(&bytes, 100, 0.01);
        assert!(bf2.might_contain("test-item"));
        assert!(!bf2.might_contain("other"));
    }

    #[test]
    fn size_bytes_positive() {
        let bf = BloomFilter::new(1000, 0.01);
        assert!(bf.size_bytes() > 0);
    }

    #[test]
    fn minimum_size_enforced() {
        let bf = BloomFilter::new(1, 0.99);
        // Minimum 64 bits = 8 bytes
        assert!(bf.size_bytes() >= 8);
    }

    #[test]
    fn no_false_negatives() {
        let mut bf = BloomFilter::new(500, 0.01);
        let items: Vec<String> = (0..500).map(|i| format!("item-{i}")).collect();
        for item in &items {
            bf.add(item);
        }
        for item in &items {
            assert!(bf.might_contain(item), "false negative for {item}");
        }
    }

    #[test]
    fn low_false_positive_rate() {
        let mut bf = BloomFilter::new(1000, 0.01);
        for i in 0..1000 {
            bf.add(&format!("in-{i}"));
        }
        let false_positives = (0..10_000)
            .filter(|i| bf.might_contain(&format!("out-{i}")))
            .count();
        // Allow up to 5% — generous margin over the 1% target
        assert!(
            false_positives < 500,
            "too many false positives: {false_positives}/10000"
        );
    }

    #[test]
    fn from_bytes_truncates_excess_data() {
        let mut bf = BloomFilter::new(10, 0.01);
        bf.add("x");
        let mut oversized = bf.to_bytes();
        oversized.extend_from_slice(&[0xff; 100]);
        let bf2 = BloomFilter::from_bytes(&oversized, 10, 0.01);
        assert!(bf2.might_contain("x"));
    }

    #[test]
    fn empty_filter_contains_nothing() {
        let bf = BloomFilter::new(100, 0.01);
        assert!(!bf.might_contain("anything"));
        assert_eq!(bf.count(), 0);
    }
}
