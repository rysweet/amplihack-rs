//! Unit tests for `server.rs` (G1): the unit name, how a `systemd-run`
//! result is read, the `PATH` lookup, and every branch of
//! `Placement::launch` against a fake `Launcher`.
//!
//! API under test:
//! - `unit_name(now: i64, n: u32) -> String`
//! - `scope_result(exit: Option<i32>, stderr: &[u8]) -> Scope`
//! - `on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf>`
//! - `Placement::launch(&mut self, socket, now, &dyn Launcher) -> Launched`

use super::server::*;
use super::tmux::SessionState;
use std::cell::RefCell;
use std::ffi::OsStr;
use std::path::Path;

const NOW: i64 = 1_793_349_461;

/// A launcher whose answers are fixed and whose calls are recorded.
struct Fake {
    server: SessionState,
    scope: Result<Scope, String>,
    session: SessionState,
    plain: Result<(), String>,
    calls: RefCell<Vec<String>>,
}

impl Fake {
    fn new(server: SessionState, scope: Scope) -> Self {
        Self {
            server,
            scope: Ok(scope),
            session: SessionState::Absent,
            plain: Ok(()),
            calls: RefCell::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl Launcher for Fake {
    fn server_state(&self) -> SessionState {
        self.calls.borrow_mut().push("list-sessions".into());
        self.server.clone()
    }
    fn scoped(&self, unit: &str) -> Result<Scope, String> {
        self.calls.borrow_mut().push(format!("systemd-run {unit}"));
        self.scope.clone()
    }
    fn has_session(&self) -> SessionState {
        self.calls.borrow_mut().push("has-session".into());
        self.session.clone()
    }
    fn plain(&self) -> Result<(), String> {
        self.calls.borrow_mut().push("new-session".into());
        self.plain.clone()
    }
}

fn started(warning: Option<&str>) -> Launched {
    Launched::Started {
        warning: warning.map(str::to_string),
    }
}

#[test]
fn unit_names_hold_the_time_and_the_counter_only() {
    assert_eq!(
        unit_name(NOW, 1),
        "amplihack-tmux-server-1793349461-1".to_string()
    );
    assert_eq!(unit_name(NOW, 12), "amplihack-tmux-server-1793349461-12");
}

#[test]
fn a_systemd_run_result_gives_its_first_line_status_or_signal() {
    assert_eq!(scope_result(Some(0), b"noise"), Scope::Started);
    let failed = |s: &str| Scope::Failed(s.to_string());
    assert_eq!(
        scope_result(Some(1), b"Failed to connect to bus: No medium found\nmore"),
        failed("systemd-run: Failed to connect to bus: No medium found")
    );
    assert_eq!(scope_result(Some(4), b""), failed("systemd-run exited 4"));
    assert_eq!(
        scope_result(None, b""),
        failed("systemd-run was ended by a signal")
    );
    assert_eq!(
        scope_result(Some(1), b"bad\x1b[31m"),
        failed("systemd-run: bad?[31m")
    );
}

#[test]
fn on_path_finds_only_executable_files_in_absolute_directories() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(b.join("systemd-run")).unwrap();
    let plain = a.join("systemd-run");
    std::fs::write(&plain, "#!/bin/sh\n").unwrap();
    let path = std::env::join_paths([&b, &a]).unwrap();
    assert_eq!(on_path("systemd-run", Some(&path)), None, "not executable");
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(on_path("systemd-run", Some(&path)), Some(plain));
    assert_eq!(on_path("systemd-run", Some(OsStr::new("a:b"))), None);
    assert_eq!(on_path("systemd-run", None), None);
}

#[test]
fn a_running_server_is_used_without_systemd_run() {
    let f = Fake::new(SessionState::Present, Scope::Started);
    let got = Placement::default().launch(None, NOW, &f);
    assert_eq!(got, started(None));
    assert_eq!(f.calls(), ["list-sessions", "new-session"]);
}

#[test]
fn an_absent_server_is_started_in_a_numbered_scope_once_per_socket() {
    let f = Fake::new(SessionState::Absent, Scope::Started);
    let mut p = Placement::default();
    assert_eq!(p.launch(None, NOW, &f), started(None));
    assert_eq!(p.launch(None, NOW, &f), started(None), "joins the server");
    let other = Path::new("/run/user/1000/tmux-other");
    assert_eq!(p.launch(Some(other), NOW, &f), started(None));
    assert_eq!(
        f.calls(),
        [
            "list-sessions",
            "systemd-run amplihack-tmux-server-1793349461-1",
            "new-session",
            "list-sessions",
            "systemd-run amplihack-tmux-server-1793349461-2",
        ]
    );
}

#[test]
fn without_systemd_run_tmux_starts_plainly_with_a_warning() {
    let f = Fake::new(SessionState::Absent, Scope::Missing);
    let got = Placement::default().launch(None, NOW, &f);
    assert_eq!(got, started(Some(MISSING)));
    assert_eq!(f.calls()[2..], ["new-session"]);
}

/// G1-S3: a failed `systemd-run` whose session exists is not launched again.
#[test]
fn a_failed_scope_with_the_session_present_starts_nothing_more() {
    let mut f = Fake::new(SessionState::Absent, Scope::Failed(TIMED_OUT.into()));
    f.session = SessionState::Present;
    let got = Placement::default().launch(None, NOW, &f);
    assert_eq!(got, started(None));
    assert_eq!(f.calls()[2..], ["has-session"], "no second new-session");
}

#[test]
fn a_failed_scope_without_the_session_starts_tmux_once_more() {
    let why = "systemd-run: Failed to connect to bus";
    let f = Fake::new(SessionState::Absent, Scope::Failed(why.into()));
    let got = Placement::default().launch(None, NOW, &f);
    assert_eq!(got, started(Some(why)));
    assert_eq!(f.calls()[2..], ["has-session", "new-session"]);

    let mut f = Fake::new(SessionState::Absent, Scope::Failed(why.into()));
    f.plain = Err("server exited unexpectedly".into());
    let got = Placement::default().launch(None, NOW, &f);
    assert_eq!(got, Launched::Failed("server exited unexpectedly".into()));
}

/// P3: an unknown session state after a failed scope starts nothing more.
#[test]
fn a_failed_scope_with_the_session_unknown_starts_nothing_more() {
    let mut f = Fake::new(SessionState::Absent, Scope::Failed(TIMED_OUT.into()));
    f.session = SessionState::Unknown("tmux could not be run".into());
    let got = Placement::default().launch(None, NOW, &f);
    let reason = "tmux could not be run".to_string();
    assert_eq!(
        got,
        Launched::Unknown {
            reason,
            attempted: true
        }
    );
    assert!(!f.calls().contains(&"new-session".to_string()));
}

/// P3: an unknown server state starts nothing on that socket in the pass.
#[test]
fn an_unknown_server_state_starts_nothing_on_its_socket() {
    let f = Fake::new(SessionState::Unknown("boom".into()), Scope::Started);
    let mut p = Placement::default();
    let want = Launched::Unknown {
        reason: "boom".into(),
        attempted: false,
    };
    assert_eq!(p.launch(None, NOW, &f), want);
    assert_eq!(p.launch(None, NOW, &f), want, "the second resume too");
    assert_eq!(f.calls(), ["list-sessions"]);
}

#[test]
fn a_tmux_refusal_before_systemd_run_is_a_failure() {
    let mut f = Fake::new(SessionState::Absent, Scope::Started);
    f.scope = Err("socket directory is not private".into());
    let got = Placement::default().launch(None, NOW, &f);
    assert_eq!(
        got,
        Launched::Failed("socket directory is not private".into())
    );
}
