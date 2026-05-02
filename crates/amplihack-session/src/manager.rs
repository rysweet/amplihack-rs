//! `SessionManager` ported from `session_manager.py`.

use crate::config::{Result, SessionConfig, SessionError};
use crate::file_utils::{safe_read_json, safe_write_json};
use crate::session::{ClaudeSession, CommandRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionMetadata {
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    last_accessed: chrono::DateTime<chrono::Utc>,
    status: String,
    config: SessionConfig,
    metadata: serde_json::Value,
    last_saved: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSession {
    session_id: String,
    state: crate::config::SessionState,
    config: SessionConfig,
    command_history: Vec<CommandRecord>,
    metadata: SessionMetadata,
    saved_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Registry {
    sessions: HashMap<String, SessionMetadata>,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Manages session persistence, resume, archival, and cleanup.
pub struct SessionManager {
    pub runtime_dir: PathBuf,
    active_sessions: HashMap<String, ClaudeSession>,
    metadata: HashMap<String, SessionMetadata>,
}

fn id_regex() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"^[A-Za-z0-9_-]{1,128}$").unwrap())
}

impl SessionManager {
    /// Construct rooted at `runtime_dir` (created if missing).
    pub fn new(runtime_dir: impl Into<PathBuf>) -> Result<Self> {
        let runtime_dir = runtime_dir.into();
        fs::create_dir_all(&runtime_dir).map_err(|e| SessionError::io(&runtime_dir, e))?;
        let mut mgr = Self {
            runtime_dir,
            active_sessions: HashMap::new(),
            metadata: HashMap::new(),
        };
        mgr.load_registry()?;
        Ok(mgr)
    }

    fn load_registry(&mut self) -> Result<()> {
        let path = self.registry_path();
        let reg: Registry = safe_read_json(&path, Registry::default())?;
        self.metadata = reg.sessions;
        Ok(())
    }

    fn save_registry(&self) -> Result<()> {
        let reg = Registry {
            sessions: self.metadata.clone(),
            updated_at: Some(chrono::Utc::now()),
        };
        safe_write_json(self.registry_path(), &reg)
    }

    /// Create a new session under `name`, returning its session_id.
    pub fn create_session(
        &mut self,
        name: &str,
        config: Option<SessionConfig>,
        metadata: Option<serde_json::Value>,
    ) -> Result<String> {
        let config = config.unwrap_or_default();
        let session = ClaudeSession::new(config.clone());
        let id = session.state.session_id.clone();
        let now = chrono::Utc::now();
        let meta = SessionMetadata {
            name: name.to_string(),
            created_at: now,
            last_accessed: now,
            status: "created".to_string(),
            config,
            metadata: metadata.unwrap_or(serde_json::Value::Object(Default::default())),
            last_saved: None,
        };
        self.active_sessions.insert(id.clone(), session);
        self.metadata.insert(id.clone(), meta);
        Ok(id)
    }

    /// Get a borrow of an in-memory active session.
    pub fn get_session(&mut self, session_id: &str) -> Option<&mut ClaudeSession> {
        if let Some(meta) = self.metadata.get_mut(session_id) {
            meta.last_accessed = chrono::Utc::now();
        }
        self.active_sessions.get_mut(session_id)
    }

    /// Persist `session_id` to disk.
    pub fn save_session(&mut self, session_id: &str, force: bool) -> Result<bool> {
        let session = match self.active_sessions.get(session_id) {
            Some(s) => s,
            None => return Ok(false),
        };
        let path = self.session_file_path(session_id);
        let metadata = self
            .metadata
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| SessionMetadata {
                name: session_id.to_string(),
                created_at: chrono::Utc::now(),
                last_accessed: chrono::Utc::now(),
                status: "saved".to_string(),
                config: session.config.clone(),
                metadata: serde_json::Value::Object(Default::default()),
                last_saved: None,
            });
        let payload = PersistedSession {
            session_id: session_id.to_string(),
            state: session.state.clone(),
            config: session.config.clone(),
            command_history: session.get_command_history(usize::MAX),
            metadata: metadata.clone(),
            saved_at: chrono::Utc::now(),
        };

        if !force && path.exists() {
            // Skip rewrite if serialized payload byte-identical to existing file.
            // Use to_string_pretty to match safe_write_json's on-disk format.
            let new_json =
                serde_json::to_string_pretty(&payload).map_err(|e| SessionError::Json {
                    path: path.clone(),
                    source: e,
                })?;
            if let Ok(existing) = fs::read_to_string(&path) {
                if existing == new_json {
                    return Ok(true);
                }
            }
        }

        safe_write_json(&path, &payload)?;
        if let Some(m) = self.metadata.get_mut(session_id) {
            m.last_saved = Some(chrono::Utc::now());
            m.status = "saved".to_string();
        }
        Ok(true)
    }

    /// Re-hydrate a session from disk into the active registry.
    pub fn resume_session(&mut self, session_id: &str) -> Result<Option<&mut ClaudeSession>> {
        Self::validate_session_id(session_id)?;
        let path = self.session_file_path(session_id);
        if !path.exists() {
            return Ok(None);
        }
        let payload: PersistedSession =
            match safe_read_json::<Option<PersistedSession>>(&path, None)? {
                Some(p) => p,
                None => return Ok(None),
            };
        let mut session = ClaudeSession::new(payload.config.clone());
        session.state = payload.state.clone();
        session.set_command_history(payload.command_history);
        self.active_sessions.insert(session_id.to_string(), session);
        self.metadata
            .insert(session_id.to_string(), payload.metadata);
        if let Some(m) = self.metadata.get_mut(session_id) {
            m.status = "resumed".to_string();
            m.last_accessed = chrono::Utc::now();
        }
        Ok(self.active_sessions.get_mut(session_id))
    }

    /// Yield (session_id, path, metadata) for every persisted session JSON in
    /// `runtime_dir`, skipping `registry.json`. Used by listing & cleanup.
    fn iter_session_files(&self) -> Result<Vec<(String, PathBuf, fs::Metadata)>> {
        if !self.runtime_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.runtime_dir)
            .map_err(|e| SessionError::io(&self.runtime_dir, e))?
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) if s != "registry" => s.to_owned(),
                _ => continue,
            };
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            out.push((stem, path, meta));
        }
        Ok(out)
    }

    /// List sessions (active + on-disk) sorted by created_at desc.
    pub fn list_sessions(
        &self,
        active_only: bool,
        include_metadata: bool,
    ) -> Result<Vec<serde_json::Value>> {
        let mut out: Vec<serde_json::Value> = Vec::new();

        for (id, session) in &self.active_sessions {
            let mut info = json!({
                "session_id": id,
                "status": "active",
                "statistics": session.get_statistics(),
            });
            if include_metadata {
                if let Some(m) = self.metadata.get(id) {
                    if let serde_json::Value::Object(ref mut o) = info {
                        o.insert("name".into(), json!(m.name));
                        o.insert("created_at".into(), json!(m.created_at.to_rfc3339()));
                        o.insert("last_accessed".into(), json!(m.last_accessed.to_rfc3339()));
                        o.insert("metadata".into(), m.metadata.clone());
                    }
                }
            }
            out.push(info);
        }

        if !active_only {
            for (stem, path, meta) in self.iter_session_files()? {
                if self.active_sessions.contains_key(&stem) {
                    continue;
                }
                let mut info = json!({
                    "session_id": stem,
                    "status": "saved",
                    "file_path": path.display().to_string(),
                    "file_size": meta.len(),
                });
                if include_metadata {
                    if let Ok(Some(p)) = safe_read_json::<Option<PersistedSession>>(&path, None) {
                        if let serde_json::Value::Object(ref mut o) = info {
                            o.insert("name".into(), json!(p.metadata.name));
                            o.insert(
                                "created_at".into(),
                                json!(p.metadata.created_at.to_rfc3339()),
                            );
                        }
                    }
                }
                out.push(info);
            }
        }
        Ok(out)
    }

    /// Move a session's JSON file into `<runtime>/archive/`.
    pub fn archive_session(&mut self, session_id: &str) -> Result<bool> {
        Self::validate_session_id(session_id)?;
        let archive_dir = self.runtime_dir.join("archive");
        fs::create_dir_all(&archive_dir).map_err(|e| SessionError::io(&archive_dir, e))?;
        let path = self.session_file_path(session_id);
        if !path.exists() {
            return Ok(false);
        }
        let ts = chrono::Utc::now().timestamp();
        let archive_path = archive_dir.join(format!("{session_id}_{ts}.json"));
        fs::rename(&path, &archive_path).map_err(|e| SessionError::io(&archive_path, e))?;
        self.active_sessions.remove(session_id);
        self.metadata.remove(session_id);
        Ok(true)
    }

    /// Archive all session files older than `max_age_days`. Returns count.
    pub fn cleanup_old_sessions(&mut self, max_age_days: u32) -> Result<u64> {
        let cutoff = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(max_age_days as u64 * 86400))
            .ok_or_else(|| SessionError::Corruption("cutoff time underflow".into()))?;
        let to_archive: Vec<String> = self
            .iter_session_files()?
            .into_iter()
            .filter(|(_, _, meta)| meta.modified().map(|mt| mt < cutoff).unwrap_or(false))
            .map(|(stem, _, _)| stem)
            .collect();
        let mut count = 0u64;
        for id in to_archive {
            if self.archive_session(&id)? {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Persist every active session and the registry to disk.
    pub fn save_all_active(&mut self) -> Result<()> {
        let ids: Vec<String> = self.active_sessions.keys().cloned().collect();
        for id in ids {
            self.save_session(&id, true)?;
        }
        self.save_registry()?;
        Ok(())
    }

    /// Validate `id` against `[A-Za-z0-9_-]{1,128}` (security: import path).
    pub fn validate_session_id(id: &str) -> Result<()> {
        if id_regex().is_match(id) {
            Ok(())
        } else {
            Err(SessionError::InvalidSessionId(id.to_string()))
        }
    }

    /// Path to the on-disk JSON file for `session_id`.
    pub fn session_file_path(&self, session_id: &str) -> PathBuf {
        self.runtime_dir.join(format!("{session_id}.json"))
    }

    /// Path to the registry file.
    pub fn registry_path(&self) -> PathBuf {
        self.runtime_dir.join("registry.json")
    }

    /// Number of active in-memory sessions.
    pub fn active_count(&self) -> usize {
        self.active_sessions.len()
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        // Best-effort: persist active sessions and registry on drop.
        let _ = self.save_all_active();
    }
}

/// Helper: read raw session JSON file (used by tests & toolkit import/export).
pub fn read_session_file(path: impl AsRef<Path>) -> Result<Option<serde_json::Value>> {
    let p = path.as_ref();
    if !p.exists() {
        return Ok(None);
    }
    safe_read_json::<Option<serde_json::Value>>(p, None)
}
