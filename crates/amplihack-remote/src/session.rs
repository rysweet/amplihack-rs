//! Remote session management.
//!
//! Manages remote Claude Code session lifecycle: creation, state
//! transitions, output capture, and persistent JSON state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::warn;

use crate::state_lock::file_lock;

/// Session lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Killed,
}

/// A remote Claude Code session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub vm_name: String,
    pub workspace: String,
    pub tmux_session: String,
    pub prompt: String,
    pub command: String,
    pub max_turns: u32,
    pub status: SessionStatus,
    pub memory_mb: u32,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
}

/// Manages remote session lifecycle.
pub struct SessionManager {
    state_file: PathBuf,
    sessions: HashMap<String, Session>,
    used_ids: std::collections::HashSet<String>,
}

impl SessionManager {
    pub const DEFAULT_MEMORY_MB: u32 = 16384;
    pub const DEFAULT_COMMAND: &'static str = "auto";
    pub const DEFAULT_MAX_TURNS: u32 = 10;

    /// Create a new session manager, loading state from disk.
    pub fn new(state_file: Option<PathBuf>) -> Result<Self, String> {
        let state_file =
            state_file.unwrap_or_else(|| dirs_home().join(".amplihack").join("remote-state.json"));

        let mut mgr = Self {
            state_file,
            sessions: HashMap::new(),
            used_ids: std::collections::HashSet::new(),
        };
        mgr.load_state()?;
        Ok(mgr)
    }

    /// Create a new session in PENDING state.
    pub fn create_session(
        &mut self,
        vm_name: &str,
        prompt: &str,
        command: Option<&str>,
        max_turns: Option<u32>,
        memory_mb: Option<u32>,
    ) -> Result<Session, String> {
        if vm_name.trim().is_empty() {
            return Err("vm_name cannot be empty".into());
        }
        if prompt.trim().is_empty() {
            return Err("prompt cannot be empty".into());
        }

        let max_turns = max_turns.unwrap_or(Self::DEFAULT_MAX_TURNS);
        let memory_mb = memory_mb.unwrap_or(Self::DEFAULT_MEMORY_MB);

        if max_turns == 0 {
            return Err("max_turns must be positive".into());
        }
        if memory_mb == 0 {
            return Err("memory_mb must be positive".into());
        }

        let session_id = self.generate_session_id();

        let session = Session {
            workspace: format!("/workspace/{session_id}"),
            tmux_session: session_id.clone(),
            session_id: session_id.clone(),
            vm_name: vm_name.to_string(),
            prompt: prompt.to_string(),
            command: command.unwrap_or(Self::DEFAULT_COMMAND).to_string(),
            max_turns,
            status: SessionStatus::Pending,
            memory_mb,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            exit_code: None,
        };

        self.sessions.insert(session_id.clone(), session.clone());
        self.save_state().map_err(|e| format!("save failed: {e}"))?;

        Ok(session)
    }

    /// Transition a PENDING session to RUNNING.
    pub fn start_session(&mut self, session_id: &str) -> Result<Session, String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Session {session_id} not found"))?;

        if session.status != SessionStatus::Pending {
            return Err(format!(
                "Session {session_id} is not PENDING \
                 (current: {:?})",
                session.status,
            ));
        }

        session.status = SessionStatus::Running;
        session.started_at = Some(Utc::now());

