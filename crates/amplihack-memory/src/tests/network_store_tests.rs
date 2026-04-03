use super::*;
use crate::memory_store::InMemoryGraphStore;
use crate::network_store_types::merge_results;
use serde_json::json;

fn make_store(agent_id: &str) -> NetworkGraphStore {
    let local = Box::new(InMemoryGraphStore::new());
    let transport = Box::new(LocalTransport::new());
    NetworkGraphStore::new(agent_id, local, transport, true).unwrap()
}

#[test]
fn create_and_get_node() {
    let mut store = make_store("agent-1");
    let props: Props = [("content".into(), json!("hello"))].into_iter().collect();
    let id = store.create_node("semantic_memory", &props).unwrap();
    assert!(!id.is_empty());
    let found = store.get_node("semantic_memory", &id).unwrap();
    assert!(found.is_some());
}

#[test]
fn search_returns_local_results() {
    let mut store = make_store("agent-1");
    let props: Props = [
        ("content".into(), json!("quantum computing")),
        ("concept".into(), json!("physics")),
    ]
    .into_iter()
    .collect();
    store.create_node("semantic_memory", &props).unwrap();

    let results = store
        .search_nodes("semantic_memory", "quantum", None, 10)
        .unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn create_and_get_edge() {
    let mut store = make_store("agent-1");
    let p1: Props = [("content".into(), json!("a"))].into_iter().collect();
    let p2: Props = [("content".into(), json!("b"))].into_iter().collect();
    let id1 = store.create_node("t", &p1).unwrap();
    let id2 = store.create_node("t", &p2).unwrap();
    store
        .create_edge("RELATED", "t", &id1, "t", &id2, &Props::new())
        .unwrap();
    let edges = store
        .get_edges(&id1, Some("RELATED"), EdgeDirection::Outgoing)
        .unwrap();
    assert_eq!(edges.len(), 1);
}

#[test]
fn agent_registry() {
    let reg = AgentRegistry::new();
    reg.register("a1", HashMap::new());
    reg.register("a2", HashMap::new());
    assert_eq!(reg.list_agents().len(), 2);
    reg.unregister("a1");
    assert_eq!(reg.list_agents().len(), 1);
    assert!(reg.get("a2").is_some());
    assert!(reg.get("a1").is_none());
}

#[test]
fn local_transport_pub_sub() {
    let transport = LocalTransport::new();
    transport.subscribe("a1").unwrap();
    transport.subscribe("a2").unwrap();

    let event = BusEvent {
        event_type: "test".into(),
        source_agent: "a1".into(),
        payload: HashMap::new(),
    };
    transport.publish(&event).unwrap();

    // a1 shouldn't see its own events
    let a1_events = transport.poll("a1").unwrap();
    assert!(a1_events.is_empty());

    // a2 should see a1's event
    let a2_events = transport.poll("a2").unwrap();
    assert_eq!(a2_events.len(), 1);

    // Second poll is empty (drained)
    let a2_events = transport.poll("a2").unwrap();
    assert!(a2_events.is_empty());
}

#[test]
fn merge_results_dedup() {
    let local = vec![
        [("node_id".into(), json!("1")), ("data".into(), json!("a"))]
            .into_iter()
            .collect(),
        [("node_id".into(), json!("2")), ("data".into(), json!("b"))]
            .into_iter()
            .collect(),
    ];
    let remote = vec![
        [("node_id".into(), json!("2")), ("data".into(), json!("b"))]
            .into_iter()
            .collect(),
        [("node_id".into(), json!("3")), ("data".into(), json!("c"))]
            .into_iter()
            .collect(),
    ];
    let merged = merge_results(&local, &remote, 10);
    assert_eq!(merged.len(), 3);
}

#[test]
fn merge_results_limit() {
    let local: Vec<Props> = (0..5)
        .map(|i| {
            [("node_id".into(), json!(format!("n{i}")))]
                .into_iter()
                .collect()
        })
        .collect();
    let merged = merge_results(&local, &[], 3);
    assert_eq!(merged.len(), 3);
}

#[test]
fn process_events_empty() {
    let mut store = make_store("agent-1");
    let count = store.process_events().unwrap();
    assert_eq!(count, 0);
}

#[test]
fn close_unregisters() {
    let registry = AgentRegistry::new();
    let local = Box::new(InMemoryGraphStore::new());
    let transport = Box::new(LocalTransport::new());
    let mut store = NetworkGraphStore::new("agent-1", local, transport, true)
        .unwrap()
        .with_registry(registry.clone());
    assert!(registry.list_agents().contains(&"agent-1".to_string()));
    store.close().unwrap();
    assert!(!registry.list_agents().contains(&"agent-1".to_string()));
}

#[test]
fn bus_event_serialization() {
    let event = BusEvent {
        event_type: "test".into(),
        source_agent: "a1".into(),
        payload: [("key".into(), json!("value"))].into_iter().collect(),
    };
    let json_str = serde_json::to_string(&event).unwrap();
    let back: BusEvent = serde_json::from_str(&json_str).unwrap();
    assert_eq!(back.event_type, "test");
    assert_eq!(back.source_agent, "a1");
}
