//! Integration tests for `amplihack-session::toolkit` and `logger`
//! (TDD: failing). Ports `tests/test_toolkit_integration.py` to Rust.

use amplihack_session::{LogLevel, SessionError, SessionToolkit, ToolkitLogger, quick_session};
use serde_json::json;
use std::path::PathBuf;
use tempfile::TempDir;

fn td() -> TempDir {
    tempfile::tempdir_in(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).unwrap()
}

// ---------- ToolkitLogger ----------

#[test]
fn logger_writes_json_line_per_call() {
    let dir = td();
    let logger = ToolkitLogger::builder()
        .session_id("sess-log-1")
        .component("test")
        .log_dir(dir.path())
        .level(LogLevel::Debug)
        .enable_console(false)
        .enable_file(true)
        .build()
        .unwrap();

    logger.info("hello", Some(json!({"k": 1}))).unwrap();
    logger.warning("careful", None).unwrap();
    logger.error("boom", Some(json!({"code": 42}))).unwrap();

    let entries = logger.get_session_logs(None).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].level, LogLevel::Info);
    assert_eq!(entries[1].level, LogLevel::Warning);
    assert_eq!(entries[2].level, LogLevel::Error);
    assert_eq!(entries[0].message, "hello");
    assert_eq!(entries[2].metadata, json!({"code": 42}));
}

#[test]
fn logger_get_session_logs_respects_limit() {
    let dir = td();
    let logger = ToolkitLogger::builder()
        .session_id("sess-log-2")
        .log_dir(dir.path())
        .enable_console(false)
        .enable_file(true)
        .build()
        .unwrap();

    for i in 0..5 {
        logger.info(format!("msg-{i}"), None).unwrap();
    }
    let last_two = logger.get_session_logs(Some(2)).unwrap();
    assert_eq!(last_two.len(), 2);
    assert_eq!(last_two.last().unwrap().message, "msg-4");
}

#[test]
fn logger_operation_emits_start_and_end_with_duration() {
    let dir = td();
    let logger = ToolkitLogger::builder()
        .session_id("sess-log-op")
        .log_dir(dir.path())
        .enable_console(false)
        .enable_file(true)
        .build()
        .unwrap();

    {
        let _op = logger.operation("import_data");
        std::thread::sleep(std::time::Duration::from_millis(5));
    } // dropped here -> emits end log

    let entries = logger.get_session_logs(None).unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.operation.as_deref() == Some("import_data")),
        "operation context should annotate emitted entries"
    );
    assert!(
        entries.iter().any(|e| e.duration_secs.unwrap_or(0.0) > 0.0),
        "operation end log should have duration_secs"
    );
}

#[test]
fn logger_rotates_when_size_exceeded() {
    let dir = td();
    let logger = ToolkitLogger::builder()
        .session_id("sess-log-rot")
        .log_dir(dir.path())
        .enable_console(false)
        .enable_file(true)
        .max_size(256) // tiny cap to force rotation
        .build()
        .unwrap();

    for i in 0..50 {
        logger
            .info(
                format!("entry-{i}-with-some-padding-content-xxxxxxxxxxxxxxxxxxxxxxxxxx"),
                None,
            )
            .unwrap();
    }
    let logs: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "log"))
        .collect();
    assert!(
        logs.len() > 1,
        "expected size-based rotation to produce >1 log file, got {logs:?}"
    );
}

#[test]
fn child_logger_inherits_session_id() {
    let dir = td();
    let parent = ToolkitLogger::builder()
        .session_id("sess-parent")
        .component("parent")
        .log_dir(dir.path())
        .enable_console(false)
        .enable_file(true)
        .build()
        .unwrap();
    let child = parent.create_child_logger("child").unwrap();
    assert_eq!(child.session_id.as_deref(), Some("sess-parent"));
    assert_eq!(child.component.as_deref(), Some("parent.child"));
}

// ---------- SessionToolkit ----------

#[test]
fn toolkit_create_and_list_sessions() {
    let dir = td();
    let mut tk = SessionToolkit::new(dir.path(), true, "INFO").unwrap();
    let id = tk.create_session("my-task", None, None).unwrap();
    let listed = tk.list_sessions(false).unwrap();
    assert!(
        listed
            .iter()
            .any(|v| v.get("session_id").and_then(|s| s.as_str()) == Some(&id))
    );
}

#[test]
fn toolkit_save_current_returns_false_without_session() {
    let dir = td();
    let mut tk = SessionToolkit::new(dir.path(), true, "INFO").unwrap();
    assert!(!tk.save_current().unwrap());
}

#[test]
fn toolkit_export_then_import_roundtrips() {
    let dir = td();
    let export = dir.path().join("exported.json");
    let mut tk = SessionToolkit::new(dir.path().join("rt1"), true, "INFO").unwrap();
    let id = tk.create_session("portable", None, None).unwrap();
    tk.manager_mut().save_session(&id, true).unwrap();
    tk.export_session(&id, &export).unwrap();
    assert!(export.exists());

    // Fresh toolkit in a different runtime dir -> import.
    let mut tk2 = SessionToolkit::new(dir.path().join("rt2"), true, "INFO").unwrap();
    let imported_id = tk2.import_session(&export).unwrap();
    assert_eq!(imported_id, id);
    assert!(tk2.manager().session_file_path(&id).exists());
}

#[test]
fn toolkit_import_rejects_bad_session_id() {
    let dir = td();
    // Hand-craft an export file with a malicious session_id.
    let bad = dir.path().join("bad.json");
    std::fs::write(
        &bad,
        r#"{"session_id":"../escape","state":{},"config":{},"command_history":[],"metadata":{}}"#,
    )
    .unwrap();
    let mut tk = SessionToolkit::new(dir.path().join("rt"), true, "INFO").unwrap();
    let err = tk.import_session(&bad).unwrap_err();
    matches!(err, SessionError::InvalidSessionId(_));
}

#[test]
fn quick_session_runs_closure_and_persists() {
    let dir = td();
    // quick_session runs in the current dir; we work inside `dir`.
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let result = quick_session("quick-task", |tk, sid| {
        assert!(!sid.is_empty());
        tk.save_current()?;
        Ok::<_, SessionError>(42_u32)
    });
    std::env::set_current_dir(prev).unwrap();
    assert_eq!(result.unwrap(), 42);
}
