//! Event text extraction and routing for Azure messaging sources.
//!
//! Extracts actionable text from Service Bus / Event Hubs payloads,
//! filters lifecycle events, and computes deterministic partition
//! assignments for agents.

use std::collections::HashMap;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Sentinel strings used as lifecycle signals.
pub const FEED_COMPLETE_PREFIX: &str = "__FEED_COMPLETE__:";
pub const ONLINE_CHECK: &str = "__ONLINE_CHECK__";
pub const STORE_FACT_BATCH: &str = "__STORE_FACT_BATCH__";

/// Event types that should be silently skipped (lifecycle / status events).
const SKIP_EVENT_TYPES: &[&str] = &["AGENT_READY", "QUERY_RESPONSE"];

// ---------------------------------------------------------------------------
// Event extraction
// ---------------------------------------------------------------------------

/// Extract actionable text from a bus/hub event payload.
///
/// Returns `None` for lifecycle events that should be skipped.
pub fn extract_text_from_event(
    event_type: Option<&str>,
    payload: &HashMap<String, Value>,
) -> Option<String> {
    if let Some(et) = event_type {
        if SKIP_EVENT_TYPES.contains(&et)
            || (et.starts_with("network_graph.") && et != "network_graph.search_query")
        {
            return None;
        }

        match et {
            "FEED_COMPLETE" => {
                let total = payload
                    .get("total")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                return Some(format!("{FEED_COMPLETE_PREFIX}{total}"));
            }
            "ONLINE_CHECK" => return Some(ONLINE_CHECK.to_string()),
            "STORE_FACT_BATCH" => return Some(STORE_FACT_BATCH.to_string()),
            "LEARN_CONTENT" => {
                return payload
                    .get("content")
                    .and_then(Value::as_str)
                    .map(String::from);
            }
            "QUERY" | "INPUT" | "network_graph.search_query" => {
                for key in &["question", "text", "content"] {
                    if let Some(val) = payload.get(*key).and_then(Value::as_str) {
                        return Some(val.to_string());
                    }
                }
                return None;
            }
            _ => {}
        }
    }

    // Generic fallback: search for first usable text field.
    for key in &["content", "text", "question", "message", "data"] {
        if let Some(val) = payload.get(*key).and_then(Value::as_str) {
            return Some(val.to_string());
        }
    }

    None
}

/// Check if a message should be delivered to this agent (target filtering).
pub fn is_targeted_to_agent(payload: &HashMap<String, Value>, agent_name: &str) -> bool {
    match payload.get("target_agent").and_then(Value::as_str) {
        Some(target) => target == agent_name,
        None => true, // No target = broadcast to all.
    }
}

/// Compute deterministic partition assignment for an agent.
///
/// Mirrors Python `stable_agent_index`: extracts numeric suffix from
/// "agent-N" names, falls back to hash-based index.
pub fn agent_partition(agent_name: &str, num_partitions: usize) -> usize {
    if num_partitions == 0 {
        return 0;
    }

    let idx = agent_name
        .rsplit(['-', '_'])
        .next()
        .and_then(|s| s.parse::<usize>().ok());

    match idx {
        Some(n) => n % num_partitions,
        None => {
            let hash: usize = agent_name.bytes().map(|b| b as usize).sum();
            hash % num_partitions
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- extract_text_from_event ----

    #[test]
    fn extract_skip_lifecycle() {
        let payload = HashMap::new();
        assert_eq!(extract_text_from_event(Some("AGENT_READY"), &payload), None);
        assert_eq!(
            extract_text_from_event(Some("QUERY_RESPONSE"), &payload),
            None
        );
    }

    #[test]
    fn extract_feed_complete() {
        let mut payload = HashMap::new();
        payload.insert("total".into(), Value::from(42));
        let result = extract_text_from_event(Some("FEED_COMPLETE"), &payload);
        assert_eq!(result, Some("__FEED_COMPLETE__:42".into()));
    }

    #[test]
    fn extract_online_check() {
        let result = extract_text_from_event(Some("ONLINE_CHECK"), &HashMap::new());
        assert_eq!(result, Some("__ONLINE_CHECK__".into()));
    }

    #[test]
    fn extract_learn_content() {
        let mut payload = HashMap::new();
        payload.insert("content".into(), Value::String("lesson text".into()));
        let result = extract_text_from_event(Some("LEARN_CONTENT"), &payload);
        assert_eq!(result, Some("lesson text".into()));
    }

    #[test]
    fn extract_query_event() {
        let mut payload = HashMap::new();
        payload.insert("question".into(), Value::String("What is X?".into()));
        let result = extract_text_from_event(Some("QUERY"), &payload);
        assert_eq!(result, Some("What is X?".into()));
    }

    #[test]
    fn extract_generic_fallback() {
        let mut payload = HashMap::new();
        payload.insert("text".into(), Value::String("some text".into()));
        let result = extract_text_from_event(None, &payload);
        assert_eq!(result, Some("some text".into()));
    }

    #[test]
    fn extract_no_usable_field() {
        let mut payload = HashMap::new();
        payload.insert("irrelevant".into(), Value::String("nope".into()));
        let result = extract_text_from_event(None, &payload);
        assert_eq!(result, None);
    }

    #[test]
    fn extract_network_graph_skip() {
        let payload = HashMap::new();
        assert_eq!(
            extract_text_from_event(Some("network_graph.update"), &payload),
            None
        );
    }

    #[test]
    fn extract_network_graph_search_query() {
        let mut payload = HashMap::new();
        payload.insert("question".into(), Value::String("search this".into()));
        let result = extract_text_from_event(Some("network_graph.search_query"), &payload);
        assert_eq!(result, Some("search this".into()));
    }

    // ---- is_targeted_to_agent ----

    #[test]
    fn targeted_matches() {
        let mut payload = HashMap::new();
        payload.insert("target_agent".into(), Value::String("agent-0".into()));
        assert!(is_targeted_to_agent(&payload, "agent-0"));
        assert!(!is_targeted_to_agent(&payload, "agent-1"));
    }

    #[test]
    fn no_target_is_broadcast() {
        let payload = HashMap::new();
        assert!(is_targeted_to_agent(&payload, "any-agent"));
    }

    // ---- agent_partition ----

    #[test]
    fn partition_numeric_suffix() {
        assert_eq!(agent_partition("agent-0", 4), 0);
        assert_eq!(agent_partition("agent-1", 4), 1);
        assert_eq!(agent_partition("agent-5", 4), 1);
        assert_eq!(agent_partition("agent_3", 4), 3);
    }

    #[test]
    fn partition_hash_fallback() {
        let p = agent_partition("my-agent", 4);
        assert!(p < 4);
    }

    #[test]
    fn partition_zero_partitions() {
        assert_eq!(agent_partition("agent-0", 0), 0);
    }
}
