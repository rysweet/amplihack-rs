//! Session management for orchestration patterns.
//!
//! Native Rust port of `session.py`. Provides `OrchestratorSession` with
//! a unique session ID, dedicated log directory, factory methods for
//! creating `ClaudeProcess` instances, and structured logging.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::claude_process::{BuildError, ClaudeProcess, ProcessRunner, TokioProcessRunner};

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("failed to create log directory {path:?}: {source}")]
    LogDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Build(#[from] BuildError),
}

/// A scoped session that groups related Claude process invocations and
/// provides shared logging + factory configuration.
pub struct OrchestratorSession {
    pattern_name: String,
    working_dir: PathBuf,
    log_dir: PathBuf,
    session_id: String,
    model: Option<String>,
    runner: Arc<dyn ProcessRunner>,
    process_counter: usize,
}

impl OrchestratorSession {
    pub fn builder() -> OrchestratorSessionBuilder {
        OrchestratorSessionBuilder::default()
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn pattern_name(&self) -> &str {
        &self.pattern_name
    }

    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    pub fn session_log_path(&self) -> PathBuf {
        self.log_dir.join("session.log")
    }

    pub fn process_log_path(&self, process_id: &str) -> PathBuf {
        self.log_dir.join(format!("{process_id}.log"))
    }

    /// Create a new `ClaudeProcess` configured with this session's working
    /// directory, log directory, model, and runner.
    pub fn create_process(
        &mut self,
        prompt: &str,
        process_id: Option<&str>,
        model: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<ClaudeProcess, SessionError> {
        let pid = match process_id {
            Some(s) => s.to_string(),
            None => {
                self.process_counter += 1;
                format!("process_{:03}", self.process_counter)
            }
        };
        let model = model.map(|s| s.to_string()).or_else(|| self.model.clone());
        let mut builder = ClaudeProcess::builder()
            .prompt(prompt)
            .process_id(pid)
            .working_dir(self.working_dir.clone())
            .log_dir(self.log_dir.clone())
            .runner(self.runner.clone());
        if let Some(m) = model {
            builder = builder.model(m);
        }
        if let Some(t) = timeout {
            builder = builder.timeout(t);
        }
        Ok(builder.build()?)
    }

    pub fn process_count(&self) -> usize {
        self.process_counter
    }

    pub fn log_info(&self, msg: &str) {
        self.log(msg, "INFO");
    }

    pub fn log_warn(&self, msg: &str) {
        self.log(msg, "WARNING");
    }

    pub fn log_error(&self, msg: &str) {
        self.log(msg, "ERROR");
    }

    pub fn log(&self, msg: &str, level: &str) {
        let line = format!("[{}] [{}] {}\n", current_hms(), level, msg);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.session_log_path())
        {
            let _ = f.write_all(line.as_bytes());
        }
        match level {
            "ERROR" => {
                tracing::error!(target: "amplihack_orchestration::session", session = %self.session_id, "{msg}")
            }
            "WARNING" => {
                tracing::warn!(target: "amplihack_orchestration::session", session = %self.session_id, "{msg}")
            }
            _ => {
                tracing::info!(target: "amplihack_orchestration::session", session = %self.session_id, "{msg}")
            }
        }
    }

    pub fn summarize(&self) -> String {
        format!(
            "Session Summary:\n  ID: {}\n  Pattern: {}\n  Working Dir: {}\n  Log Dir: {}\n  Processes Created: {}",
            self.session_id,
            self.pattern_name,
            self.working_dir.display(),
            self.log_dir.display(),
            self.process_counter,
        )
    }

    fn write_metadata(&self) -> std::io::Result<()> {
        let started = humantime_now();
        let metadata = format!(
            "Session ID: {}\nPattern: {}\nWorking Directory: {}\nLog Directory: {}\nModel: {}\nStarted: {}\n{}\n",
            self.session_id,
            self.pattern_name,
            self.working_dir.display(),
            self.log_dir.display(),
            self.model.as_deref().unwrap_or("default"),
            started,
            "-".repeat(80),
        );
        std::fs::write(self.session_log_path(), metadata)
    }
}

#[derive(Default)]
pub struct OrchestratorSessionBuilder {
    pattern_name: Option<String>,
    working_dir: Option<PathBuf>,
    base_log_dir: Option<PathBuf>,
    model: Option<String>,
    runner: Option<Arc<dyn ProcessRunner>>,
}

impl OrchestratorSessionBuilder {
    pub fn pattern_name(mut self, n: impl Into<String>) -> Self {
        self.pattern_name = Some(n.into());
        self
    }
    pub fn working_dir(mut self, d: PathBuf) -> Self {
        self.working_dir = Some(d);
        self
    }
    pub fn base_log_dir(mut self, d: PathBuf) -> Self {
        self.base_log_dir = Some(d);
        self
    }
    pub fn model(mut self, m: impl Into<String>) -> Self {
        self.model = Some(m.into());
        self
    }
    pub fn runner(mut self, r: Arc<dyn ProcessRunner>) -> Self {
        self.runner = Some(r);
        self
    }

    pub fn build(self) -> Result<OrchestratorSession, SessionError> {
        let pattern_name = self
            .pattern_name
            .ok_or(SessionError::MissingField("pattern_name"))?;
        let working_dir = self
            .working_dir
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let session_id = format!("{}_{}", pattern_name, timestamp);
        let base_log_dir = self
            .base_log_dir
            .unwrap_or_else(|| working_dir.join(".claude").join("runtime").join("logs"));
        let log_dir = base_log_dir.join(&session_id);
        std::fs::create_dir_all(&log_dir).map_err(|e| SessionError::LogDir {
            path: log_dir.clone(),
            source: e,
        })?;
        let runner = self
            .runner
            .unwrap_or_else(|| Arc::new(TokioProcessRunner::new()) as Arc<dyn ProcessRunner>);
        let session = OrchestratorSession {
            pattern_name,
            working_dir,
            log_dir,
            session_id,
            model: self.model,
            runner,
            process_counter: 0,
        };
        let _ = session.write_metadata();
        Ok(session)
    }
}

fn current_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

fn humantime_now() -> String {
    // Lightweight, locale-free YYYY-MM-DD HH:MM:SS rendering of UTC now.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d) = days_to_ymd((secs / 86_400) as i64);
    let h = (secs / 3600) % 24;
    let mi = (secs / 60) % 60;
    let se = secs % 60;
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, mi, se)
}

/// Convert "days since 1970-01-01" → (year, month, day) in the proleptic
/// Gregorian calendar.
fn days_to_ymd(mut z: i64) -> (i64, u32, u32) {
    z += 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn humantime_returns_iso_like() {
        let h = humantime_now();
        assert_eq!(h.len(), 19);
        assert_eq!(&h[4..5], "-");
        assert_eq!(&h[10..11], " ");
    }

    #[test]
    fn days_to_ymd_epoch_is_1970_01_01() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn current_hms_is_8_chars() {
        assert_eq!(current_hms().len(), 8);
    }
}
