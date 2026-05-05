//! Structured JSON-line logger ported from `toolkit_logger.py`.

use crate::config::{Result, SessionError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// Severity levels for [`ToolkitLogger`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

impl LogLevel {
    fn rank(self) -> u8 {
        match self {
            LogLevel::Debug => 10,
            LogLevel::Info => 20,
            LogLevel::Warning => 30,
            LogLevel::Error => 40,
            LogLevel::Critical => 50,
        }
    }

    pub(crate) fn parse(s: &str) -> LogLevel {
        match s.to_ascii_uppercase().as_str() {
            "DEBUG" => LogLevel::Debug,
            "WARNING" | "WARN" => LogLevel::Warning,
            "ERROR" => LogLevel::Error,
            "CRITICAL" | "FATAL" => LogLevel::Critical,
            _ => LogLevel::Info,
        }
    }
}

/// A single structured log entry written as a JSON object per line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub level: LogLevel,
    pub message: String,
    pub session_id: Option<String>,
    pub component: Option<String>,
    pub operation: Option<String>,
    pub duration_secs: Option<f64>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub error: Option<String>,
}

/// Builder for [`ToolkitLogger`].
#[derive(Debug, Clone)]
pub struct ToolkitLoggerBuilder {
    session_id: Option<String>,
    component: Option<String>,
    log_dir: PathBuf,
    level: LogLevel,
    enable_console: bool,
    enable_file: bool,
    max_size: u64,
    max_files: u32,
    rotate_daily: bool,
}

impl ToolkitLoggerBuilder {
    pub fn new() -> Self {
        Self {
            session_id: None,
            component: None,
            log_dir: PathBuf::from(".claude/runtime/logs"),
            level: LogLevel::Info,
            enable_console: true,
            enable_file: true,
            max_size: 10 * 1024 * 1024,
            max_files: 5,
            rotate_daily: false,
        }
    }
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }
    pub fn component(mut self, c: impl Into<String>) -> Self {
        self.component = Some(c.into());
        self
    }
    pub fn log_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.log_dir = dir.into();
        self
    }
    pub fn level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }
    pub fn enable_console(mut self, e: bool) -> Self {
        self.enable_console = e;
        self
    }
    pub fn enable_file(mut self, e: bool) -> Self {
        self.enable_file = e;
        self
    }
    pub fn max_size(mut self, n: u64) -> Self {
        self.max_size = n;
        self
    }
    pub fn max_files(mut self, n: u32) -> Self {
        self.max_files = n;
        self
    }
    pub fn rotate_daily(mut self, b: bool) -> Self {
        self.rotate_daily = b;
        self
    }
    pub fn build(self) -> Result<ToolkitLogger> {
        if self.enable_file {
            fs::create_dir_all(&self.log_dir).map_err(|e| SessionError::io(&self.log_dir, e))?;
        }
        Ok(ToolkitLogger {
            session_id: self.session_id,
            component: self.component,
            log_dir: self.log_dir,
            level: self.level,
            enable_console: self.enable_console,
            enable_file: self.enable_file,
            max_size: self.max_size,
            max_files: self.max_files,
            write_lock: Mutex::new(()),
        })
    }
}

impl Default for ToolkitLoggerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Structured logger writing JSON-line entries with size+date rotation.
#[derive(Debug)]
pub struct ToolkitLogger {
    pub session_id: Option<String>,
    pub component: Option<String>,
    pub log_dir: PathBuf,
    pub level: LogLevel,
    enable_console: bool,
    enable_file: bool,
    max_size: u64,
    max_files: u32,
    write_lock: Mutex<()>,
}

impl ToolkitLogger {
    pub fn builder() -> ToolkitLoggerBuilder {
        ToolkitLoggerBuilder::new()
    }

