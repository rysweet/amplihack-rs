//! Bloom filter for compact set-membership testing.
//!
//! Matches Python `amplihack/memory/bloom.py`:
//! - Space-efficient (~1 KB for 1000 items at 1% FPR)
//! - Double-hashing (MD5 + SHA1) for k hash positions
//! - No false negatives; configurable false-positive rate
//! - Used by gossip protocol to identify missing nodes between shards

use md5::{Digest as Md5Digest, Md5};
use sha1::Sha1;

/// Bloom filter for compact deduplication.
pub struct BloomFilter {
    bits: Vec<u8>,
    num_bits: usize,
    num_hashes: u32,
    count: usize,
}

impl BloomFilter {
    /// Create a new Bloom filter sized for `capacity` items at the given
    /// false-positive rate.
    pub fn new(capacity: usize, fpr: f64) -> Self {
        let fpr = fpr.clamp(1e-10, 0.5);
        let num_bits = optimal_num_bits(capacity, fpr).max(8);
        let num_hashes = optimal_num_hashes(num_bits, capacity).max(1);
        let byte_len = (num_bits + 7) / 8;
        Self {
            bits: vec![0u8; byte_len],
            num_bits,
            num_hashes,
            count: 0,
        }
    }

    /// Add an item to the filter.
    pub fn add(&mut self, item: &str) {
        for i in 0..self.num_hashes {
            let pos = self.hash_position(item, i);
            self.bits[pos / 8] |= 1 << (pos % 8);
        }
        self.count += 1;
    }

    /// Batch-add multiple items.
    pub fn add_all(&mut self, items: &[&str]) {
        for item in items {
            self.add(item);
        }
    }

    /// Test whether the filter might contain the item.
    /// No false negatives; may return false positives.
    pub fn might_contain(&self, item: &str) -> bool {
        for i in 0..self.num_hashes {
            let pos = self.hash_position(item, i);
            if self.bits[pos / 8] & (1 << (pos % 8)) == 0 {
                return false;
            }
        }
        true
    }

    /// Return items from the input that are NOT in the filter.
    /// Used by gossip to identify which peer nodes we're missing.
    pub fn missing_from<'a>(&self, items: &[&'a str]) -> Vec<&'a str> {
        items
            .iter()
            .filter(|item| !self.might_contain(item))
            .copied()
            .collect()
    }

    /// Number of items added.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Size in bytes.
    pub fn size_bytes(&self) -> usize {
        self.bits.len()
    }

    /// Serialize to bytes for network transmission.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.bits.len());
        out.extend_from_slice(&(self.num_bits as u32).to_le_bytes());
        out.extend_from_slice(&self.num_hashes.to_le_bytes());
        out.extend_from_slice(&(self.count as u32).to_le_bytes());
        out.extend_from_slice(&self.bits);
        out
    }

    /// Deserialize from bytes.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }
        let num_bits = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
        let num_hashes = u32::from_le_bytes(data[4..8].try_into().ok()?);
        let count = u32::from_le_bytes(data[8..12].try_into().ok()?) as usize;
        let bits = data[12..].to_vec();
        if bits.len() < (num_bits + 7) / 8 {
            return None;
        }
        Some(Self {
            bits,
            num_bits,
            num_hashes,
            count,
        })
    }

    /// Double-hashing: position = (h1 + i * h2) % num_bits
    fn hash_position(&self, item: &str, i: u32) -> usize {
        let h1 = {
            let mut hasher = Md5::new();
            hasher.update(item.as_bytes());
            let result = hasher.finalize();
            u64::from_le_bytes(result[..8].try_into().unwrap())
        };
        let h2 = {
            let mut hasher = Sha1::new();
            hasher.update(item.as_bytes());
            let result = hasher.finalize();
            u64::from_le_bytes(result[..8].try_into().unwrap())
        };
        ((h1.wrapping_add((i as u64).wrapping_mul(h2))) % self.num_bits as u64) as usize
    }
}

fn optimal_num_bits(n: usize, fpr: f64) -> usize {
    let n = n.max(1) as f64;
    ((-n * fpr.ln()) / (2.0_f64.ln().powi(2))).ceil() as usize
}

fn optimal_num_hashes(m: usize, n: usize) -> u32 {
    let n = n.max(1) as f64;
    let m = m as f64;
    ((m / n) * 2.0_f64.ln()).ceil() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_membership() {
        let mut bf = BloomFilter::new(100, 0.01);
        bf.add("hello");
        bf.add("world");
        assert!(bf.might_contain("hello"));
        assert!(bf.might_contain("world"));
        assert!(!bf.might_contain("missing"));
    }

    #[test]
    fn missing_from_finds_absent_items() {
        let mut bf = BloomFilter::new(100, 0.01);
        bf.add("a");
        bf.add("b");
        let missing = bf.missing_from(&["a", "b", "c", "d"]);
        assert!(missing.contains(&"c"));
        assert!(missing.contains(&"d"));
        assert!(!missing.contains(&"a"));
    }

    #[test]
    fn batch_add() {
        let mut bf = BloomFilter::new(100, 0.01);
        bf.add_all(&["x", "y", "z"]);
        assert_eq!(bf.count(), 3);
        assert!(bf.might_contain("x"));
        assert!(bf.might_contain("z"));
    }

    #[test]
    fn serialization_round_trip() {
        let mut bf = BloomFilter::new(1000, 0.01);
        bf.add("test-item-1");
        bf.add("test-item-2");
        let bytes = bf.to_bytes();
        let bf2 = BloomFilter::from_bytes(&bytes).unwrap();
        assert!(bf2.might_contain("test-item-1"));
        assert!(bf2.might_contain("test-item-2"));
        assert!(!bf2.might_contain("test-item-3"));
        assert_eq!(bf2.count(), 2);
    }

    #[test]
    fn invalid_bytes_return_none() {
        assert!(BloomFilter::from_bytes(&[]).is_none());
        assert!(BloomFilter::from_bytes(&[0; 5]).is_none());
    }

    #[test]
    fn size_reasonable_for_1000_items() {
        let bf = BloomFilter::new(1000, 0.01);
        // ~1.2 KB for 1000 items at 1% FPR
        assert!(bf.size_bytes() < 2048, "size={}", bf.size_bytes());
        assert!(bf.size_bytes() > 512, "size={}", bf.size_bytes());
    }

    #[test]
    fn low_false_positive_rate() {
        let mut bf = BloomFilter::new(1000, 0.01);
        for i in 0..1000 {
            bf.add(&format!("item-{i}"));
        }
        let mut false_positives = 0;
        for i in 1000..2000 {
            if bf.might_contain(&format!("item-{i}")) {
                false_positives += 1;
            }
        }
        // Allow up to 5% (generous margin over 1% theoretical)
        assert!(
            false_positives < 50,
            "false_positives={false_positives}/1000"
        );
    }
}
