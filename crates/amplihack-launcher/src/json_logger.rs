//! JSON-based structured logging for auto-mode events.
//!
//! Writes one JSON object per line (JSONL) to `auto.jsonl` for easy
//! parsing and analysis by external tools.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::Value;

/// Structured JSONL logger for auto-mode events.
pub struct JsonLogger {
    log_file: PathBuf,
}

impl JsonLogger {
    /// Create a new logger writing to `<log_dir>/auto.jsonl`.
    ///
    /// Creates `log_dir` if it does not exist.
    pub fn new(log_dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(log_dir)?;
        Ok(Self {
            log_file: log_dir.join("auto.jsonl"),
        })
    }

    /// Log a structured event.
    ///
    /// Each call appends exactly one JSON line.
    ///
    /// # Parameters
    /// - `event_type`: e.g. `"turn_start"`, `"turn_complete"`, `"error"`.
    /// - `data`: optional extra key-value pairs merged into the event.
    /// - `level`: `"INFO"`, `"WARNING"`, or `"ERROR"`.
    pub fn log_event(
        &self,
        event_type: &str,
        data: Option<&HashMap<String, Value>>,
        level: &str,
    ) {
        let mut event = serde_json::Map::new();
        event.insert(
            "timestamp".into(),
            Value::String(Utc::now().to_rfc3339()),
        );
        event.insert("level".into(), Value::String(level.into()));
        event.insert("event".into(), Value::String(event_type.into()));

        if let Some(extra) = data {
            for (k, v) in extra {
                event.insert(k.clone(), v.clone());
            }
        }

        if let Err(e) = self.write_line(&Value::Object(event)) {
            eprintln!("Warning: Failed to write JSON log: {e}");
        }
    }

    /// Convenience: log an INFO event.
    pub fn info(&self, event_type: &str, data: Option<&HashMap<String, Value>>) {
        self.log_event(event_type, data, "INFO");
    }

    /// Convenience: log an ERROR event.
    pub fn error(&self, event_type: &str, data: Option<&HashMap<String, Value>>) {
        self.log_event(event_type, data, "ERROR");
    }

    /// Path to the log file.
    pub fn log_path(&self) -> &Path {
        &self.log_file
    }

    fn write_line(&self, value: &Value) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)?;
        serde_json::to_writer(&mut file, value)?;
        writeln!(file)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_logger() -> (tempfile::TempDir, JsonLogger) {
        let dir = tempfile::tempdir().unwrap();
        let logger = JsonLogger::new(dir.path()).unwrap();
        (dir, logger)
    }

    #[test]
    fn log_event_creates_file_and_writes_jsonl() {
        let (_dir, logger) = setup_logger();

        logger.log_event("turn_start", None, "INFO");
        logger.log_event("turn_complete", None, "INFO");

        let content = fs::read_to_string(logger.log_path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["event"], "turn_start");
        assert_eq!(first["level"], "INFO");
        assert!(first["timestamp"].is_string());
    }

    #[test]
    fn log_event_merges_data() {
        let (_dir, logger) = setup_logger();

        let mut data = HashMap::new();
        data.insert("turn".into(), Value::Number(5.into()));
        data.insert("phase".into(), Value::String("building".into()));

        logger.log_event("turn_start", Some(&data), "INFO");

        let content = fs::read_to_string(logger.log_path()).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["turn"], 5);
        assert_eq!(entry["phase"], "building");
    }

    #[test]
    fn info_helper() {
        let (_dir, logger) = setup_logger();
        logger.info("agent_invoked", None);

        let content = fs::read_to_string(logger.log_path()).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["level"], "INFO");
    }

    #[test]
    fn error_helper() {
        let (_dir, logger) = setup_logger();
        logger.error("crash", None);

        let content = fs::read_to_string(logger.log_path()).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["level"], "ERROR");
    }

    #[test]
    fn creates_log_dir_if_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        let logger = JsonLogger::new(&nested).unwrap();
        logger.info("test", None);
        assert!(logger.log_path().exists());
    }
}
