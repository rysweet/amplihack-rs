use amplihack_hive::{
    AgentSpec, DEFAULT_CONTRADICTION_OVERLAP, EventBusConfig, GatewayConfig, GraphStoreConfig,
    HiveController, HiveFact, HiveManifest, InMemoryGateway, InMemoryGraphStore,
};
use chrono::Utc;
use std::collections::HashMap;

fn make_agent(name: &str, role: &str, replicas: u32) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        role: role.to_string(),
        replicas,
        memory_config: None,
        domain: String::new(),
    }
}

fn make_manifest(agents: Vec<AgentSpec>) -> HiveManifest {
    HiveManifest {
        agents,
        graph_config: serde_json::json!({"backend": "memory"}),
        event_bus_config: serde_json::json!({"type": "local"}),
        gateway_config: serde_json::json!({"port": 8080}),
        graph_store: GraphStoreConfig::default(),
        event_bus: EventBusConfig::default(),
        gateway: GatewayConfig::default(),
    }
}

// --- accessor tests ---

#[test]
fn new_controller_has_idle_status() {
    let controller = HiveController::new();
    let state = controller.status();
    assert_eq!(state.graph_status, "idle");
    assert_eq!(state.bus_status, "idle");
    assert!(state.running_agents.is_empty());
    assert!(!state.hive_store_connected);
    assert!(!state.event_bus_connected);
}

#[test]
fn new_controller_has_no_desired_manifest() {
    let controller = HiveController::new();
    assert!(controller.desired_manifest().is_none());
}

#[test]
fn status_returns_current_state() {
    let controller: HiveController = Default::default();
    let state = controller.status();
    assert_eq!(state.graph_status, "idle");
    assert_eq!(state.bus_status, "idle");
}

#[test]
fn controller_default_is_constructible() {
    let _controller: HiveController = Default::default();
}

// --- apply_manifest tests ---

#[test]
fn apply_manifest_creates_agents() {
    let mut ctrl = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learn", 2)]);
    let actions = ctrl.apply_manifest(manifest).unwrap();
    assert!(actions.iter().any(|a| a.contains("create learner")));
    assert_eq!(ctrl.status().running_agents.len(), 1);
    assert!(ctrl.status().hive_store_connected);
    assert!(ctrl.status().event_bus_connected);
}

#[test]
fn apply_manifest_with_multiple_agents() {
    let mut ctrl = HiveController::new();
    let manifest = make_manifest(vec![
        make_agent("learner", "learn", 2),
        make_agent("retriever", "retrieve", 3),
    ]);
    let actions = ctrl.apply_manifest(manifest).unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(ctrl.status().running_agents.len(), 2);
}

#[test]
fn apply_manifest_removes_stale_agents() {
    let mut ctrl = HiveController::new();
    let m1 = make_manifest(vec![make_agent("a", "r", 1), make_agent("b", "r", 1)]);
    ctrl.apply_manifest(m1).unwrap();
    let m2 = make_manifest(vec![make_agent("a", "r", 1)]);
    let actions = ctrl.apply_manifest(m2).unwrap();
    assert!(actions.iter().any(|a| a.contains("remove b")));
}

#[test]
fn apply_manifest_with_domain() {
    let mut ctrl = HiveController::new();
    let agent = AgentSpec {
        name: "domain-agent".into(),
        role: "worker".into(),
        replicas: 1,
        memory_config: None,
        domain: "science".into(),
    };
    let manifest = make_manifest(vec![agent]);
    ctrl.apply_manifest(manifest).unwrap();
    assert_eq!(ctrl.status().running_agents[0].domain, "science");
}

// --- learn + promote + query tests ---

#[test]
fn learn_stores_and_promotes() {
    let mut ctrl = HiveController::new();
    ctrl.apply_manifest(make_manifest(vec![make_agent("a1", "r", 1)]))
        .unwrap();
    let result = ctrl.learn("a1", "rust", "systems language", 0.9).unwrap();
    assert!(!result.fact_id.is_empty());
    assert!(result.promoted);
}

#[test]
fn learn_unknown_agent_errors() {
    let mut ctrl = HiveController::new();
    assert!(ctrl.learn("ghost", "c", "content", 0.5).is_err());
}

#[test]
fn promote_fact_works() {
    let mut ctrl = HiveController::new();
    ctrl.apply_manifest(make_manifest(vec![make_agent("a1", "r", 1)]))
        .unwrap();
    let result = ctrl.learn("a1", "rust", "systems", 0.9).unwrap();
    let promoted = ctrl.promote_fact("a1", &result.fact_id).unwrap();
    assert!(promoted);
}

#[test]
fn query_agent_returns_facts() {
    let mut ctrl = HiveController::new();
    ctrl.apply_manifest(make_manifest(vec![make_agent("a1", "r", 1)]))
        .unwrap();
    ctrl.learn("a1", "rust", "systems language", 0.9).unwrap();
    let facts = ctrl.query_agent("a1", "rust").unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
}

#[test]
fn query_routed_aggregates() {
    let mut ctrl = HiveController::new();
    ctrl.apply_manifest(make_manifest(vec![
        make_agent("a1", "r", 1),
        make_agent("a2", "r", 1),
    ]))
    .unwrap();
    ctrl.learn("a1", "rust", "systems language", 0.9).unwrap();
    ctrl.learn("a2", "rust", "memory safe", 0.85).unwrap();
    let facts = ctrl.query_routed("rust", 10).unwrap();
    assert_eq!(facts.len(), 2);
}

// --- gateway tests ---

