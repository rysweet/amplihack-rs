use amplihack_agent_core::{AgentSession, AgentState, SessionManager};

// ---------------------------------------------------------------------------
// AgentSession basics
// ---------------------------------------------------------------------------

#[test]
fn session_new_has_correct_ids() {
    let s = AgentSession::new("sess-1", "agent-1");
    assert_eq!(s.session_id, "sess-1");
    assert_eq!(s.agent_id, "agent-1");
}

#[test]
fn session_starts_idle() {
    let s = AgentSession::new("s", "a");
    assert_eq!(s.state, AgentState::Idle);
}

#[test]
fn session_created_at_is_positive() {
    let s = AgentSession::new("s", "a");
    assert!(s.created_at > 0.0);
}

#[test]
fn session_last_active_equals_created_at() {
    let s = AgentSession::new("s", "a");
    assert!((s.last_active - s.created_at).abs() < 0.01);
}

#[test]
fn session_touch_updates_timestamp() {
    let mut s = AgentSession::new("s", "a");
    let before = s.last_active;
    std::thread::sleep(std::time::Duration::from_millis(15));
    s.touch();
    assert!(s.last_active > before);
}

#[test]
fn session_not_expired_when_fresh() {
    let s = AgentSession::new("s", "a");
    assert!(!s.is_expired(3600.0));
}

#[test]
fn session_metadata_starts_empty() {
    let s = AgentSession::new("s", "a");
    assert!(s.metadata.is_empty());
}

#[test]
fn session_serde_roundtrip() {
    let s = AgentSession::new("sess-rt", "agent-rt");
    let json = serde_json::to_string(&s).unwrap();
    let parsed: AgentSession = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.session_id, "sess-rt");
    assert_eq!(parsed.agent_id, "agent-rt");
    assert_eq!(parsed.state, AgentState::Idle);
}

// ---------------------------------------------------------------------------
// SessionManager — construction
// ---------------------------------------------------------------------------

#[test]
fn manager_starts_empty() {
    let mgr = SessionManager::new();
    assert!(mgr.is_empty());
    assert_eq!(mgr.len(), 0);
}

#[test]
fn manager_default_timeout() {
    let mgr = SessionManager::new();
    assert_eq!(mgr.timeout_secs, 3600.0);
}

#[test]
fn manager_custom_timeout() {
    let mgr = SessionManager::new().with_timeout(120.0);
    assert_eq!(mgr.timeout_secs, 120.0);
}

// ---------------------------------------------------------------------------
// SessionManager — create
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn create_session_returns_session() {
    let mut mgr = SessionManager::new();
    let session = mgr.create_session("agent-1").unwrap();
    assert_eq!(session.agent_id, "agent-1");
    assert!(!session.session_id.is_empty());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn create_session_increments_count() {
    let mut mgr = SessionManager::new();
    mgr.create_session("a1").unwrap();
    assert_eq!(mgr.len(), 1);
    mgr.create_session("a2").unwrap();
    assert_eq!(mgr.len(), 2);
}

// ---------------------------------------------------------------------------
// SessionManager — get
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn get_session_returns_existing() {
    let mut mgr = SessionManager::new();
    let created = mgr.create_session("a1").unwrap();
    let fetched = mgr.get_session(&created.session_id).unwrap();
    assert_eq!(fetched.agent_id, "a1");
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn get_session_not_found() {
    let mgr = SessionManager::new();
    let result = mgr.get_session("nonexistent");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// SessionManager — end
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn end_session_removes_it() {
    let mut mgr = SessionManager::new();
    let session = mgr.create_session("a1").unwrap();
    let ended = mgr.end_session(&session.session_id).unwrap();
    assert_eq!(ended.agent_id, "a1");
    assert!(mgr.is_empty());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn end_session_not_found() {
    let mut mgr = SessionManager::new();
    let result = mgr.end_session("nonexistent");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// SessionManager — list
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn list_sessions_returns_all_active() {
    let mut mgr = SessionManager::new();
    mgr.create_session("a1").unwrap();
    mgr.create_session("a2").unwrap();
    let sessions = mgr.list_sessions();
    assert_eq!(sessions.len(), 2);
}

// ---------------------------------------------------------------------------
// SessionManager — state tracking
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn get_session_mut_allows_state_change() {
    let mut mgr = SessionManager::new();
    let created = mgr.create_session("a1").unwrap();
    let session = mgr.get_session_mut(&created.session_id).unwrap();
    session.state = AgentState::Acting;
    let fetched = mgr.get_session(&created.session_id).unwrap();
    assert_eq!(fetched.state, AgentState::Acting);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn session_touch_via_manager() {
    let mut mgr = SessionManager::new();
    let created = mgr.create_session("a1").unwrap();
    let sid = created.session_id.clone();
    std::thread::sleep(std::time::Duration::from_millis(15));
    let session = mgr.get_session_mut(&sid).unwrap();
    let before = session.last_active;
    session.touch();
    assert!(session.last_active > before);
}
