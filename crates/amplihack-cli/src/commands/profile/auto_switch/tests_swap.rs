//! Swap bookkeeping failures, rerun-buffer cleanup, usage-file validation and
//! file modes (issue #1569). Unit tests on a temporary profile root.

use super::super::fsx;
use super::super::liveness::Liveness;
use super::super::state::{Journal, Op, PHASE_INCONSISTENT, State};
use super::{MoveError, cleanup_rerun, fail, recover, rerun_owner, swap_allowed};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

struct Root {
    _tmp: tempfile::TempDir,
    state: State,
}

fn root() -> Root {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let state = State::new(tmp.path()).unwrap();
    state.ensure().unwrap();
    Root { _tmp: tmp, state }
}

fn journal() -> Journal {
    Journal {
        version: 1,
        op: Op::Swap,
        dir: "X".into(),
        from: "X".into(),
        from_present: true,
        to: "Y".into(),
        phase: "J0".into(),
        started_at: 1,
        pid: 1,
        fingerprint: None,
        refresh_lock_ino: None,
    }
}

/// Permission bits do not stop root, so these tests need an ordinary user.
fn unprivileged() -> bool {
    fsx::euid() != 0
}

fn read_only(state: &State) {
    fs::set_permissions(&state.dir, fs::Permissions::from_mode(0o500)).unwrap();
}

fn writable(state: &State) {
    fs::set_permissions(&state.dir, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn failed_journal_save_is_named_in_the_error() {
    if !unprivileged() {
        return;
    }
    let r = root();
    read_only(&r.state);
    let mut j = journal();
    let e = fail(&r.state, &mut j, MoveError::Exists, &[]).to_string();
    writable(&r.state);
    assert_eq!(j.phase, PHASE_INCONSISTENT);
    assert!(e.contains("already exists"), "{e}");
    assert!(e.contains("could not save the journal"), "{e}");
}

#[test]
fn failed_disable_never_claims_swaps_are_disabled() {
    if !unprivileged() {
        return;
    }
    let r = root();
    read_only(&r.state);
    let mut j = journal();
    let e = fail(&r.state, &mut j, MoveError::Unsupported, &[]).to_string();
    writable(&r.state);
    assert!(
        e.contains("could not disable swaps for profile \"X\""),
        "{e}"
    );
    assert!(!e.contains("swaps disabled"), "{e}");
    assert!(!super::swaps_disabled(&r.state, "X"));
}

#[test]
fn disabled_swaps_are_reported_when_the_marker_is_written() {
    let r = root();
    let mut j = journal();
    let e = fail(&r.state, &mut j, MoveError::Unsupported, &[]).to_string();
    assert!(e.contains("swaps disabled for profile \"X\""), "{e}");
    assert!(super::swaps_disabled(&r.state, "X"));
}

#[test]
fn failed_journal_delete_is_named_in_the_error() {
    if !unprivileged() {
        return;
    }
    let r = root();
    r.state.save_journal(&journal()).unwrap();
    read_only(&r.state);
    let mut j = journal();
    let e = fail(&r.state, &mut j, MoveError::Other("boom".into()), &[]).to_string();
    writable(&r.state);
    assert!(e.contains("boom"), "{e}");
    assert!(e.contains("could not delete the journal"), "{e}");
}

#[test]
fn liveness_unsupported_blocks_swap() {
    let r = root();
    let x = ProfileName::parse("X").unwrap();
    assert!(!swap_allowed(&r.state, &Liveness::Unsupported, &x));
    assert!(swap_allowed(
        &r.state,
        &Liveness::Known(BTreeSet::new()),
        &x
    ));
    // Everything live: swaps may run, but no unhosted candidate is eligible.
    assert!(Liveness::Everything.is_live(&x));
}

#[test]
fn rerun_buffer_names_give_their_supervisor() {
    let pid = |s: &str| rerun_owner(std::ffi::OsStr::new(s));
    assert_eq!(pid("1234-1.out"), Some(1234));
    assert_eq!(pid("1234-12.out"), Some(1234));
    for bad in [
        "1234.out",
        "x-1.out",
        "1234-x.out",
        "1234-1.txt",
        "0-1.out",
        "-1-1.out",
    ] {
        assert_eq!(pid(bad), None, "{bad}");
    }
}

#[test]
fn recover_sweeps_dead_pid_rerun_files() {
    let r = root();
    let rerun = r.state.dir.join("rerun");
    r.state.ensure_sub(&rerun).unwrap();
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    let mine = std::process::id();
    let files = [
        format!("{dead}-1.out"),
        format!("{mine}-1.out"),
        "notes.txt".to_string(),
    ];
    for f in &files {
        fs::write(rerun.join(f), b"agent output").unwrap();
    }
    cleanup_rerun(&r.state);
    assert!(!rerun.join(&files[0]).exists(), "dead supervisor's buffer");
    assert!(
        rerun.join(&files[1]).exists(),
        "running supervisor's buffer"
    );
    assert!(
        rerun.join(&files[2]).exists(),
        "not a rerun buffer, and new"
    );
}

#[test]
fn recover_without_hosts_skips_scan_but_still_sweeps_rerun() {
    let r = root();
    let rerun = r.state.dir.join("rerun");
    r.state.ensure_sub(&rerun).unwrap();
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    let buf = rerun.join(format!("{dead}-1.out"));
    fs::write(&buf, b"agent output").unwrap();
    assert!(
        matches!(recover(&r.state), Ok(None)),
        "no hosted logins and no journal: no /proc scan"
    );
    assert!(!buf.exists(), "the early return still sweeps rerun buffers");
}

#[test]
fn usage_rejects_traversal_key() {
    let r = root();
    let body = |key: &str| {
        format!(
            "{{\"version\":1,\"accounts\":{{\"X\":{{\"observed_at\":1,\"source\":\"statusline\"}},\
             \"{key}\":{{\"observed_at\":1,\"source\":\"statusline\"}}}}}}"
        )
    };
    let path = r.state.dir.join("usage.json");
    for key in ["../evil", "a/b", ".hidden", ""] {
        fs::write(&path, body(key)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            r.state.load_usage().accounts.is_empty(),
            "{key:?}: one bad key makes the file count as empty"
        );
    }
    fs::write(&path, body("Y")).unwrap();
    assert_eq!(r.state.load_usage().accounts.len(), 2);
}

#[test]
fn keep_mode_write_keeps_an_existing_mode_and_makes_new_files_private() {
    let r = root();
    let dir = r.state.root.join("X");
    fs::create_dir(&dir).unwrap();
    let old = dir.join(".claude.json");
    fs::write(&old, b"{}").unwrap();
    fs::set_permissions(&old, fs::Permissions::from_mode(0o644)).unwrap();
    fsx::write_atomic_keep_mode(&dir, ".claude.json", b"{\"a\":1}").unwrap();
    assert_eq!(fs::metadata(&old).unwrap().mode() & 0o7777, 0o644);
    assert_eq!(fs::read(&old).unwrap(), b"{\"a\":1}");

    fsx::write_atomic_keep_mode(&dir, "settings.json", b"{}").unwrap();
    let new = fs::metadata(dir.join("settings.json")).unwrap();
    assert_eq!(new.mode() & 0o7777, 0o600);

    fsx::write_private_atomic(&dir, ".claude.json", b"{}").unwrap();
    assert_eq!(fs::metadata(&old).unwrap().mode() & 0o7777, 0o600);
}
