//! Session discovery adapter.
//!
//! Matches Python `amplihack/memory/discoveries.py`:
//! - Store and retrieve session discoveries
//! - Wraps MemoryCoordinator with discovery-specific metadata

use crate::coordinator::MemoryCoordinator;
use crate::models::{MemoryQuery, MemoryType, StorageRequest};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

/// A discovery record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discovery {
    pub content: String,
    pub category: String,
    pub date: String,
    pub summary: String,
}

/// Store a discovery through the memory coordinator.
pub fn store_discovery(
    coordinator: &mut MemoryCoordinator,
    content: &str,
    category: &str,
    date: Option<&str>,
    summary: Option<&str>,
    session_id: &str,
) -> Option<String> {
    let now = chrono_date();
    let mut request = StorageRequest::new(content, MemoryType::Semantic, session_id);
    request.metadata.insert("source".into(), json!("discovery"));
    request
        .metadata
        .insert("category".into(), json!(category));
    request
        .metadata
        .insert("timestamp".into(), json!(date.unwrap_or(&now)));
    if let Some(s) = summary {
        request.metadata.insert("summary".into(), json!(s));
    }
    coordinator.store(request)
}

/// Retrieve recent discoveries from the coordinator.
pub fn get_recent_discoveries(
    coordinator: &mut MemoryCoordinator,
    days: u32,
    limit: usize,
    session_id: Option<&str>,
) -> Vec<Discovery> {
    let mut query = MemoryQuery::new("discovery learning pattern solution")
        .with_budget(4000)
        .with_types(vec![MemoryType::Semantic]);
    query.limit = limit;
    if let Some(sid) = session_id {
        query = query.with_session(sid);
    }
    if days > 0 {
        query.time_range_secs = Some(days as f64 * 86400.0);
    }

    coordinator
        .retrieve(&query)
        .into_iter()
        .filter(|e| {
            e.metadata
                .get("source")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == "discovery")
        })
        .map(|e| Discovery {
            content: e.content,
            category: e
                .metadata
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("general")
                .to_string(),
            date: e
                .metadata
                .get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            summary: e
                .metadata
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect()
}

fn chrono_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple ISO date (no chrono dependency)
    let days = secs / 86400;
    let years = 1970 + days / 365;
    format!("{years}-01-01") // Approximate; sufficient for metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MemoryConfig;

    fn test_coord() -> MemoryCoordinator {
        MemoryCoordinator::new(MemoryConfig::for_testing())
    }

    #[test]
    fn store_and_retrieve_discovery() {
        let mut coord = test_coord();
        let id = store_discovery(
            &mut coord,
            "Learned that Rust macros can reduce boilerplate significantly",
            "coding",
            Some("2024-01-15"),
            Some("Macros reduce boilerplate"),
            "s1",
        );
        assert!(id.is_some());

        let discoveries = get_recent_discoveries(&mut coord, 365, 10, Some("s1"));
        assert_eq!(discoveries.len(), 1);
        assert_eq!(discoveries[0].category, "coding");
        assert!(discoveries[0].content.contains("Rust macros"));
    }

    #[test]
    fn empty_session_returns_nothing() {
        let mut coord = test_coord();
        let discoveries = get_recent_discoveries(&mut coord, 30, 10, Some("nonexistent"));
        assert!(discoveries.is_empty());
    }

    #[test]
    fn discovery_metadata_preserved() {
        let mut coord = test_coord();
        store_discovery(
            &mut coord,
            "Important architectural decision about using traits",
            "architecture",
            Some("2024-06-01"),
            Some("Traits for abstraction"),
            "s1",
        );
        let discoveries = get_recent_discoveries(&mut coord, 365, 10, None);
        assert_eq!(discoveries[0].date, "2024-06-01");
        assert_eq!(discoveries[0].summary, "Traits for abstraction");
    }
}
