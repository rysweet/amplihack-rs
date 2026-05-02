//! `ClaudeSession` ported from `claude_session.py`.
//!
//! Threading model: the Python original uses a daemon heartbeat thread; this
//! port replaces that with the explicit [`ClaudeSession::check_health`] method
//! per design spec §1.

use crate::config::{Result, SessionConfig, SessionError, SessionState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Trait for executing user commands. Concrete impls plug into [`ClaudeSession`].
///
/// Replaces the Python `_simulate_command_execution` placeholder with an
/// explicit extension point.
pub trait CommandExecutor: Send + Sync {
    fn execute(
        &self,
        command: &str,
        kwargs: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, SessionError>;
}

/// Default no-op executor that returns a `{"status":"completed"}` shape.
#[derive(Debug, Default, Clone)]
pub struct NoopExecutor;

impl CommandExecutor for NoopExecutor {
    fn execute(
        &self,
        command: &str,
        kwargs: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, SessionError> {
        Ok(json!({
            "command": command,
            "status": "completed",
            "kwargs": kwargs,
        }))
    }
}

/// One row of `ClaudeSession::get_command_history`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandRecord {
    pub command: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub kwargs: serde_json::Value,
    pub result: String,
    pub duration_secs: f64,
    pub error: Option<String>,
}

fn generate_session_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let nanos = now.subsec_nanos();
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::SeqCst) as u32;
    let mix = nanos
        .wrapping_add(counter.wrapping_mul(2_654_435_761))
        .wrapping_add(secs as u32);
    format!("claude_session_{secs}_{mix:08x}")
}

/// Enhanced session wrapper with timeout handling and lifecycle management.
pub struct ClaudeSession {
    pub config: SessionConfig,
    pub state: SessionState,
    executor: Box<dyn CommandExecutor>,
    command_history: Vec<CommandRecord>,
    checkpoints: Vec<SessionState>,
}

impl ClaudeSession {
    /// Construct with `config` and the default [`NoopExecutor`].
    pub fn new(config: SessionConfig) -> Self {
        Self::with_executor(config, Box::new(NoopExecutor))
    }

    /// Construct with an explicit executor.
    pub fn with_executor(config: SessionConfig, executor: Box<dyn CommandExecutor>) -> Self {
        let id = config
            .session_id
            .clone()
            .unwrap_or_else(generate_session_id);
        let state = SessionState::new(id);
        Self {
            config,
            state,
            executor,
            command_history: Vec::new(),
            checkpoints: Vec::new(),
        }
    }

    pub fn start(&mut self) {
        let now = chrono::Utc::now();
        self.state.is_active = true;
        self.state.start_time = now;
        self.state.last_activity = now;
        tracing::info!("session {} started", self.state.session_id);
    }

    pub fn stop(&mut self) {
        self.state.is_active = false;
        tracing::info!("session {} stopped", self.state.session_id);
    }

    pub fn execute_command(
        &mut self,
        command: &str,
        timeout: Option<Duration>,
        kwargs: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if !self.state.is_active {
            return Err(SessionError::NotActive);
        }
        let _effective_timeout = timeout.unwrap_or(self.config.timeout);
        let started = Instant::now();
        let started_ts = chrono::Utc::now();
        self.state.command_count += 1;
        self.state.last_activity = started_ts;

        let result = self.executor.execute(command, &kwargs);
        let dur = started.elapsed().as_secs_f64();

        let record = match &result {
            Ok(_) => CommandRecord {
                command: command.to_string(),
                timestamp: started_ts,
                kwargs: kwargs.clone(),
                result: "success".to_string(),
                duration_secs: dur,
                error: None,
            },
            Err(e) => {
                self.state.error_count += 1;
                self.state.last_error = Some(e.to_string());
                CommandRecord {
                    command: command.to_string(),
                    timestamp: started_ts,
                    kwargs: kwargs.clone(),
                    result: "error".to_string(),
                    duration_secs: dur,
                    error: Some(e.to_string()),
                }
            }
        };
        self.command_history.push(record);
        result
    }

    /// Explicit health check (replaces Python heartbeat thread).
    /// Returns `Err(SessionError::Timeout)` if `last_activity` is older than
    /// `config.timeout`.
    pub fn check_health(&mut self) -> Result<()> {
        let now = chrono::Utc::now();
        let elapsed = now.signed_duration_since(self.state.last_activity);
        let elapsed_ms = elapsed.num_milliseconds().max(0) as u64;
        let timeout_ms = self.config.timeout.as_millis() as u64;
        if elapsed_ms > timeout_ms {
            self.state.is_active = false;
            self.state.last_error = Some(format!(
                "Session timeout after {:.3}s",
                self.config.timeout.as_secs_f64()
            ));
            return Err(SessionError::Timeout {
                timeout: self.config.timeout,
            });
        }
        Ok(())
    }

    pub fn save_checkpoint(&mut self) {
        self.checkpoints.push(self.state.clone());
    }

    /// Restore a checkpoint by index. Negative indices count from the end:
    /// `-1` selects the most recent checkpoint.
    pub fn restore_checkpoint(&mut self, index: i64) -> Result<()> {
        if self.checkpoints.is_empty() {
            return Err(SessionError::NoCheckpoints);
        }
        let len = self.checkpoints.len() as i64;
        let idx = if index < 0 { len + index } else { index };
        if idx < 0 || idx >= len {
            return Err(SessionError::CheckpointOutOfRange(index));
        }
        self.state = self.checkpoints[idx as usize].clone();
        Ok(())
    }

    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    pub fn get_statistics(&self) -> serde_json::Value {
        let now = chrono::Utc::now();
        let uptime = now
            .signed_duration_since(self.state.start_time)
            .num_milliseconds() as f64
            / 1000.0;
        let since = now
            .signed_duration_since(self.state.last_activity)
            .num_milliseconds() as f64
            / 1000.0;
        let denom = self.state.command_count.max(1) as f64;
        json!({
            "session_id": self.state.session_id,
            "uptime": uptime,
            "command_count": self.state.command_count,
            "error_count": self.state.error_count,
            "error_rate": self.state.error_count as f64 / denom,
            "is_active": self.state.is_active,
            "checkpoints": self.checkpoints.len(),
            "last_activity": self.state.last_activity.to_rfc3339(),
            "time_since_activity": since,
        })
    }

    pub fn get_command_history(&self, limit: usize) -> Vec<CommandRecord> {
        let len = self.command_history.len();
        let take = limit.min(len);
        self.command_history[len - take..].to_vec()
    }

    pub fn clear_history(&mut self) {
        self.command_history.clear();
        self.checkpoints.clear();
    }

    /// Internal: replace the command_history vector (used by deserialization).
    pub(crate) fn set_command_history(&mut self, hist: Vec<CommandRecord>) {
        self.command_history = hist;
    }
}
