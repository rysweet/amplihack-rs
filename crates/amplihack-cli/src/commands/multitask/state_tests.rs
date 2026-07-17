//! Issue #871 — workstream state persistence must be observable.
//!
//! Missing != corrupt: a MISSING state file stays silent, a CORRUPT file emits
//! `error!`, and a FAILED checkpoint write is logged instead of silently
//! claiming the workstream is resumable.

use super::models::{PersistedState, Workstream};
use super::persistence::load_state;
use super::state::finalize_workstream;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

#[derive(Default)]
struct CaptureSubscriber {
    lines: Arc<Mutex<Vec<String>>>,
    next_id: Arc<AtomicU64>,
}

impl Subscriber for CaptureSubscriber {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        // In tracing, ERROR < WARN, so `<= WARN` captures both warn! and error!.
        *meta.level() <= tracing::Level::WARN
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut grabber = FieldGrabber::default();
        event.record(&mut grabber);
        let mut line = event.metadata().level().to_string();
        line.push(' ');
        line.push_str(&grabber.fields.join(" "));
        self.lines
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(line);
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
    fn register_callsite(&self, _: &'static Metadata<'static>) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }
    fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
        Some(tracing::metadata::LevelFilter::WARN)
    }
}

#[derive(Default)]
struct FieldGrabber {
    fields: Vec<String>,
}
impl Visit for FieldGrabber {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields.push(format!("{}={value:?}", field.name()));
    }
}

fn capture<T>(op: impl FnOnce() -> T) -> (T, Vec<String>) {
    let sub = CaptureSubscriber::default();
    let lines = Arc::clone(&sub.lines);
    let out = tracing::subscriber::with_default(sub, op);
    let captured = lines.lock().unwrap_or_else(|p| p.into_inner()).clone();
    (out, captured)
}

fn has_level(lines: &[String], level: &str) -> bool {
    lines.iter().any(|l| l.starts_with(level))
}

fn joined(lines: &[String]) -> String {
    lines.join("\n")
}

#[test]
fn load_state_missing_file_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("ws-does-not-exist.json");
    let (result, logs) = capture(|| load_state(&missing));
    assert!(result.is_none(), "missing file must load as None");
    assert!(
        logs.is_empty(),
        "a missing state file is the normal first-run condition and must \
         emit no diagnostics; got: {logs:?}"
    );
}

#[test]
fn load_state_corrupt_file_logs_error() {
    let dir = tempfile::tempdir().unwrap();
    let corrupt = dir.path().join("ws-42.json");
    fs::write(&corrupt, b"{ this is not valid json").unwrap();
    let (result, logs) = capture(|| load_state(&corrupt));
    assert!(result.is_none(), "corrupt file still loads as None");
    assert!(
        has_level(&logs, "ERROR"),
        "a corrupt state file must be surfaced at error! (distinguishable \
         from a missing file); got: {logs:?}"
    );
    assert!(
        joined(&logs).contains("ws-42.json"),
        "the diagnostic must reference the offending path; got: {logs:?}"
    );
}

#[test]
fn load_state_valid_file_returns_some_silently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ws-7.json");
    let state = PersistedState {
        issue: 7,
        ..Default::default()
    };
    fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();
    let (result, logs) = capture(|| load_state(&path));
    assert_eq!(result.map(|s| s.issue), Some(7));
    assert!(
        logs.is_empty(),
        "a valid load must be silent; got: {logs:?}"
    );
}

#[test]
fn persist_state_failure_is_logged() {
    let base = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut ws = Workstream::new(
        792,
        "feature/obs".to_string(),
        "desc".to_string(),
        "task".to_string(),
        "default-workflow".to_string(),
        base.path(),
        state.path(),
    );
    // Point the state file at a path whose parent is a *file*, so the atomic
    // write can never succeed — deterministic for every user (including root),
    // unlike a chmod-based approach.
    let blocker = state.path().join("blocker");
    fs::write(&blocker, b"x").unwrap();
    ws.state_file = blocker.join("ws-792.json");

    let (_, logs) = capture(|| finalize_workstream(&mut ws, 1));
    assert!(
        !logs.is_empty(),
        "a failed checkpoint write must be surfaced, not swallowed by a \
         discarded `persist_state` result"
    );
    assert!(
        joined(&logs).contains("792"),
        "the checkpoint-failure diagnostic must reference the workstream \
         (issue id / path); got: {logs:?}"
    );
}
