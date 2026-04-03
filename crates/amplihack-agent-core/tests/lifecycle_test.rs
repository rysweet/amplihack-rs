use amplihack_agent_core::{AgentLifecycle, BasicLifecycle, HealthStatus, LifecycleState};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_lifecycle() -> BasicLifecycle {
    BasicLifecycle::new()
}

// ---------------------------------------------------------------------------
// Initial state
// ---------------------------------------------------------------------------

#[test]
fn starts_stopped() {
    let lc = make_lifecycle();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Stopped);
}

#[test]
fn default_is_stopped() {
    let lc = BasicLifecycle::default();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Stopped);
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

#[test]
fn start_from_stopped() {
    let mut lc = make_lifecycle();
    lc.start().unwrap();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Running);
}

#[test]
fn start_when_already_running_is_error() {
    let mut lc = make_lifecycle();
    lc.start().unwrap();
    let result = lc.start();
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Stop
// ---------------------------------------------------------------------------

#[test]
fn stop_from_running() {
    let mut lc = make_lifecycle();
    lc.start().unwrap();
    lc.stop().unwrap();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Stopped);
}

#[test]
fn stop_when_already_stopped_is_error() {
    let mut lc = make_lifecycle();
    let result = lc.stop();
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Pause / Resume
// ---------------------------------------------------------------------------

#[test]
fn pause_from_running() {
    let mut lc = make_lifecycle();
    lc.start().unwrap();
    lc.pause().unwrap();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Paused);
}

#[test]
fn pause_when_stopped_is_error() {
    let mut lc = make_lifecycle();
    let result = lc.pause();
    assert!(result.is_err());
}

#[test]
fn resume_from_paused() {
    let mut lc = make_lifecycle();
    lc.start().unwrap();
    lc.pause().unwrap();
    lc.resume().unwrap();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Running);
}

#[test]
fn resume_when_not_paused_is_error() {
    let mut lc = make_lifecycle();
    lc.start().unwrap();
    let result = lc.resume();
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

#[test]
fn health_check_when_stopped() {
    let lc = make_lifecycle();
    let health = lc.health_check();
    assert!(health.healthy);
    assert_eq!(health.lifecycle_state, LifecycleState::Stopped);
    assert_eq!(health.uptime_secs, 0.0);
}

#[test]
fn health_check_when_running() {
    let mut lc = make_lifecycle();
    lc.start().unwrap();
    let health = lc.health_check();
    assert!(health.healthy);
    assert_eq!(health.lifecycle_state, LifecycleState::Running);
    assert!(health.uptime_secs >= 0.0);
}

// ---------------------------------------------------------------------------
// Full lifecycle cycle
// ---------------------------------------------------------------------------

#[test]
fn full_lifecycle_cycle() {
    let mut lc = make_lifecycle();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Stopped);

    lc.start().unwrap();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Running);

    lc.pause().unwrap();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Paused);

    lc.resume().unwrap();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Running);

    lc.stop().unwrap();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Stopped);
}

#[test]
fn stop_from_paused() {
    let mut lc = make_lifecycle();
    lc.start().unwrap();
    lc.pause().unwrap();
    lc.stop().unwrap();
    assert_eq!(lc.lifecycle_state(), LifecycleState::Stopped);
}

// ---------------------------------------------------------------------------
// LifecycleState display
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_state_display() {
    assert_eq!(LifecycleState::Stopped.to_string(), "stopped");
    assert_eq!(LifecycleState::Running.to_string(), "running");
    assert_eq!(LifecycleState::Paused.to_string(), "paused");
    assert_eq!(LifecycleState::Failed.to_string(), "failed");
}

#[test]
fn lifecycle_state_serde() {
    let json = serde_json::to_string(&LifecycleState::Running).unwrap();
    assert_eq!(json, r#""running""#);
    let parsed: LifecycleState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, LifecycleState::Running);
}

// ---------------------------------------------------------------------------
// HealthStatus constructors
// ---------------------------------------------------------------------------

#[test]
fn health_status_ok() {
    let h = HealthStatus::ok(LifecycleState::Running, 42.0);
    assert!(h.healthy);
    assert_eq!(h.lifecycle_state, LifecycleState::Running);
    assert_eq!(h.uptime_secs, 42.0);
    assert_eq!(h.message, "healthy");
}

#[test]
fn health_status_unhealthy() {
    let h = HealthStatus::unhealthy(LifecycleState::Failed, "oom");
    assert!(!h.healthy);
    assert_eq!(h.message, "oom");
    assert_eq!(h.lifecycle_state, LifecycleState::Failed);
}
