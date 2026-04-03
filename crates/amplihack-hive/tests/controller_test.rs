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

// --- apply tests (todo!()) ---

#[test]
#[should_panic(expected = "not yet implemented")]
fn apply_manifest_sets_desired() {
    let mut controller = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learner", 2)]);
    controller.apply(manifest).unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn apply_manifest_with_multiple_agents() {
    let mut controller = HiveController::new();
    let manifest = make_manifest(vec![
        make_agent("learner", "learner", 2),
        make_agent("retriever", "retriever", 3),
        make_agent("gateway", "gateway", 1),
    ]);
    controller.apply(manifest).unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
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
}

// --- reconcile tests (todo!()) ---

#[test]
#[should_panic(expected = "not yet implemented")]
fn reconcile_no_manifest() {
    let mut controller = HiveController::new();
    let _actions = controller.reconcile().unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn reconcile_with_manifest() {
    let mut controller = HiveController::new();
    let manifest = make_manifest(vec![make_agent("learner", "learner", 2)]);
    controller.apply(manifest).unwrap();
    let _actions = controller.reconcile().unwrap();
}

// --- scale_agent tests (todo!()) ---

#[test]
#[should_panic(expected = "not yet implemented")]
fn scale_agent_up() {
    let mut controller = HiveController::new();
    controller.scale_agent("learner", 5).unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn scale_agent_down() {
    let mut controller = HiveController::new();
    controller.scale_agent("learner", 1).unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn scale_agent_nonexistent() {
    let mut controller = HiveController::new();
    controller.scale_agent("no-such-agent", 3).unwrap();
}

// --- remove_agent tests (todo!()) ---

#[test]
#[should_panic(expected = "not yet implemented")]
fn remove_agent_existing() {
    let mut controller = HiveController::new();
    let _removed = controller.remove_agent("learner").unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn remove_agent_nonexistent() {
    let mut controller = HiveController::new();
    let _removed = controller.remove_agent("ghost-agent").unwrap();
}
