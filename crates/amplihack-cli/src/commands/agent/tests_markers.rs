//! Unit tests for `markers.rs`: which files in `ended/` are accepted (S14,
//! A6, step 5b section 6), which are deleted and when, and the `decide` table
//! of design 4.14 with the "active agent wins" rule (data model section 9.3).
//!
//! API under test:
//! - `struct Marker { session_id: String, reason: String, at: i64, path: PathBuf }`
//! - `markers::read(dir: &Path, now: i64) -> Vec<Marker>`: valid markers only;
//!   an invalid direct entry (including a symlink) is unlinked once its
//!   `lstat` mtime is more than 7 days before `now`, and left otherwise;
//!   names starting with `.` are skipped; nothing is followed or recursed
//! - `enum Disposition { End(String), Delete, Keep }` (`Debug`, `PartialEq`)
//! - `markers::decide(m: &Marker, reg: &Registry, alive: &dyn Fn(u32, u64) -> bool,
//!   now: i64) -> Disposition`

use super::markers::*;
use super::registry::Registry;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const ID: &str = "3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57";
const DAY: i64 = 86_400;
/// 2026-10-30T09:00:00Z.
const LAUNCHED: i64 = 1_793_350_800;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Sets the `lstat` mtime of `p` (a symlink itself, not its target).
fn set_mtime(p: &Path, t: i64) {
    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
    let ts = libc::timespec {
        tv_sec: t as libc::time_t,
        tv_nsec: 0,
    };
    let times = [ts, ts];
    // SAFETY: a valid NUL-terminated path and two timespecs.
    let rc = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    assert_eq!(rc, 0, "utimensat {}", p.display());
}

fn ended() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("ended");
    fs::create_dir(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    (tmp, d)
}

fn write(dir: &Path, name: &str, body: &str, mode: u32) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
    p
}

fn valid(id: &str, reason: &str, at: i64) -> String {
    json!({ "session_id": id, "reason": reason, "at": at }).to_string()
}

#[test]
fn a_valid_marker_is_read() {
    let (_t, d) = ended();
    let p = write(
        &d,
        ID,
        &valid(ID, "prompt_input_exit", 1_793_400_000),
        0o600,
    );
    let got = read(&d, now());
    assert_eq!(got.len(), 1);
    let m = &got[0];
    assert_eq!(
        (m.session_id.as_str(), m.reason.as_str()),
        (ID, "prompt_input_exit")
    );
    assert_eq!((m.at, m.path.as_path()), (1_793_400_000, p.as_path()));
    write(&d, ID, &valid(ID, "logout", 5), 0o600);
    assert_eq!(read(&d, now())[0].reason, "logout");
}

#[test]
fn invalid_files_are_ignored_and_kept_while_young() {
    let (_t, d) = ended();
    let other = "6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36";
    let pad = "x".repeat(5000);
    let big = json!({ "session_id": "a4", "reason": "logout", "at": 1, "pad": pad }).to_string();
    let cases = [
        write(&d, ID, &valid(ID, "prompt_input_exit", 1), 0o644),
        write(&d, other, &valid(ID, "prompt_input_exit", 1), 0o600),
        write(&d, "-abc", &valid("-abc", "logout", 1), 0o600),
        write(&d, "a1", &valid("a1", "clear", 1), 0o600),
        write(&d, "a2", &valid("a2", "exit", 1), 0o600),
        write(&d, "a3", "not json", 0o600),
        write(&d, "a4", &big, 0o600),
        write(
            &d,
            "a5",
            "{\"session_id\":\"a5\",\"reason\":\"logout\"}",
            0o600,
        ),
    ];
    assert!(read(&d, now()).is_empty());
    for p in &cases {
        assert!(p.exists(), "{} was deleted while young", p.display());
    }
}

#[test]
fn invalid_files_are_deleted_after_seven_days() {
    let (_t, d) = ended();
    let t = now();
    let old = write(&d, "a3", "not json", 0o600);
    set_mtime(&old, t - 8 * DAY);
    let loose = write(&d, ID, &valid(ID, "logout", 1), 0o644);
    set_mtime(&loose, t - 8 * DAY);
    let young = write(&d, "a4", "not json", 0o600);
    set_mtime(&young, t - 6 * DAY);
    assert!(read(&d, t).is_empty());
    assert!(!old.exists() && !loose.exists());
    assert!(young.exists());
}

#[test]
fn symlinks_are_never_followed_and_removed_only_when_old() {
    let (tmp, d) = ended();
    let target = write(tmp.path(), "elsewhere", &valid(ID, "logout", 1), 0o600);
    let link = d.join(ID);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let t = now();
    assert!(read(&d, t).is_empty(), "a symlinked marker is not accepted");
    assert!(link.symlink_metadata().is_ok(), "a young symlink is left");
    set_mtime(&link, t - 8 * DAY);
    assert!(read(&d, t).is_empty());
    assert!(
        link.symlink_metadata().is_err(),
        "an old symlink is unlinked"
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), valid(ID, "logout", 1));
}

