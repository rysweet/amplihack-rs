//! Content ingestion, fact extraction, and temporal metadata detection.
//!
//! Port of Python `learning_ingestion.py` — provides content truncation,
//! source-label extraction, fast temporal-metadata detection, and the
//! `FactBatch` / `StoredBatchResult` types used by the learning pipeline.

use std::collections::HashMap;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum content length before truncation.
pub const MAX_CONTENT_LENGTH: usize = 50_000;

// ---------------------------------------------------------------------------
// Compiled regexes
// ---------------------------------------------------------------------------

static TIMESTAMP_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\bTimestamp:\s*(\d{4}-\d{2}-\d{2})(?:[ T](\d{2}):(\d{2})(?::(\d{2}))?)?",
    )
    .unwrap()
});

static ISO_DATE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b(\d{4}-\d{2}-\d{2})(?:[ T](\d{2}):(\d{2})(?::(\d{2}))?)?\b").unwrap()
});

static DAY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bDay\s+(\d{1,4})\b").unwrap());

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Temporal metadata extracted from content.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemporalMetadata {
    pub source_date: String,
    pub temporal_order: String,
    pub temporal_index: i64,
}

/// A single prepared fact ready for storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedFact {
    pub context: String,
    pub fact: String,
    pub confidence: f64,
    pub tags: Vec<String>,
    #[serde(default)]
    pub temporal_metadata: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub source_id: Option<String>,
}

/// A prepared batch of facts extracted from content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactBatch {
    pub facts_extracted: usize,
    pub facts: Vec<PreparedFact>,
    pub summary_fact: Option<PreparedFact>,
    pub content_summary: String,
    pub perception: String,
    pub episode_content: String,
    pub source_label: String,
}

/// Result of storing a fact batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredBatchResult {
    pub facts_extracted: usize,
    pub facts_stored: usize,
    pub content_summary: String,
}

// ---------------------------------------------------------------------------
// Public API — deterministic helpers
// ---------------------------------------------------------------------------

/// Truncate content to [`MAX_CONTENT_LENGTH`] if necessary.
pub fn truncate_content(content: &str) -> &str {
    if content.len() > MAX_CONTENT_LENGTH {
        tracing::warn!(
            original_len = content.len(),
            max = MAX_CONTENT_LENGTH,
            "Content truncated"
        );
        &content[..MAX_CONTENT_LENGTH]
    } else {
        content
    }
}

/// Derive a stable source label from the content title or leading text.
pub fn extract_source_label(content: &str) -> String {
    if let Some(stripped) = content.strip_prefix("Title: ")
        && let Some(end) = stripped.find('\n')
    {
        return stripped[..end].trim().to_string();
    }
    content.chars().take(60).collect::<String>().trim().to_string()
}

/// Detect temporal metadata directly from the source text (fast path).
///
/// Returns `Some(TemporalMetadata)` if obvious patterns are found,
/// `None` if LLM extraction would be needed.
pub fn detect_temporal_metadata_fast(content: &str) -> Option<TemporalMetadata> {
    if let Some(cap) = TIMESTAMP_RE.captures(content) {
        let date = cap.get(1).unwrap().as_str();
        let hour = cap.get(2).map_or("00", |m| m.as_str());
        let minute = cap.get(3).map_or("00", |m| m.as_str());
        let second = cap.get(4).map_or("00", |m| m.as_str());
        let temporal_order = format!("{date} {hour}:{minute}:{second}");
        let idx_str = format!("{}{hour}{minute}{second}", date.replace('-', ""));
        let temporal_index = idx_str.parse::<i64>().unwrap_or(0);
        return Some(TemporalMetadata {
            source_date: date.to_string(),
            temporal_order,
            temporal_index,
        });
    }

    if let Some(cap) = ISO_DATE_RE.captures(content) {
        let date = cap.get(1).unwrap().as_str();
        let hour = cap.get(2).map_or("00", |m| m.as_str());
        let minute = cap.get(3).map_or("00", |m| m.as_str());
        let second = cap.get(4).map_or("00", |m| m.as_str());
        let temporal_order = if cap.get(2).is_some() {
            format!("{date} {hour}:{minute}:{second}")
        } else {
            date.to_string()
        };
        let idx_str = format!("{}{hour}{minute}{second}", date.replace('-', ""));
        let temporal_index = idx_str.parse::<i64>().unwrap_or(0);
        return Some(TemporalMetadata {
            source_date: date.to_string(),
            temporal_order,
            temporal_index,
        });
    }

    if let Some(cap) = DAY_RE.captures(content) {
        let day: i64 = cap[1].parse().unwrap_or(0);
        return Some(TemporalMetadata {
            source_date: String::new(),
            temporal_order: format!("Day {day}"),
            temporal_index: day,
        });
    }

    None
}

