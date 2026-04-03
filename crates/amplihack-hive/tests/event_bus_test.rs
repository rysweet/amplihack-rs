use amplihack_hive::{BusEvent, EventBus, LocalEventBus};
use chrono::Utc;

fn make_event(topic: &str, source: &str) -> BusEvent {
    BusEvent {
        topic: topic.to_string(),
        payload: serde_json::json!({"key": "value"}),
        source_id: source.to_string(),
        timestamp: Utc::now(),
    }
}

// --- publish tests ---

#[test]
fn publish_single_event() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("topic-a", "handler-1").unwrap();
    bus.publish(make_event("topic-a", "agent-1")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].source_id, "agent-1");
}

#[test]
fn publish_multiple_events() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("topic-a", "handler-1").unwrap();
    bus.subscribe("topic-b", "handler-1").unwrap();
    bus.publish(make_event("topic-a", "agent-1")).unwrap();
    bus.publish(make_event("topic-a", "agent-2")).unwrap();
    bus.publish(make_event("topic-b", "agent-1")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 3);
}

#[test]
fn publish_preserves_event_data() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("metrics", "handler-1").unwrap();
    let event = BusEvent {
        topic: "metrics".to_string(),
        payload: serde_json::json!({"cpu": 42, "mem": 1024}),
        source_id: "monitor-1".to_string(),
        timestamp: Utc::now(),
    };
    bus.publish(event.clone()).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].payload,
        serde_json::json!({"cpu": 42, "mem": 1024})
    );
    assert_eq!(events[0].source_id, "monitor-1");
}

// --- subscribe tests ---

#[test]
fn subscribe_to_topic() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("topic-a", "handler-1").unwrap();
    bus.publish(make_event("topic-a", "agent-1")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn subscribe_multiple_handlers() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("topic-a", "handler-1").unwrap();
    bus.subscribe("topic-a", "handler-2").unwrap();
    bus.publish(make_event("topic-a", "agent-1")).unwrap();
    let events1 = bus.pending_events("handler-1").unwrap();
    let events2 = bus.pending_events("handler-2").unwrap();
    assert_eq!(events1.len(), 1);
    assert_eq!(events2.len(), 1);
}

// --- unsubscribe tests ---

#[test]
fn unsubscribe_handler() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("topic-a", "handler-1").unwrap();
    bus.unsubscribe("topic-a", "handler-1").unwrap();
    bus.publish(make_event("topic-a", "agent-1")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert!(events.is_empty());
}

#[test]
fn unsubscribe_nonexistent_handler() {
    let mut bus = LocalEventBus::new();
    // Should succeed as a no-op
    bus.unsubscribe("topic-x", "no-such-handler").unwrap();
}

// --- pending_events tests ---

#[test]
fn pending_events_after_publish() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("topic-a", "handler-1").unwrap();
    bus.publish(make_event("topic-a", "agent-1")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn pending_events_empty() {
    let bus = LocalEventBus::new();
    let events = bus.pending_events("handler-1").unwrap();
    assert!(events.is_empty());
}

// --- drain_events tests ---

#[test]
fn drain_events_clears_queue() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("topic-a", "handler-1").unwrap();
    bus.publish(make_event("topic-a", "agent-1")).unwrap();
    let events = bus.drain_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
    // Queue should be empty after drain
    let remaining = bus.pending_events("handler-1").unwrap();
    assert!(remaining.is_empty());
}

#[test]
fn drain_events_multiple_topics() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("topic-a", "handler-1").unwrap();
    bus.subscribe("topic-b", "handler-1").unwrap();
    bus.publish(make_event("topic-a", "agent-1")).unwrap();
    bus.publish(make_event("topic-b", "agent-2")).unwrap();
    let events = bus.drain_events("handler-1").unwrap();
    assert_eq!(events.len(), 2);
}

#[test]
fn event_ordering_preserved() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("topic-a", "handler-1").unwrap();
    for i in 0..5 {
        let event = BusEvent {
            topic: "topic-a".to_string(),
            payload: serde_json::json!({"seq": i}),
            source_id: format!("agent-{i}"),
            timestamp: Utc::now(),
        };
        bus.publish(event).unwrap();
    }
    let events = bus.drain_events("handler-1").unwrap();
    assert_eq!(events.len(), 5);
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.source_id, format!("agent-{i}"));
    }
}

// --- constructor tests ---

#[test]
fn new_local_event_bus_is_constructible() {
    let _bus = LocalEventBus::new();
}

#[test]
fn local_event_bus_default_is_constructible() {
    let _bus: LocalEventBus = Default::default();
}
