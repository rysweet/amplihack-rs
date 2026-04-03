//! Optional JSONL trace logger with automatic token sanitization.
//!
//! Ported from `amplihack/tracing/trace_logger.py`.
//!
//! - **Opt-in** by default (must explicitly enable).
//! - **Zero overhead** when disabled.
//! - **Thread-safe** via `Mutex` on the file handle.
//! - **Security-first**: sensitive-looking values are automatically redacted.

use chrono::Utc;
use regex::Regex;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// Default trace-file location under the user's home directory.
pub const DEFAULT_TRACE_SUBPATH: &str = ".amplihack/trace.jsonl";

/// Regex matching common secret/token patterns to redact.
static SENSITIVE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(token|secret|password|api[_-]?key|auth|credential|bearer)")
        .expect("SENSITIVE_KEY regex is valid")
});

/// Optional JSONL trace logger with automatic token sanitization.
///
/// # Usage
///
/// ```no_run
/// use amplihack_utils::trace_logger::TraceLogger;
///
/// let logger = TraceLogger::from_env();
/// if logger.is_enabled() {
///     logger.log(&serde_json::json!({"event": "start"}));
/// }
/// ```
pub struct TraceLogger {
    enabled: bool,
    log_file: Option<PathBuf>,
    handle: Mutex<Option<File>>,
}

impl TraceLogger {
    /// Create a new `TraceLogger`.
    ///
    /// When `enabled` is `true`, `log_file` should be `Some`.
    pub fn new(enabled: bool, log_file: Option<PathBuf>) -> Self {
        let handle = if enabled {
            log_file.as_deref().and_then(|p| open_log_file(p).ok())
        } else {
            None
        };
        Self {
            enabled,
            log_file,
            handle: Mutex::new(handle),
        }
    }

    /// Create a `TraceLogger` from environment variables.
    ///
    /// | Variable | Purpose |
    /// |---|---|
    /// | `AMPLIHACK_TRACE_LOGGING` (or `CLAUDE_TRACE_ENABLED`) | `"true"` / `"1"` / `"yes"` to enable |
    /// | `AMPLIHACK_TRACE_FILE` (or `CLAUDE_TRACE_FILE`) | Override log-file path |
    pub fn from_env() -> Self {
        let enabled_str = std::env::var("CLAUDE_TRACE_ENABLED")
            .or_else(|_| std::env::var("AMPLIHACK_TRACE_LOGGING"))
            .unwrap_or_default()
            .to_lowercase();

        let enabled = matches!(enabled_str.as_str(), "true" | "1" | "yes");

        let log_file = if enabled {
            let explicit = std::env::var("CLAUDE_TRACE_FILE")
                .or_else(|_| std::env::var("AMPLIHACK_TRACE_FILE"))
                .ok()
                .map(PathBuf::from);
            Some(explicit.unwrap_or_else(default_trace_file))
        } else {
            None
        };

        Self::new(enabled, log_file)
    }

    /// Whether trace logging is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The path to the log file, if configured.
    pub fn log_file(&self) -> Option<&Path> {
        self.log_file.as_deref()
    }

    /// Log a trace event as a single JSONL line.
    ///
    /// - When disabled this is a no-op.
    /// - Automatically injects a `timestamp` field if missing.
    /// - Redacts values whose keys look like secrets or tokens.
    pub fn log(&self, data: &Value) {
        if !self.enabled {
            return;
        }

        let mut guard = match self.handle.lock() {
            Ok(g) => g,
            Err(_) => return, // poisoned mutex – silently skip
        };
        let file = match guard.as_mut() {
            Some(f) => f,
            None => return,
        };

        let mut entry = match data {
            Value::Object(map) => Value::Object(map.clone()),
            other => {
                let mut m = serde_json::Map::new();
                m.insert("data".into(), other.clone());
                Value::Object(m)
            }
        };

        // Inject timestamp if absent.
        if let Value::Object(ref mut map) = entry {
            map.entry("timestamp")
                .or_insert_with(|| Value::String(Utc::now().to_rfc3339()));
        }

        let sanitized = sanitize_value(&entry);

        if let Ok(line) = serde_json::to_string(&sanitized) {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
}

/// Resolve the default trace-file path.
fn default_trace_file() -> PathBuf {
    #[cfg(unix)]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(not(unix))]
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);

    home.unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_TRACE_SUBPATH)
}

/// Open (or create) the log file in append mode, creating parent dirs.
fn open_log_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Recursively sanitize a JSON value, redacting sensitive-looking keys.
fn sanitize_value(val: &Value) -> Value {
    match val {
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                if SENSITIVE_KEY.is_match(k) {
                    out.insert(k.clone(), Value::String("[REDACTED]".into()));
                } else {
                    out.insert(k.clone(), sanitize_value(v));
                }
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sanitize_value).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn disabled_logger_is_noop() {
        let logger = TraceLogger::new(false, None);
        assert!(!logger.is_enabled());
        // Should not panic.
        logger.log(&serde_json::json!({"event": "test"}));
    }

    #[test]
    fn enabled_logger_writes_jsonl() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trace.jsonl");
        let logger = TraceLogger::new(true, Some(path.clone()));

        logger.log(&serde_json::json!({"event": "hello"}));
        logger.log(&serde_json::json!({"event": "world"}));

        drop(logger);

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("hello"));
        assert!(lines[1].contains("world"));
    }

    #[test]
    fn timestamp_injected_automatically() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("ts.jsonl");
        let logger = TraceLogger::new(true, Some(path.clone()));

        logger.log(&serde_json::json!({"event": "ts_test"}));
        drop(logger);

        let content = fs::read_to_string(&path).unwrap();
        let val: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert!(val.get("timestamp").is_some());
    }

    #[test]
    fn sensitive_keys_redacted() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("redact.jsonl");
        let logger = TraceLogger::new(true, Some(path.clone()));

        logger.log(&serde_json::json!({
            "event": "auth",
            "api_key": "sk-secret-123",
            "password": "hunter2",
            "safe_field": "visible"
        }));
        drop(logger);

        let content = fs::read_to_string(&path).unwrap();
        let val: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(val["api_key"], "[REDACTED]");
        assert_eq!(val["password"], "[REDACTED]");
        assert_eq!(val["safe_field"], "visible");
    }

    #[test]
    fn nested_sensitive_keys_redacted() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested.jsonl");
        let logger = TraceLogger::new(true, Some(path.clone()));

        logger.log(&serde_json::json!({
            "config": {
                "auth_token": "secret",
                "name": "ok"
            }
        }));
        drop(logger);

        let content = fs::read_to_string(&path).unwrap();
        let val: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(val["config"]["auth_token"], "[REDACTED]");
        assert_eq!(val["config"]["name"], "ok");
    }

    #[test]
    fn from_env_disabled_by_default() {
        // SAFETY: Tests run single-threaded per module; env var mutation is contained.
        unsafe {
            std::env::remove_var("CLAUDE_TRACE_ENABLED");
            std::env::remove_var("AMPLIHACK_TRACE_LOGGING");
        }
        let logger = TraceLogger::from_env();
        assert!(!logger.is_enabled());
    }
}