#[test]
fn hidden_names_and_directories_are_skipped() {
    let (_t, d) = ended();
    let t = now();
    let tmpfile = write(
        &d,
        &format!(".{ID}.tmp.123.456"),
        &valid(ID, "logout", 1),
        0o600,
    );
    let sub = d.join("a6");
    fs::create_dir(&sub).unwrap();
    let inner = write(&sub, "a6", &valid("a6", "logout", 1), 0o600);
    set_mtime(&sub, t - 8 * DAY);
    assert!(read(&d, t).is_empty());
    assert!(tmpfile.exists(), "a write in progress is not touched");
    assert!(inner.exists(), "no recursion into a directory");
}

fn registry(agents: serde_json::Value) -> Registry {
    serde_json::from_value(json!({ "version": 1, "agents": agents })).unwrap()
}

fn entry(state: &str, recorded: Option<(u32, u64)>) -> serde_json::Value {
    let ts = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0)
            .unwrap()
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    };
    let mut e = json!({
        "state": state, "tmux_session": "s", "tmux_window": "w", "launch_dir": "/w",
        "conversation_id": ID, "registered_at": ts(LAUNCHED - 60),
        "launched_at": ts(LAUNCHED), "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d"
    });
    match state {
        "ended" => {
            e["ended_at"] = json!(ts(LAUNCHED + 5));
            e["ended_by"] = json!("wrapper");
        }
        "stopped" => e["alert"] = json!("stopped: restart limit (3 restarts in the last hour)"),
        _ => {}
    }
    if let Some((pid, start)) = recorded {
        e["claude_pid"] = json!(pid);
        e["proc_start"] = json!(start);
    }
    e
}

fn marker(at: i64) -> Marker {
    Marker {
        session_id: ID.into(),
        reason: "prompt_input_exit".into(),
        at,
        path: PathBuf::from(format!("/nonexistent/ended/{ID}")),
    }
}

const NONE_ALIVE: &dyn Fn(u32, u64) -> bool = &|_, _| false;
const ALL_ALIVE: &dyn Fn(u32, u64) -> bool = &|_, _| true;

#[test]
fn a_later_marker_ends_the_active_agent() {
    let reg = registry(json!({ "area": entry("active", None) }));
    let now = LAUNCHED + 100;
    assert_eq!(
        decide(&marker(LAUNCHED + 10), &reg, NONE_ALIVE, now),
        Disposition::End("area".into())
    );
    let reg = registry(json!({ "area": entry("active", Some((5000, 77))) }));
    let alive = |pid: u32, start: u64| (pid, start) == (5000, 76);
    assert_eq!(
        decide(&marker(LAUNCHED + 10), &reg, &alive, now),
        Disposition::End("area".into())
    );
}

#[test]
fn a_marker_is_kept_while_the_recorded_process_lives() {
    let reg = registry(json!({ "area": entry("active", Some((5000, 77))) }));
    let alive = |pid: u32, start: u64| (pid, start) == (5000, 77);
    assert_eq!(
        decide(&marker(LAUNCHED + 10), &reg, &alive, LAUNCHED + 100),
        Disposition::Keep
    );
}

#[test]
fn a_stale_marker_is_deleted() {
    let reg = registry(json!({ "area": entry("active", None) }));
    let now = LAUNCHED + 100;
    assert_eq!(
        decide(&marker(LAUNCHED), &reg, NONE_ALIVE, now),
        Disposition::Delete,
        "same second"
    );
    assert_eq!(
        decide(&marker(LAUNCHED - 30), &reg, NONE_ALIVE, now),
        Disposition::Delete
    );
}

#[test]
fn a_marker_for_an_agent_that_is_not_active_is_deleted() {
    for state in ["ended", "stopped"] {
        let reg = registry(json!({ "area": entry(state, None) }));
        let got = decide(&marker(LAUNCHED + 10), &reg, ALL_ALIVE, LAUNCHED + 100);
        assert_eq!(got, Disposition::Delete, "{state}");
    }
}

#[test]
fn an_unmatched_marker_is_kept_for_seven_days() {
    let mut other = entry("active", None);
    other["conversation_id"] = json!("6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36");
    let reg = registry(json!({ "other": other }));
    let at = LAUNCHED;
    assert_eq!(
        decide(&marker(at), &reg, NONE_ALIVE, at + 6 * DAY),
        Disposition::Keep
    );
    assert_eq!(
        decide(&marker(at), &reg, NONE_ALIVE, at + 8 * DAY),
        Disposition::Delete
    );
    let empty = registry(json!({}));
    assert_eq!(
        decide(&marker(at), &empty, NONE_ALIVE, at + 60),
        Disposition::Keep
    );
}

#[test]
fn the_active_agent_is_matched_before_an_ended_one() {
    let reg = registry(json!({
        "a-old": entry("ended", None),
        "b-new": entry("active", None),
        "c-stopped": entry("stopped", None)
    }));
    let got = decide(&marker(LAUNCHED + 10), &reg, NONE_ALIVE, LAUNCHED + 100);
    assert_eq!(got, Disposition::End("b-new".into()));
}
