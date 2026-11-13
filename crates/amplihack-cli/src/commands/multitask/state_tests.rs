//! Issue #871 — workstream state persistence must be observable.
//!
//! Missing != corrupt: a MISSING state file stays silent, a CORRUPT file
//! returns an error, and a failed state write is returned to its caller.

use super::models::PersistedState;
use super::persistence::{persist_state, read_state};
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

#[test]
fn load_state_missing_file_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("ws-does-not-exist.json");
    let (result, logs) = capture(|| read_state(&missing));
    assert!(result.unwrap().is_none(), "missing file must load as None");
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
    let error = read_state(&corrupt).expect_err("corrupt state must stop execution");
    assert!(error.to_string().contains("ws-42.json"));
}

#[test]
fn load_state_valid_file_returns_some_silently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ws-7.json");
    let state = PersistedState {
        key: "7".to_string(),
        issue: Some(7),
        ..Default::default()
    };
    fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();
    let (result, logs) = capture(|| read_state(&path));
    assert_eq!(result.unwrap().map(|s| s.issue), Some(Some(7)));
    assert!(
        logs.is_empty(),
        "a valid load must be silent; got: {logs:?}"
    );
}

#[cfg(unix)]
#[test]
fn persist_state_failure_is_returned() {
    let base = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut ws = super::models::Workstream::new(
        super::models::WorkstreamKey::new("792").unwrap(),
        Some(792),
        "feature/obs".to_string(),
        "desc".to_string(),
        "task".to_string(),
        "default-workflow".to_string(),
        base.path(),
        state.path(),
    );
    use std::os::unix::fs::PermissionsExt;
    ws.state_file = state.path().join("ws-792.json");
    persist_state(&ws).unwrap();
    let committed = fs::read(&ws.state_file).unwrap();
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = persist_state(&ws);
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let error = result.expect_err("failed state writes must propagate");
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    assert_eq!(fs::read(&ws.state_file).unwrap(), committed);
}