        let result = session.clone();
        self.save_state().map_err(|e| format!("save failed: {e}"))?;
        Ok(result)
    }

    /// Get a session by ID.
    pub fn get_session(&self, session_id: &str) -> Option<&Session> {
        self.sessions.get(session_id)
    }

    /// List sessions, optionally filtered by status.
    pub fn list_sessions(&self, status: Option<SessionStatus>) -> Vec<&Session> {
        self.sessions
            .values()
            .filter(|s| status.is_none() || Some(s.status) == status)
            .collect()
    }

    /// Kill a session (PENDING or RUNNING → KILLED).
    pub fn kill_session(&mut self, session_id: &str) -> bool {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return false;
        };

        session.status = SessionStatus::Killed;
        session.completed_at = Some(Utc::now());

        let _ = self.save_state();
        true
    }

    /// Capture output from a running tmux session via SSH.
    pub async fn capture_output(&self, session_id: &str, lines: u32) -> String {
        let Some(session) = self.sessions.get(session_id) else {
            return String::new();
        };

        // Validate session ID format
        let re = regex::Regex::new(r"^sess-\d{8}-\d{6}-[a-f0-9]{4}$").unwrap();
        if !re.is_match(&session.tmux_session) {
            return String::new();
        }

        let command = format!(
            "tmux capture-pane -t {} -p -S -{lines}",
            session.tmux_session,
        );

        execute_ssh_command(&session.vm_name, &command).await
    }

    // ---- internal ----

    fn generate_session_id(&mut self) -> String {
        let now = Utc::now();
        let date = now.format("%Y%m%d").to_string();
        let time = now.format("%H%M%S").to_string();

        for _ in 0..100 {
            let suffix = format!("{:04x}", rand_u16());
            let id = format!("sess-{date}-{time}-{suffix}");
            if !self.used_ids.contains(&id) && !self.sessions.contains_key(&id) {
                self.used_ids.insert(id.clone());
                return id;
            }
        }

        // Fallback with microseconds
        let micro = &now.format("%f").to_string()[..4];
        format!("sess-{date}-{time}-{micro}")
    }

    fn load_state(&mut self) -> Result<(), String> {
        if !self.state_file.exists() {
            return Ok(());
        }

        let content =
            std::fs::read_to_string(&self.state_file).map_err(|e| format!("read failed: {e}"))?;

        if content.trim().is_empty() {
            return Ok(());
        }

        let data: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| format!("State file corrupt: {e}"))?;

        if let Some(sessions) = data.get("sessions") {
            self.sessions = serde_json::from_value(sessions.clone()).unwrap_or_default();
            self.used_ids = self.sessions.keys().cloned().collect();
        }

        Ok(())
    }

    fn save_state(&self) -> Result<(), String> {
        let lock_path = self.state_file.with_extension("lock");
        let _guard = file_lock(&lock_path).map_err(|e| format!("lock failed: {e}"))?;

        if let Some(parent) = self.state_file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
        }

        // Load existing to merge
        let mut existing: serde_json::Value = if self.state_file.exists() {
            std::fs::read_to_string(&self.state_file)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        let sessions_json =
            serde_json::to_value(&self.sessions).map_err(|e| format!("serialize failed: {e}"))?;

        existing["sessions"] = sessions_json;

        let content = serde_json::to_string_pretty(&existing)
            .map_err(|e| format!("serialize failed: {e}"))?;

        std::fs::write(&self.state_file, content).map_err(|e| format!("write failed: {e}"))?;

        Ok(())
    }
}

/// Execute a command on a remote VM via azlin SSH.
async fn execute_ssh_command(vm_name: &str, command: &str) -> String {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Command::new("azlin")
            .args(["ssh", vm_name, "--", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;

    match output {
        Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(Err(e)) => {
            warn!(
                vm = vm_name,
                error = %e,
                "SSH command failed"
            );
            String::new()
        }
        Err(_) => {
            warn!(vm = vm_name, "SSH command timed out");
            String::new()
        }
    }
}

/// Simple random u16 without external crate.
fn rand_u16() -> u16 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    hasher.finish() as u16
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/root"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_status_serialization() {
        let s = SessionStatus::Running;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#""running""#);
        let s2: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s2, SessionStatus::Running);
    }

    #[test]
    fn create_session_validates_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("state.json");
        let mut mgr = SessionManager::new(Some(state_file)).unwrap();

        assert!(mgr.create_session("", "prompt", None, None, None).is_err());
        assert!(mgr.create_session("vm", "", None, None, None).is_err());
        assert!(
            mgr.create_session("vm", "prompt", None, Some(0), None,)
                .is_err()
        );
    }

    #[test]
    fn session_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("state.json");
        let mut mgr = SessionManager::new(Some(state_file)).unwrap();

        let session = mgr
            .create_session("vm1", "do stuff", None, None, None)
            .unwrap();
        assert_eq!(session.status, SessionStatus::Pending);

        let started = mgr.start_session(&session.session_id).unwrap();
        assert_eq!(started.status, SessionStatus::Running);

        assert!(mgr.kill_session(&session.session_id));

        let s = mgr.get_session(&session.session_id).unwrap();
        assert_eq!(s.status, SessionStatus::Killed);
    }

    #[test]
    fn list_sessions_with_filter() {
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("state.json");
        let mut mgr = SessionManager::new(Some(state_file)).unwrap();

        mgr.create_session("vm1", "task1", None, None, None)
            .unwrap();
        mgr.create_session("vm1", "task2", None, None, None)
            .unwrap();

        let all = mgr.list_sessions(None);
        assert_eq!(all.len(), 2);

        let pending = mgr.list_sessions(Some(SessionStatus::Pending));
        assert_eq!(pending.len(), 2);

        let running = mgr.list_sessions(Some(SessionStatus::Running));
        assert!(running.is_empty());
    }
}
