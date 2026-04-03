use amplihack_hive::{BusEvent, EventBus, LocalEventBus, MAX_MAILBOX_SIZE, make_event};

fn ev(topic: &str, source: &str) -> BusEvent {
    make_event(topic, source, serde_json::json!({"key": "value"}))
}

// --- publish tests ---

#[test]
fn publish_single_event() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].source_id, "agent-1");
}

#[test]
fn publish_multiple_events() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    bus.publish(ev("topic-a", "agent-2")).unwrap();
    bus.publish(ev("topic-b", "agent-3")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 3);
}

#[test]
fn publish_preserves_event_data() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    let event = make_event("metrics", "monitor-1", serde_json::json!({"cpu": 42, "mem": 1024}));
    bus.publish(event).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload, serde_json::json!({"cpu": 42, "mem": 1024}));
    assert_eq!(events[0].source_id, "monitor-1");
}

// --- subscribe tests ---

#[test]
fn subscribe_receives_events() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn subscribe_with_filter() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", Some(&["topic-a"])).unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    bus.publish(ev("topic-b", "agent-2")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].topic, "topic-a");
}

#[test]
fn subscribe_multiple_agents() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    bus.subscribe("handler-2", None).unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    let e1 = bus.pending_events("handler-1").unwrap();
    let e2 = bus.pending_events("handler-2").unwrap();
    assert_eq!(e1.len(), 1);
    assert_eq!(e2.len(), 1);
}

// --- unsubscribe tests ---

#[test]
fn unsubscribe_handler() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    bus.unsubscribe("handler-1").unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert!(events.is_empty());
}

#[test]
fn unsubscribe_nonexistent_handler() {
    let mut bus = LocalEventBus::new();
    bus.unsubscribe("no-such-handler").unwrap();
}

// --- pending_events tests ---

#[test]
fn pending_events_after_publish() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    let events = bus.pending_events("handler-1").unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn pending_events_empty() {
    let bus = LocalEventBus::new();
    let events = bus.pending_events("handler-1").unwrap();
    assert!(events.is_empty());
}

// --- poll tests ---

#[test]
fn poll_drains_queue() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    let events = bus.poll("handler-1").unwrap();
    assert_eq!(events.len(), 1);
    let remaining = bus.pending_events("handler-1").unwrap();
    assert!(remaining.is_empty());
}

#[test]
fn poll_multiple_topics() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    bus.publish(ev("topic-b", "agent-2")).unwrap();
    let events = bus.poll("handler-1").unwrap();
    assert_eq!(events.len(), 2);
}

// --- close tests ---

#[test]
fn close_clears_all_state() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    bus.publish(ev("topic-a", "agent-1")).unwrap();
    bus.close().unwrap();
    assert!(bus.pending_events("handler-1").unwrap().is_empty());
}

// --- event ordering ---

#[test]
fn event_ordering_preserved() {
    let mut bus = LocalEventBus::new();
    bus.subscribe("handler-1", None).unwrap();
    for i in 0..5 {
        bus.publish(ev("topic-a", &format!("agent-{i}"))).unwrap();
    }
    let events = bus.poll("handler-1").unwrap();
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

#[test]
fn max_mailbox_size_is_positive() {
    const { assert!(MAX_MAILBOX_SIZE > 0) };
}