/// Build the tags list for a prepared fact, merging temporal markers.
pub fn build_fact_tags(
    base_tags: &[String],
    temporal: &TemporalMetadata,
) -> Vec<String> {
    let mut tags: Vec<String> = base_tags.to_vec();
    if !temporal.source_date.is_empty() {
        tags.push(format!("date:{}", temporal.source_date));
    }
    if !temporal.temporal_order.is_empty() {
        tags.push(format!("time:{}", temporal.temporal_order));
    }
    tags
}

/// Create an empty `FactBatch` for blank/empty input.
pub fn empty_batch() -> FactBatch {
    FactBatch {
        facts_extracted: 0,
        facts: Vec::new(),
        summary_fact: None,
        content_summary: "Empty content".into(),
        perception: String::new(),
        episode_content: String::new(),
        source_label: String::new(),
    }
}

/// Whether content contains procedural / step-by-step markers.
pub fn is_procedural_content(content: &str) -> bool {
    let lower = content.to_lowercase();
    ["step 1", "step 2", "steps:", "procedure", "instructions"]
        .iter()
        .any(|kw| lower.contains(kw))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short() {
        let s = "hello world";
        assert_eq!(truncate_content(s), s);
    }

    #[test]
    fn truncate_long() {
        let s = "a".repeat(60_000);
        assert_eq!(truncate_content(&s).len(), MAX_CONTENT_LENGTH);
    }

    #[test]
    fn source_label_title() {
        let c = "Title: My Article\nBody text here.";
        assert_eq!(extract_source_label(c), "My Article");
    }

    #[test]
    fn source_label_no_title() {
        let c = "Some content without a title prefix";
        assert_eq!(extract_source_label(c), "Some content without a title prefix");
    }

    #[test]
    fn source_label_long() {
        let c = "A".repeat(100);
        assert_eq!(extract_source_label(&c).len(), 60);
    }

    #[test]
    fn detect_timestamp() {
        let c = "Timestamp: 2024-03-15T14:30:00 event occurred";
        let m = detect_temporal_metadata_fast(c).unwrap();
        assert_eq!(m.source_date, "2024-03-15");
        assert!(m.temporal_order.contains("14:30:00"));
        assert!(m.temporal_index > 0);
    }

    #[test]
    fn detect_iso_date() {
        let c = "Published on 2024-01-20 in the journal";
        let m = detect_temporal_metadata_fast(c).unwrap();
        assert_eq!(m.source_date, "2024-01-20");
        assert_eq!(m.temporal_order, "2024-01-20");
    }

    #[test]
    fn detect_day_marker() {
        let c = "Day 7 of the experiment showed results";
        let m = detect_temporal_metadata_fast(c).unwrap();
        assert_eq!(m.temporal_order, "Day 7");
        assert_eq!(m.temporal_index, 7);
    }

    #[test]
    fn detect_no_temporal() {
        assert!(detect_temporal_metadata_fast("No temporal info here").is_none());
    }

    #[test]
    fn build_tags_with_temporal() {
        let temporal = TemporalMetadata {
            source_date: "2024-03-15".into(),
            temporal_order: "Day 7".into(),
            temporal_index: 7,
        };
        let tags = build_fact_tags(&["learned".into()], &temporal);
        assert!(tags.contains(&"date:2024-03-15".to_string()));
        assert!(tags.contains(&"time:Day 7".to_string()));
    }

    #[test]
    fn build_tags_empty_temporal() {
        let temporal = TemporalMetadata::default();
        let tags = build_fact_tags(&["learned".into()], &temporal);
        assert_eq!(tags.len(), 1);
    }

    #[test]
    fn empty_batch_check() {
        let b = empty_batch();
        assert_eq!(b.facts_extracted, 0);
        assert_eq!(b.content_summary, "Empty content");
    }

    #[test]
    fn procedural_detection() {
        assert!(is_procedural_content("Step 1: install\nStep 2: build"));
        assert!(!is_procedural_content("Regular content text"));
    }
}