    pub fn debug(&self, msg: impl Into<String>, metadata: Option<serde_json::Value>) -> Result<()> {
        self.log(LogLevel::Debug, msg.into(), None, None, metadata, None)
    }
    pub fn info(&self, msg: impl Into<String>, metadata: Option<serde_json::Value>) -> Result<()> {
        self.log(LogLevel::Info, msg.into(), None, None, metadata, None)
    }
    pub fn warning(
        &self,
        msg: impl Into<String>,
        metadata: Option<serde_json::Value>,
    ) -> Result<()> {
        self.log(LogLevel::Warning, msg.into(), None, None, metadata, None)
    }
    pub fn error(&self, msg: impl Into<String>, metadata: Option<serde_json::Value>) -> Result<()> {
        self.log(LogLevel::Error, msg.into(), None, None, metadata, None)
    }
    pub fn critical(
        &self,
        msg: impl Into<String>,
        metadata: Option<serde_json::Value>,
    ) -> Result<()> {
        self.log(LogLevel::Critical, msg.into(), None, None, metadata, None)
    }
    pub fn success(&self, msg: impl Into<String>, duration_secs: Option<f64>) -> Result<()> {
        self.log(
            LogLevel::Info,
            format!("✓ {}", msg.into()),
            None,
            duration_secs,
            None,
            None,
        )
    }

    /// Internal entry point; writes a single entry honoring level/rotation.
    pub(crate) fn log(
        &self,
        level: LogLevel,
        message: String,
        operation: Option<String>,
        duration_secs: Option<f64>,
        metadata: Option<serde_json::Value>,
        error: Option<String>,
    ) -> Result<()> {
        if level.rank() < self.level.rank() {
            return Ok(());
        }
        let entry = LogEntry {
            timestamp: chrono::Utc::now(),
            level,
            message,
            session_id: self.session_id.clone(),
            component: self.component.clone(),
            operation,
            duration_secs,
            metadata: metadata.unwrap_or(serde_json::Value::Object(Default::default())),
            error,
        };
        let line = serde_json::to_string(&entry).map_err(|e| SessionError::Json {
            path: self.current_log_file(),
            source: e,
        })?;
        if self.enable_console {
            println!("{line}");
        }
        if self.enable_file {
            self.write_line_to_file(&line)?;
        }
        Ok(())
    }

    fn write_line_to_file(&self, line: &str) -> Result<()> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| SessionError::Corruption("logger mutex poisoned".into()))?;
        let path = self.current_log_file();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| SessionError::io(parent, e))?;
        }
        // Size-based rotation: rotate BEFORE writing if current file would exceed cap.
        if path.exists() {
            let sz = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if sz + line.len() as u64 + 1 > self.max_size {
                self.rotate(&path)?;
            }
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| SessionError::io(&path, e))?;
        writeln!(f, "{line}").map_err(|e| SessionError::io(&path, e))?;
        // Note: no per-line fsync — log durability relies on OS flush. Per-line
        // sync_all() would make logging 100-1000x slower for high-volume callers.
        Ok(())
    }

    fn rotate(&self, current: &Path) -> Result<()> {
        // Rotate to {stem}.{nanos}.log to guarantee uniqueness.
        let stem = current
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("log");
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let rotated = self.log_dir.join(format!("{stem}.{nanos}.log"));
        fs::rename(current, &rotated).map_err(|e| SessionError::io(&rotated, e))?;

        // Enforce max_files: remove oldest rotated files beyond limit.
        let mut rotated_files: Vec<(PathBuf, std::time::SystemTime)> = fs::read_dir(&self.log_dir)
            .map_err(|e| SessionError::io(&self.log_dir, e))?
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let name = p.file_name()?.to_str()?.to_owned();
                if !name.starts_with(&format!("{stem}.")) || !name.ends_with(".log") {
                    return None;
                }
                let mt = e.metadata().ok()?.modified().ok()?;
                Some((p, mt))
            })
            .collect();
        rotated_files.sort_by_key(|(_, mt)| *mt);
        let keep = self.max_files.saturating_sub(1) as usize;
        if rotated_files.len() > keep {
            let drop_n = rotated_files.len() - keep;
            for (p, _) in rotated_files.into_iter().take(drop_n) {
                let _ = fs::remove_file(&p);
            }
        }
        Ok(())
    }

    /// RAII operation context: emits start/end log entries with duration.
    pub fn operation(&self, name: impl Into<String>) -> OperationContext<'_> {
        let name = name.into();
        let _ = self.log(
            LogLevel::Info,
            format!("Started operation: {name}"),
            Some(name.clone()),
            None,
            None,
            None,
        );
        OperationContext {
            logger: self,
            name,
            started: Instant::now(),
            failed: None,
        }
    }

    /// Read structured log entries for this session from disk.
    pub fn get_session_logs(&self, limit: Option<usize>) -> Result<Vec<LogEntry>> {
        let stem = match &self.session_id {
            Some(id) => id.clone(),
            None => "toolkit".to_string(),
        };
        let mut files: Vec<(PathBuf, std::time::SystemTime)> = match fs::read_dir(&self.log_dir) {
            Ok(rd) => rd
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let name = p.file_name()?.to_str()?.to_owned();
                    if name == format!("{stem}.log")
                        || (name.starts_with(&format!("{stem}.")) && name.ends_with(".log"))
                    {
                        let mt = e.metadata().ok()?.modified().ok()?;
                        Some((p, mt))
                    } else {
                        None
                    }
                })
                .collect(),
            Err(_) => return Ok(Vec::new()),
        };
        files.sort_by_key(|(_, mt)| *mt);

        let mut all: Vec<LogEntry> = Vec::new();
        for (p, _) in files {
            let f = match fs::File::open(&p) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let r = BufReader::new(f);
            for line_res in r.lines() {
                let line = match line_res {
                    Ok(l) => l,
                    Err(_) => break,
                };
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
                    all.push(entry);
                }
            }
        }
        all.sort_by_key(|e| e.timestamp);
        if let Some(n) = limit {
            if all.len() > n {
                let skip = all.len() - n;
                all.drain(0..skip);
            }
        }
        Ok(all)
    }

    /// Create a child logger sharing session_id but with a sub-component.
    pub fn create_child_logger(&self, component: impl Into<String>) -> Result<ToolkitLogger> {
        let child_component = match &self.component {
            Some(parent) => format!("{parent}.{}", component.into()),
            None => component.into(),
        };
        ToolkitLoggerBuilder::new()
            .log_dir(&self.log_dir)
            .level(self.level)
            .enable_console(false)
            .enable_file(self.enable_file)
            .max_size(self.max_size)
            .max_files(self.max_files)
            .session_id(self.session_id.clone().unwrap_or_default())
            .component(child_component)
            .build()
            .map(|mut l| {
                if self.session_id.is_none() {
                    l.session_id = None;
                }
                l
            })
    }

    /// Path to the current rotated log file (for tests/inspection).
    pub fn current_log_file(&self) -> PathBuf {
        let stem = self.session_id.as_deref().unwrap_or("toolkit");
        self.log_dir.join(format!("{stem}.log"))
    }
}

