//! Agent session management.
//!
//! Matches Python session tracking in `amplihack/agents/goal_seeking/`.
//! Provides `AgentSession` for per-agent session state and `SessionManager`
//! for creating, retrieving, and ending sessions.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{AgentError, Result};
use crate::models::AgentState;

// ---------------------------------------------------------------------------
// AgentSession
// ---------------------------------------------------------------------------

/// A single agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub session_id: String,
    pub agent_id: String,
    pub created_at: f64,
    pub last_active: f64,
    pub state: AgentState,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

impl AgentSession {
    /// Create a new session with the given IDs.
    pub fn new(session_id: impl Into<String>, agent_id: impl Into<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH")
            .as_secs_f64();
        Self {
            session_id: session_id.into(),
            agent_id: agent_id.into(),
            created_at: now,
            last_active: now,
            state: AgentState::Idle,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Whether this session has been idle longer than `timeout_secs`.
    pub fn is_expired(&self, timeout_secs: f64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH")
            .as_secs_f64();
        (now - self.last_active) > timeout_secs
    }

    /// Touch the session to update `last_active`.
    pub fn touch(&mut self) {
        self.last_active = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH")
            .as_secs_f64();
    }
}

// ---------------------------------------------------------------------------
// SessionManager
// ---------------------------------------------------------------------------

/// Manages agent sessions.
pub struct SessionManager {
    sessions: std::collections::HashMap<String, AgentSession>,
    /// Default session timeout in seconds.
    pub timeout_secs: f64,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: std::collections::HashMap::new(),
            timeout_secs: 3600.0,
        }
    }

    pub fn with_timeout(mut self, secs: f64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Remove all expired sessions from the manager.
    pub fn cleanup_expired(&mut self) {
        let timeout = self.timeout_secs;
        self.sessions.retain(|_, s| !s.is_expired(timeout));
    }

    /// Create a new session for the given agent.
    pub fn create_session(&mut self, agent_id: &str) -> Result<AgentSession> {
        self.cleanup_expired();
        static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let now_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH")
            .as_nanos();
        let session_id = format!("sess-{now_nanos:x}-{count}");
        let session = AgentSession::new(&session_id, agent_id);
        self.sessions.insert(session_id, session.clone());
        Ok(session)
    }

    /// Retrieve an existing session by ID.
    pub fn get_session(&self, session_id: &str) -> Result<&AgentSession> {
        self.sessions
            .get(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))
    }

    /// Retrieve a mutable reference to an existing session.
    pub fn get_session_mut(&mut self, session_id: &str) -> Result<&mut AgentSession> {
        self.sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))
    }

    /// End (remove) a session by ID.
    pub fn end_session(&mut self, session_id: &str) -> Result<AgentSession> {
        self.sessions
            .remove(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))
    }

    /// List all active (non-expired) sessions.
    pub fn list_sessions(&self) -> Vec<&AgentSession> {
        self.sessions
            .values()
            .filter(|s| !s.is_expired(self.timeout_secs))
            .collect()
    }

    /// Number of active (non-expired) sessions.
    pub fn len(&self) -> usize {
        let timeout = self.timeout_secs;
        self.sessions
            .values()
            .filter(|s| !s.is_expired(timeout))
            .count()
    }

    /// Whether the manager has no active sessions.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_new_defaults() {
        let s = AgentSession::new("s1", "a1");
        assert_eq!(s.session_id, "s1");
        assert_eq!(s.agent_id, "a1");
        assert_eq!(s.state, AgentState::Idle);
        assert!(s.created_at > 0.0);
    }

    #[test]
    fn session_touch_updates_last_active() {
        let mut s = AgentSession::new("s1", "a1");
        let before = s.last_active;
        std::thread::sleep(std::time::Duration::from_millis(10));
        s.touch();
        assert!(s.last_active >= before);
    }

    #[test]
    fn session_serde_roundtrip() {
        let s = AgentSession::new("s1", "a1");
        let json = serde_json::to_string(&s).unwrap();
        let parsed: AgentSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "s1");
        assert_eq!(parsed.agent_id, "a1");
    }

    #[test]
    fn session_manager_starts_empty() {
        let mgr = SessionManager::new();
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
    }
}
