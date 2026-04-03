use amplihack_hive::{AgentSpec, HiveController, HiveManifest};

fn make_agent(name: &str, role: &str, replicas: u32) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        role: role.to_string(),
        replicas,
        memory_config: None,
    }
}

fn make_manifest(agents: Vec<AgentSpec>) -> HiveManifest {
    HiveManifest {
        agents,
        graph_config: serde_json::json!({"backend": "memory"}),
        event_bus_config: serde_json::json!({"type": "local"}),
        gateway_config: serde_json::json!({"port": 8080}),
    }
}

// --- accessor tests (REAL implementations, should pass) ---

#[test]
fn new_controller_has_idle_status() {
    let controller = HiveController::new();
    let state = controller.status();
    assert_eq!(state.graph_status, "idle");
    assert_eq!(state.bus_status, "idle");
    assert!(state.running_agents.is_empty());
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

// --- apply tests ---

#[test]
fn apply_manifest_sets_desired() {
    let mut controller = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learner", 2)]);
    controller.apply(manifest.clone()).unwrap();
    assert!(controller.desired_manifest().is_some());
    assert_eq!(controller.desired_manifest().unwrap().agents.len(), 1);
    assert_eq!(controller.status().running_agents.len(), 1);
}

#[test]
fn apply_manifest_with_multiple_agents() {
    let mut controller = HiveController::new();
    let manifest = make_manifest(vec![
        make_agent("learner", "learner", 2),
        make_agent("retriever", "retriever", 3),
        make_agent("gateway", "gateway", 1),
    ]);
    controller.apply(manifest).unwrap();
    assert_eq!(controller.desired_manifest().unwrap().agents.len(), 3);
    assert_eq!(controller.status().running_agents.len(), 3);
}

#[test]
fn apply_manifest_with_memory_config() {
    let mut controller = HiveController::new();
    let agent = AgentSpec {
        name: "learner".to_string(),
        role: "learner".to_string(),
        replicas: 1,
        memory_config: Some("persistent".to_string()),
    };
    let manifest = make_manifest(vec![agent]);
    controller.apply(manifest).unwrap();
    let agents = &controller.desired_manifest().unwrap().agents;
    assert_eq!(agents[0].memory_config, Some("persistent".to_string()));
}

// --- reconcile tests ---

#[test]
fn reconcile_no_manifest() {
    let mut controller = HiveController::new();
    let actions = controller.reconcile().unwrap();
    assert!(actions.is_empty());
}

#[test]
fn reconcile_with_manifest() {
    let mut controller = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learner", 2)]);
    controller.apply(manifest).unwrap();
    // After apply, current matches desired, so reconcile should find no drift
    let actions = controller.reconcile().unwrap();
    assert!(actions.is_empty());
}

// --- scale_agent tests ---

#[test]
fn scale_agent_up() {
    let mut controller = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learner", 2)]);
    controller.apply(manifest).unwrap();
    controller.scale_agent("learner", 5).unwrap();
    let agents = &controller.status().running_agents;
    assert_eq!(agents[0].replicas, 5);
}

#[test]
fn scale_agent_down() {
    let mut controller = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learner", 5)]);
    controller.apply(manifest).unwrap();
    controller.scale_agent("learner", 1).unwrap();
    let agents = &controller.status().running_agents;
    assert_eq!(agents[0].replicas, 1);
}

#[test]
fn scale_agent_nonexistent() {
    let mut controller = HiveController::new();
    let result = controller.scale_agent("no-such-agent", 3);
    assert!(result.is_err());
}

// --- remove_agent tests ---

#[test]
fn remove_agent_existing() {
    let mut controller = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learner", 2)]);
    controller.apply(manifest).unwrap();
    let removed = controller.remove_agent("learner").unwrap();
    assert!(removed);
    assert!(controller.status().running_agents.is_empty());
}

#[test]
fn remove_agent_nonexistent() {
    let mut controller = HiveController::new();
    let removed = controller.remove_agent("ghost-agent").unwrap();
    assert!(!removed);
}