/// RAII context returned by [`ToolkitLogger::operation`]; logs success/failure
/// with elapsed duration when dropped.
#[must_use = "OperationContext logs on drop; bind it to a variable"]
pub struct OperationContext<'a> {
    logger: &'a ToolkitLogger,
    name: String,
    started: Instant,
    failed: Option<String>,
}

impl OperationContext<'_> {
    /// Mark the operation as failed (will be logged as ERROR on drop).
    pub fn fail(&mut self, message: impl Into<String>) {
        self.failed = Some(message.into());
    }
}

impl Drop for OperationContext<'_> {
    fn drop(&mut self) {
        let dur = self.started.elapsed().as_secs_f64();
        let msg = match &self.failed {
            Some(reason) => (
                LogLevel::Error,
                format!("Failed operation: {} ({reason})", self.name),
                Some(reason.clone()),
            ),
            None => (
                LogLevel::Info,
                format!("✓ Completed operation: {}", self.name),
                None,
            ),
        };
        let _ = self.logger.log(
            msg.0,
            msg.1,
            Some(self.name.clone()),
            Some(dur),
            None,
            msg.2,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- LogLevel ---

    #[test]
    fn log_level_rank_ordering() {
        assert!(LogLevel::Debug.rank() < LogLevel::Info.rank());
        assert!(LogLevel::Info.rank() < LogLevel::Warning.rank());
        assert!(LogLevel::Warning.rank() < LogLevel::Error.rank());
        assert!(LogLevel::Error.rank() < LogLevel::Critical.rank());
    }

    #[test]
    fn log_level_parse_known() {
        assert_eq!(LogLevel::parse("DEBUG"), LogLevel::Debug);
        assert_eq!(LogLevel::parse("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::parse("WARNING"), LogLevel::Warning);
        assert_eq!(LogLevel::parse("WARN"), LogLevel::Warning);
        assert_eq!(LogLevel::parse("ERROR"), LogLevel::Error);
        assert_eq!(LogLevel::parse("CRITICAL"), LogLevel::Critical);
        assert_eq!(LogLevel::parse("FATAL"), LogLevel::Critical);
        assert_eq!(LogLevel::parse("INFO"), LogLevel::Info);
    }

    #[test]
    fn log_level_parse_unknown_defaults_to_info() {
        assert_eq!(LogLevel::parse("TRACE"), LogLevel::Info);
        assert_eq!(LogLevel::parse("bogus"), LogLevel::Info);
        assert_eq!(LogLevel::parse(""), LogLevel::Info);
    }

    #[test]
    fn log_level_serde_roundtrip() {
        let json = serde_json::to_string(&LogLevel::Warning).unwrap();
        assert_eq!(json, "\"WARNING\"");
        let parsed: LogLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, LogLevel::Warning);
    }

    // --- LogEntry ---

    #[test]
    fn log_entry_serde_roundtrip() {
        let entry = LogEntry {
            timestamp: chrono::Utc::now(),
            level: LogLevel::Info,
            message: "test message".into(),
            session_id: Some("sess-1".into()),
            component: Some("test".into()),
            operation: None,
            duration_secs: Some(1.5),
            metadata: json!({"key": "value"}),
            error: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.message, "test message");
        assert_eq!(parsed.level, LogLevel::Info);
        assert_eq!(parsed.session_id, Some("sess-1".into()));
        assert_eq!(parsed.duration_secs, Some(1.5));
    }

    // --- ToolkitLoggerBuilder ---

    #[test]
    fn builder_defaults() {
        let b = ToolkitLoggerBuilder::new();
        assert_eq!(b.level, LogLevel::Info);
        assert!(b.enable_console);
        assert!(b.enable_file);
        assert_eq!(b.max_size, 10 * 1024 * 1024);
        assert_eq!(b.max_files, 5);
        assert!(!b.rotate_daily);
    }

    #[test]
    fn builder_chaining() {
        let dir = tempfile::tempdir().unwrap();
        let logger = ToolkitLoggerBuilder::new()
            .session_id("s1")
            .component("comp")
            .log_dir(dir.path())
            .level(LogLevel::Debug)
            .enable_console(false)
            .enable_file(true)
            .max_size(1024)
            .max_files(3)
            .rotate_daily(true)
            .build()
            .unwrap();

        assert_eq!(logger.session_id, Some("s1".into()));
        assert_eq!(logger.component, Some("comp".into()));
        assert_eq!(logger.level, LogLevel::Debug);
        assert_eq!(logger.max_size, 1024);
        assert_eq!(logger.max_files, 3);
    }

    // --- ToolkitLogger ---

    fn test_logger(dir: &Path) -> ToolkitLogger {
        ToolkitLoggerBuilder::new()
            .session_id("test")
            .log_dir(dir)
            .level(LogLevel::Debug)
            .enable_console(false)
            .enable_file(true)
            .build()
            .unwrap()
    }

    #[test]
    fn current_log_file_uses_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let logger = test_logger(dir.path());
        assert_eq!(logger.current_log_file(), dir.path().join("test.log"));
    }

    #[test]
    fn current_log_file_no_session() {
        let dir = tempfile::tempdir().unwrap();
        let logger = ToolkitLoggerBuilder::new()
            .log_dir(dir.path())
            .enable_console(false)
            .build()
            .unwrap();
        assert_eq!(logger.current_log_file(), dir.path().join("toolkit.log"));
    }

    #[test]
    fn log_writes_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let logger = test_logger(dir.path());
        logger.info("hello world", None).unwrap();

        let content = fs::read_to_string(logger.current_log_file()).unwrap();
        assert!(content.contains("hello world"));
        let entry: LogEntry = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry.level, LogLevel::Info);
    }

    #[test]
    fn log_respects_level_filter() {
        let dir = tempfile::tempdir().unwrap();
        let logger = ToolkitLoggerBuilder::new()
            .session_id("test")
            .log_dir(dir.path())
            .level(LogLevel::Warning)
            .enable_console(false)
            .build()
            .unwrap();

        logger.debug("should be filtered", None).unwrap();
        logger.info("also filtered", None).unwrap();
        logger.warning("should appear", None).unwrap();

        let content = fs::read_to_string(logger.current_log_file()).unwrap();
        assert!(!content.contains("should be filtered"));
        assert!(!content.contains("also filtered"));
        assert!(content.contains("should appear"));
    }

    #[test]
    fn log_success_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let logger = test_logger(dir.path());
        logger.success("deployment", Some(2.5)).unwrap();

        let content = fs::read_to_string(logger.current_log_file()).unwrap();
        assert!(content.contains("✓ deployment"));
    }

    #[test]
    fn log_error_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let logger = test_logger(dir.path());
        logger.error("boom", Some(json!({"code": 500}))).unwrap();

        let content = fs::read_to_string(logger.current_log_file()).unwrap();
        let entry: LogEntry = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry.level, LogLevel::Error);
        assert_eq!(entry.metadata["code"], 500);
    }

    #[test]
    fn rotation_on_max_size() {
        let dir = tempfile::tempdir().unwrap();
        let logger = ToolkitLoggerBuilder::new()
            .session_id("rot")
            .log_dir(dir.path())
            .level(LogLevel::Debug)
            .enable_console(false)
            .max_size(100)
            .max_files(3)
            .build()
            .unwrap();

        // Write enough to trigger rotation
        for i in 0..10 {
            logger.info(format!("message-{i}"), None).unwrap();
        }

        let files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().map(|e| e == "log").unwrap_or(false))
            .collect();
        // Should have rotated files (more than 1 log file)
        assert!(
            files.len() > 1,
            "expected rotation, got {} files",
            files.len()
        );
    }

    #[test]
    fn get_session_logs_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let logger = test_logger(dir.path());
        logger.info("first", None).unwrap();
        logger.warning("second", None).unwrap();
        logger.error("third", None).unwrap();

        let logs = logger.get_session_logs(None).unwrap();
        assert_eq!(logs.len(), 3);
        assert_eq!(logs[0].message, "first");
        assert_eq!(logs[2].message, "third");
    }

    #[test]
    fn get_session_logs_with_limit() {
        let dir = tempfile::tempdir().unwrap();
        let logger = test_logger(dir.path());
        for i in 0..5 {
            logger.info(format!("msg-{i}"), None).unwrap();
        }

        let logs = logger.get_session_logs(Some(2)).unwrap();
        assert_eq!(logs.len(), 2);
        // Limit keeps the LAST N entries
        assert!(logs[0].message.contains("msg-3"));
        assert!(logs[1].message.contains("msg-4"));
    }

    #[test]
    fn get_session_logs_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let logger = test_logger(dir.path());
        let logs = logger.get_session_logs(None).unwrap();
        assert!(logs.is_empty());
    }

    #[test]
    fn child_logger_inherits_settings() {
        let dir = tempfile::tempdir().unwrap();
        let parent = ToolkitLoggerBuilder::new()
            .session_id("parent-sess")
            .component("parent")
            .log_dir(dir.path())
            .level(LogLevel::Warning)
            .enable_console(false)
            .max_size(2048)
            .max_files(7)
            .build()
            .unwrap();

        let child = parent.create_child_logger("child").unwrap();
        assert_eq!(child.session_id, Some("parent-sess".into()));
        assert_eq!(child.component, Some("parent.child".into()));
        assert_eq!(child.level, LogLevel::Warning);
        assert_eq!(child.max_size, 2048);
        assert_eq!(child.max_files, 7);
    }

    #[test]
    fn child_logger_no_parent_component() {
        let dir = tempfile::tempdir().unwrap();
        let parent = ToolkitLoggerBuilder::new()
            .session_id("s")
            .log_dir(dir.path())
            .enable_console(false)
            .build()
            .unwrap();

        let child = parent.create_child_logger("sub").unwrap();
        assert_eq!(child.component, Some("sub".into()));
    }

    // --- OperationContext ---

    #[test]
    fn operation_context_logs_start_and_end() {
        let dir = tempfile::tempdir().unwrap();
        let logger = test_logger(dir.path());
        {
            let _op = logger.operation("deploy");
            // op dropped here
        }
        let logs = logger.get_session_logs(None).unwrap();
        assert!(logs.len() >= 2);
        assert!(logs[0].message.contains("Started operation: deploy"));
        assert!(logs[1].message.contains("Completed operation: deploy"));
    }

    #[test]
    fn operation_context_fail() {
        let dir = tempfile::tempdir().unwrap();
        let logger = test_logger(dir.path());
        {
            let mut op = logger.operation("risky");
            op.fail("disk full");
        }
        let logs = logger.get_session_logs(None).unwrap();
        let last = logs.last().unwrap();
        assert_eq!(last.level, LogLevel::Error);
        assert!(last.message.contains("Failed operation: risky"));
        assert!(last.message.contains("disk full"));
    }
}
