//! Hive learning feed — publishes content events to the hive.
//!
//! Matches Python `amplihack/workloads/hive/_feed.py`:
//! - Builds content pool from configured sources
//! - Publishes HIVE_LEARN_CONTENT events
//! - Emits HIVE_FEED_COMPLETE when done

use crate::error::{HiveError, Result};
use crate::event_bus::EventBus;
use crate::hive_events::{make_feed_complete_event, make_learn_content_event};
use tracing::{debug, info};
use uuid::Uuid;

/// Configuration for a learning feed.
#[derive(Debug, Clone)]
pub struct FeedConfig {
    pub source_name: String,
    pub content_items: Vec<String>,
    pub feed_id: Option<String>,
}

impl FeedConfig {
    pub fn new(source_name: impl Into<String>, items: Vec<String>) -> Self {
        Self {
            source_name: source_name.into(),
            content_items: items,
            feed_id: None,
        }
    }

    pub fn with_feed_id(mut self, id: impl Into<String>) -> Self {
        self.feed_id = Some(id.into());
        self
    }
}

/// Result of running a feed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FeedResult {
    pub feed_id: String,
    pub items_published: u32,
    pub errors: Vec<String>,
}

/// Run a learning feed, publishing content to the event bus.
pub fn run_feed(bus: &mut dyn EventBus, config: &FeedConfig) -> Result<FeedResult> {
    let feed_id = config
        .feed_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    info!(
        feed_id = %feed_id,
        source = %config.source_name,
        items = config.content_items.len(),
        "Starting learning feed"
    );

    let mut published = 0u32;
    let mut errors = Vec::new();

    for (i, content) in config.content_items.iter().enumerate() {
        if content.trim().is_empty() {
            debug!(index = i, "Skipping empty content item");
            continue;
        }

        let event = make_learn_content_event(&config.source_name, content)?;
        match bus.publish(event) {
            Ok(()) => {
                published += 1;
                debug!(index = i, published, "Published learn content event");
            }
            Err(e) => {
                let msg = format!("Failed to publish item {i}: {e}");
                errors.push(msg);
            }
        }
    }

    // Emit feed-complete event
    let complete_event = make_feed_complete_event(&feed_id, published)?;
    if let Err(e) = bus.publish(complete_event) {
        errors.push(format!("Failed to publish feed-complete: {e}"));
    }

    info!(
        feed_id = %feed_id,
        published,
        errors = errors.len(),
        "Feed complete"
    );

    Ok(FeedResult {
        feed_id,
        items_published: published,
        errors,
    })
}

/// Build a default content pool for testing and demos.
pub fn build_default_content_pool() -> Vec<String> {
    vec![
        "Rust is a systems programming language focused on safety and performance.".into(),
        "The borrow checker ensures memory safety without garbage collection.".into(),
        "Cargo is the Rust package manager and build system.".into(),
        "Traits provide polymorphism through composition rather than inheritance.".into(),
        "Error handling uses Result<T, E> with the ? operator for propagation.".into(),
        "Async Rust uses futures and the async/await syntax.".into(),
        "The ownership system prevents data races at compile time.".into(),
        "Pattern matching with match expressions is exhaustive by default.".into(),
    ]
}

/// Validate that content items meet minimum requirements.
pub fn validate_content(items: &[String]) -> Result<()> {
    if items.is_empty() {
        return Err(HiveError::Workload("Content pool is empty".into()));
    }
    let non_empty = items.iter().filter(|s| !s.trim().is_empty()).count();
    if non_empty == 0 {
        return Err(HiveError::Workload("All content items are empty".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_bus::LocalEventBus;

    #[test]
    fn feed_publishes_content() {
        let mut bus = LocalEventBus::new();
        let config = FeedConfig::new(
            "test",
            vec!["item 1".into(), "item 2".into(), "item 3".into()],
        );
        let result = run_feed(&mut bus, &config).unwrap();
        assert_eq!(result.items_published, 3);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn feed_skips_empty_items() {
        let mut bus = LocalEventBus::new();
        let config = FeedConfig::new(
            "test",
            vec!["item 1".into(), "".into(), "  ".into(), "item 2".into()],
        );
        let result = run_feed(&mut bus, &config).unwrap();
        assert_eq!(result.items_published, 2);
    }

    #[test]
    fn feed_uses_custom_feed_id() {
        let mut bus = LocalEventBus::new();
        let config = FeedConfig::new("test", vec!["item".into()]).with_feed_id("my-feed");
        let result = run_feed(&mut bus, &config).unwrap();
        assert_eq!(result.feed_id, "my-feed");
    }

    #[test]
    fn feed_generates_feed_id() {
        let mut bus = LocalEventBus::new();
        let config = FeedConfig::new("test", vec!["item".into()]);
        let result = run_feed(&mut bus, &config).unwrap();
        assert!(!result.feed_id.is_empty());
    }

    #[test]
    fn default_content_pool_not_empty() {
        let pool = build_default_content_pool();
        assert!(!pool.is_empty());
        assert!(pool.len() >= 5);
    }

    #[test]
    fn validate_content_rejects_empty() {
        assert!(validate_content(&[]).is_err());
        assert!(validate_content(&["".into(), "  ".into()]).is_err());
    }

    #[test]
    fn validate_content_accepts_valid() {
        assert!(validate_content(&["hello".into()]).is_ok());
    }

    #[test]
    fn feed_result_serde() {
        let result = FeedResult {
            feed_id: "f1".into(),
            items_published: 5,
            errors: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: FeedResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.items_published, 5);
    }
}
