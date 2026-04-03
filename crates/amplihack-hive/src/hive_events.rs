//! Hive topic constants and event factory functions.
//!
//! Matches Python `amplihack/workloads/hive/events.py`:
//! - Named topic constants for Service Bus / local event routing
//! - Factory functions for creating typed HiveEvent payloads

use crate::models::BusEvent;
use crate::workload::HiveEvent;
use crate::error::Result;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Topic constants
// ---------------------------------------------------------------------------

pub const HIVE_LEARN_CONTENT: &str = "hive.learn.content";
pub const HIVE_FEED_COMPLETE: &str = "hive.feed.complete";
pub const HIVE_AGENT_READY: &str = "hive.agent.ready";
pub const HIVE_QUERY: &str = "hive.query";
pub const HIVE_QUERY_RESPONSE: &str = "hive.query.response";

/// All hive topics for subscription.
pub const ALL_HIVE_TOPICS: &[&str] = &[
    HIVE_LEARN_CONTENT,
    HIVE_FEED_COMPLETE,
    HIVE_AGENT_READY,
    HIVE_QUERY,
    HIVE_QUERY_RESPONSE,
];

// ---------------------------------------------------------------------------
// Factory functions
// ---------------------------------------------------------------------------

/// Create a learn-content event with the given source and content.
pub fn make_learn_content_event(source: &str, content: &str) -> Result<BusEvent> {
    let event = HiveEvent::LearnContent {
        content: content.to_string(),
        source: source.to_string(),
    };
    Ok(BusEvent {
        event_id: Uuid::new_v4().to_string(),
        source_id: Uuid::new_v4().to_string(),
        topic: HIVE_LEARN_CONTENT.to_string(),
        payload: serde_json::to_value(&event)?,
        timestamp: chrono::Utc::now(),
    })
}

/// Create a feed-complete event.
pub fn make_feed_complete_event(feed_id: &str, items: u32) -> Result<BusEvent> {
    let event = HiveEvent::FeedComplete {
        feed_id: feed_id.to_string(),
        items,
    };
    Ok(BusEvent {
        event_id: Uuid::new_v4().to_string(),
        source_id: Uuid::new_v4().to_string(),
        topic: HIVE_FEED_COMPLETE.to_string(),
        payload: serde_json::to_value(&event)?,
        timestamp: chrono::Utc::now(),
    })
}

/// Create an agent-ready event.
pub fn make_agent_ready_event(agent_id: &str) -> Result<BusEvent> {
    let event = HiveEvent::AgentReady {
        agent_id: agent_id.to_string(),
    };
    Ok(BusEvent {
        event_id: Uuid::new_v4().to_string(),
        source_id: Uuid::new_v4().to_string(),
        topic: HIVE_AGENT_READY.to_string(),
        payload: serde_json::to_value(&event)?,
        timestamp: chrono::Utc::now(),
    })
}

/// Create a query event with auto-generated query ID.
pub fn make_query_event(question: &str) -> Result<(String, BusEvent)> {
    let query_id = Uuid::new_v4().to_string();
    let event = HiveEvent::Query {
        query_id: query_id.clone(),
        question: question.to_string(),
    };
    let bus_event = BusEvent {
        event_id: Uuid::new_v4().to_string(),
        source_id: Uuid::new_v4().to_string(),
        topic: HIVE_QUERY.to_string(),
        payload: serde_json::to_value(&event)?,
        timestamp: chrono::Utc::now(),
    };
    Ok((query_id, bus_event))
}

/// Create a query-response event.
pub fn make_query_response_event(query_id: &str, answer: &str, confidence: f64) -> Result<BusEvent> {
    if !(0.0..=1.0).contains(&confidence) {
        return Err(crate::error::HiveError::InvalidConfidence(confidence));
    }
    let event = HiveEvent::QueryResponse {
        query_id: query_id.to_string(),
        answer: answer.to_string(),
        confidence,
    };
    Ok(BusEvent {
        event_id: Uuid::new_v4().to_string(),
        source_id: Uuid::new_v4().to_string(),
        topic: HIVE_QUERY_RESPONSE.to_string(),
        payload: serde_json::to_value(&event)?,
        timestamp: chrono::Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_constants() {
        assert_eq!(ALL_HIVE_TOPICS.len(), 5);
        assert!(ALL_HIVE_TOPICS.contains(&HIVE_LEARN_CONTENT));
        assert!(ALL_HIVE_TOPICS.contains(&HIVE_QUERY_RESPONSE));
    }

    #[test]
    fn make_learn_content_event_has_correct_topic() {
        let event = make_learn_content_event("test-source", "hello world").unwrap();
        assert_eq!(event.topic, HIVE_LEARN_CONTENT);
        assert!(!event.source_id.is_empty());
        assert!(!event.event_id.is_empty());
    }

    #[test]
    fn make_feed_complete_event_has_correct_topic() {
        let event = make_feed_complete_event("feed-1", 10).unwrap();
        assert_eq!(event.topic, HIVE_FEED_COMPLETE);
        let payload: HiveEvent = serde_json::from_value(event.payload).unwrap();
        let HiveEvent::FeedComplete { items, .. } = payload else {
            unreachable!("Expected FeedComplete, got {payload:?}");
        };
        assert_eq!(items, 10);
    }

    #[test]
    fn make_agent_ready_event_has_correct_topic() {
        let event = make_agent_ready_event("agent-42").unwrap();
        assert_eq!(event.topic, HIVE_AGENT_READY);
    }

    #[test]
    fn make_query_event_returns_query_id() {
        let (query_id, event) = make_query_event("What is Rust?").unwrap();
        assert_eq!(event.topic, HIVE_QUERY);
        assert!(!query_id.is_empty());
        let payload: HiveEvent = serde_json::from_value(event.payload).unwrap();
        let HiveEvent::Query {
            query_id: qid,
            question,
        } = payload
        else {
            unreachable!("Expected Query, got {payload:?}");
        };
        assert_eq!(qid, query_id);
        assert_eq!(question, "What is Rust?");
    }

    #[test]
    fn make_query_response_event_has_confidence() {
        let event = make_query_response_event("q1", "A language", 0.85).unwrap();
        assert_eq!(event.topic, HIVE_QUERY_RESPONSE);
        let payload: HiveEvent = serde_json::from_value(event.payload).unwrap();
        let HiveEvent::QueryResponse { confidence, .. } = payload else {
            unreachable!("Expected QueryResponse, got {payload:?}");
        };
        assert!((confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn events_have_unique_ids() {
        let e1 = make_learn_content_event("s", "c").unwrap();
        let e2 = make_learn_content_event("s", "c").unwrap();
        assert_ne!(e1.event_id, e2.event_id);
    }
}
