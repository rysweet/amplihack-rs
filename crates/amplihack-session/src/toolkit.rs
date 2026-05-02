//! `SessionToolkit` façade ported from `session_toolkit.py`.

use crate::config::{Result, SessionConfig, SessionError};
use crate::file_utils::{cleanup_temp_files, safe_read_json, safe_write_json};
use crate::logger::{LogLevel, ToolkitLogger};
use crate::manager::SessionManager;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// Unified façade combining [`SessionManager`] and [`ToolkitLogger`].
pub struct SessionToolkit {
    pub runtime_dir: PathBuf,
    pub auto_save: bool,
    pub log_level: String,
    manager: SessionManager,
    pub(crate) current_session_id: Option<String>,
    logger: Option<ToolkitLogger>,
}

impl SessionToolkit {
    pub fn new(
        runtime_dir: impl Into<PathBuf>,
        auto_save: bool,
        log_level: impl Into<String>,
    ) -> Result<Self> {
        let runtime_dir = runtime_dir.into();
        fs::create_dir_all(&runtime_dir).map_err(|e| SessionError::io(&runtime_dir, e))?;
        let manager = SessionManager::new(runtime_dir.join("sessions"))?;
        let level_str = log_level.into();
        let logger = ToolkitLogger::builder()
            .log_dir(runtime_dir.join("logs"))
            .level(LogLevel::parse(&level_str))
            .enable_console(false)
            .enable_file(true)
            .component("session_toolkit")
            .build()?;
        Ok(Self {
            runtime_dir,
            auto_save,
            log_level: level_str,
            manager,
            current_session_id: None,
            logger: Some(logger),
        })
    }

    pub fn create_session(
        &mut self,
        name: &str,
        config: Option<SessionConfig>,
        metadata: Option<serde_json::Value>,
    ) -> Result<String> {
        let id = self.manager.create_session(name, config, metadata)?;
        self.current_session_id = Some(id.clone());
        Ok(id)
    }

    pub fn list_sessions(&self, active_only: bool) -> Result<Vec<serde_json::Value>> {
        self.manager.list_sessions(active_only, true)
    }

    pub fn delete_session(&mut self, session_id: &str) -> Result<bool> {
        self.manager.archive_session(session_id)
    }

    pub fn save_current(&mut self) -> Result<bool> {
        match self.current_session_id.clone() {
            Some(id) => self.manager.save_session(&id, true),
            None => Ok(false),
        }
    }

    pub fn current_session_id(&self) -> Option<&str> {
        self.current_session_id.as_deref()
    }

    pub fn get_logger(&self, component: Option<&str>) -> Result<Option<ToolkitLogger>> {
        let parent = match &self.logger {
            Some(l) => l,
            None => return Ok(None),
        };
        match component {
            Some(c) => Ok(Some(parent.create_child_logger(c)?)),
            None => {
                // Return a clone-equivalent logger pointing at the same files.
                let cloned = ToolkitLogger::builder()
                    .log_dir(parent.log_dir.clone())
                    .level(parent.level)
                    .enable_console(false)
                    .enable_file(true)
                    .session_id(parent.session_id.clone().unwrap_or_default())
                    .component(parent.component.clone().unwrap_or_default())
                    .build()?;
                Ok(Some(cloned))
            }
        }
    }

    pub fn manager(&self) -> &SessionManager {
        &self.manager
    }

    pub fn manager_mut(&mut self) -> &mut SessionManager {
        &mut self.manager
    }

    /// Export a session to a portable JSON file.
    pub fn export_session(&self, session_id: &str, dst: impl AsRef<Path>) -> Result<()> {
        SessionManager::validate_session_id(session_id)?;
        let src = self.manager.session_file_path(session_id);
        if !src.exists() {
            return Err(SessionError::NotFound(session_id.to_string()));
        }
        let value: serde_json::Value =
            safe_read_json(&src, serde_json::Value::Object(Default::default()))?;
        safe_write_json(dst.as_ref(), &value)
    }

    /// Import a session from a portable JSON file. Validates session_id.
    pub fn import_session(&mut self, src: impl AsRef<Path>) -> Result<String> {
        let src = src.as_ref();
        let value: serde_json::Value =
            safe_read_json(src, serde_json::Value::Object(Default::default()))?;
        let id = value
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                SessionError::Corruption(format!(
                    "import file {} missing session_id",
                    src.display()
                ))
            })?
            .to_string();
        SessionManager::validate_session_id(&id)?;
        let dst = self.manager.session_file_path(&id);
        safe_write_json(&dst, &value)?;
        Ok(id)
    }

    pub fn cleanup_old_data(&mut self, max_age_days: u32) -> Result<u64> {
        let mut total = self.manager.cleanup_old_sessions(max_age_days)?;
        let temp_dir = self.runtime_dir.join("temp");
        total = total.saturating_add(cleanup_temp_files(&temp_dir, 24.0, "*.tmp")?);
        let log_dir = self.runtime_dir.join("logs");
        total = total.saturating_add(cleanup_temp_files(
            &log_dir,
            (max_age_days as f64) * 24.0,
            "*.log",
        )?);
        Ok(total)
    }

    pub fn get_toolkit_stats(&self) -> Result<serde_json::Value> {
        let sessions = self.list_sessions(false)?;
        let active = sessions
            .iter()
            .filter(|v| v.get("status").and_then(|s| s.as_str()) == Some("active"))
            .count();
        Ok(json!({
            "total_sessions": sessions.len(),
            "active_sessions": active,
            "runtime_dir": self.runtime_dir.display().to_string(),
            "auto_save_enabled": self.auto_save,
            "current_session_id": self.current_session_id,
        }))
    }
}

/// Quick RAII helper: create a toolkit, run `f` against a fresh session,
/// then auto-save.
pub fn quick_session<F, R>(name: &str, f: F) -> Result<R>
where
    F: FnOnce(&mut SessionToolkit, &str) -> Result<R>,
{
    let mut tk = SessionToolkit::new(".claude/runtime", true, "INFO")?;
    let sid = tk.create_session(name, None, None)?;
    if let Some(s) = tk.manager_mut().get_session(&sid) {
        s.start();
    }
    let result = f(&mut tk, &sid)?;
    if tk.auto_save {
        tk.save_current()?;
    }
    if let Some(s) = tk.manager_mut().get_session(&sid) {
        s.stop();
    }
    Ok(result)
}