#[test]
fn gateway_default_passes_all() {
    let ctrl = HiveController::new();
    assert!(ctrl.gateway().passes_trust(0.0));
    assert!(ctrl.gateway().passes_trust(1.0));
}

#[test]
fn gateway_from_manifest_respects_threshold() {
    let mut manifest = make_manifest(vec![make_agent("a", "r", 1)]);
    manifest.gateway.trust_threshold = 0.5;
    let ctrl = HiveController::from_manifest(manifest);
    assert!(!ctrl.gateway().passes_trust(0.3));
    assert!(ctrl.gateway().passes_trust(0.5));
}

// --- propagate tests ---

#[test]
fn propagate_distributes_fact() {
    let mut ctrl = HiveController::new();
    ctrl.apply_manifest(make_manifest(vec![
        make_agent("a1", "r", 1),
        make_agent("a2", "r", 1),
    ]))
    .unwrap();
    ctrl.propagate("a1", "rust", "systems", 0.9).unwrap();
}

// --- shutdown tests ---

#[test]
fn shutdown_clears_state() {
    let mut ctrl = HiveController::new();
    ctrl.apply_manifest(make_manifest(vec![make_agent("a1", "r", 1)]))
        .unwrap();
    ctrl.shutdown().unwrap();
    assert!(ctrl.status().running_agents.is_empty());
    assert_eq!(ctrl.status().graph_status, "stopped");
    assert_eq!(ctrl.status().bus_status, "stopped");
    assert!(!ctrl.status().hive_store_connected);
}

// --- apply + reconcile tests ---

#[test]
fn apply_sets_desired_only() {
    let mut ctrl = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learn", 2)]);
    ctrl.apply(manifest.clone()).unwrap();
    assert!(ctrl.desired_manifest().is_some());
    assert!(ctrl.status().running_agents.is_empty());
}

#[test]
fn reconcile_no_manifest() {
    let mut ctrl = HiveController::new();
    let actions = ctrl.reconcile().unwrap();
    assert!(actions.is_empty());
}

#[test]
fn reconcile_with_manifest() {
    let mut ctrl = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learn", 2)]);
    ctrl.apply(manifest).unwrap();
    let actions = ctrl.reconcile().unwrap();
    assert!(!actions.is_empty());
    assert_eq!(ctrl.status().running_agents.len(), 1);
}

// --- scale_agent tests ---

#[test]
fn scale_agent_up() {
    let mut ctrl = HiveController::new();
    ctrl.apply(make_manifest(vec![make_agent("learner", "learn", 2)]))
        .unwrap();
    ctrl.reconcile().unwrap();
    ctrl.scale_agent("learner", 5).unwrap();
    assert_eq!(ctrl.status().running_agents[0].replicas, 5);
}

#[test]
fn scale_agent_nonexistent() {
    let mut ctrl = HiveController::new();
    assert!(ctrl.scale_agent("no-such", 3).is_err());
}

// --- remove_agent tests ---

#[test]
fn remove_agent_existing() {
    let mut ctrl = HiveController::new();
    ctrl.apply(make_manifest(vec![make_agent("learner", "learn", 2)]))
        .unwrap();
    let removed = ctrl.remove_agent("learner").unwrap();
    assert!(removed);
}

#[test]
fn remove_agent_nonexistent() {
    let mut ctrl = HiveController::new();
    let removed = ctrl.remove_agent("ghost").unwrap();
    assert!(!removed);
}

// --- InMemoryGraphStore tests ---

#[test]
fn graph_store_insert_and_get() {
    let mut store = InMemoryGraphStore::new();
    let fact = HiveFact {
        fact_id: "f1".into(),
        concept: "rust".into(),
        content: "systems".into(),
        confidence: 0.9,
        source_id: "a".into(),
        tags: vec![],
        created_at: Utc::now(),
        status: "promoted".into(),
        metadata: HashMap::new(),
    };
    store.insert(fact);
    assert_eq!(store.len(), 1);
    assert!(!store.is_empty());
    assert!(store.get("f1").is_some());
}

#[test]
fn graph_store_query() {
    let mut store = InMemoryGraphStore::new();
    let fact = HiveFact {
        fact_id: "f1".into(),
        concept: "rust".into(),
        content: "systems".into(),
        confidence: 0.9,
        source_id: "a".into(),
        tags: vec![],
        created_at: Utc::now(),
        status: "promoted".into(),
        metadata: HashMap::new(),
    };
    store.insert(fact);
    let results = store.query("rust", 0.5);
    assert_eq!(results.len(), 1);
    let results = store.query("rust", 0.95);
    assert!(results.is_empty());
}

#[test]
fn graph_store_default() {
    let store = InMemoryGraphStore::default();
    assert!(store.is_empty());
}

// --- InMemoryGateway tests ---

#[test]
fn gateway_passes_trust_threshold() {
    let gw = InMemoryGateway::new(0.5, DEFAULT_CONTRADICTION_OVERLAP);
    assert!(gw.passes_trust(0.5));
    assert!(gw.passes_trust(1.0));
    assert!(!gw.passes_trust(0.3));
}

#[test]
fn gateway_contradiction_detection() {
    let gw = InMemoryGateway::default();
    assert!(gw.is_contradiction("the cat sat on the mat", "the cat sat on the floor"));
    assert!(!gw.is_contradiction("hello world", "goodbye moon"));
}

#[test]
fn gateway_default_values() {
    let gw = InMemoryGateway::default();
    assert!(gw.passes_trust(0.0));
}
