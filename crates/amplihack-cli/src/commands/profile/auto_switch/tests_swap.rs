//! Swap bookkeeping failures, rerun-buffer cleanup, usage-file validation and
//! file modes (issue #1569). Unit tests on a temporary profile root.

use super::super::fsx;
use super::super::liveness::Liveness;
use super::super::state::{Journal, Op, PHASE_INCONSISTENT, State};
use super::{MoveError, cleanup_rerun, fail, recover, rerun_owner, same_version, swap_allowed};
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

/// Review 17d #5: a damaged usage.json is reported (by position only, never
/// by content) to callers that may print, while `load_usage` stays silent.
#[test]
fn damaged_usage_is_reported_without_its_contents() {
    let r = root();
    r.state.ensure().unwrap();
    assert!(
        r.state.try_load_usage().unwrap().accounts.is_empty(),
        "a missing file is empty, not an error"
    );
    let path = r.state.dir.join("usage.json");
    let cases = [
        (
            "{\"version\":1,\"accounts\":{\"SECRET-SENTINEL\"",
            "line 1 column",
        ),
        ("{\"version\":99,\"accounts\":{}}", "unknown version"),
        (
            "{\"version\":1,\"accounts\":{\"../x\":{\"observed_at\":1,\"source\":\"statusline\"}}}",
            "invalid profile name",
        ),
    ];
    for (body, want) in cases {
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let msg = r.state.try_load_usage().unwrap_err().to_string();
        assert!(msg.contains("usage.json") && msg.contains(want), "{msg}");
        assert!(!msg.contains("SECRET-SENTINEL"), "contents leaked: {msg}");
        assert!(r.state.load_usage().accounts.is_empty());
        assert!(r.state.load_usage_or_warn().accounts.is_empty());
    }
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

#[test]
fn keep_mode_write_drops_setuid_setgid_and_sticky_bits() {
    let r = root();
    let dir = r.state.root.join("X");
    fs::create_dir(&dir).unwrap();
    let file = dir.join(".claude.json");
    fs::write(&file, b"{}").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o5755)).unwrap();
    assert_eq!(fs::metadata(&file).unwrap().mode() & 0o7000, 0o5000);
    fsx::write_atomic_keep_mode(&dir, ".claude.json", b"{\"a\":1}").unwrap();
    assert_eq!(fs::metadata(&file).unwrap().mode() & 0o7777, 0o755);
}

/// Sets `path`'s mtime to exactly `sec`.`nsec`.
fn set_mtime(path: &std::path::Path, sec: i64, nsec: i64) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: sec as libc::time_t,
            tv_nsec: nsec as _,
        },
    ];
    // SAFETY: `c` is a valid NUL-terminated path and `times` has two entries.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "utimensat: {}", std::io::Error::last_os_error());
}

#[test]
fn a_change_of_whole_seconds_alone_counts_as_a_new_version() {
    let r = root();
    let file = r.state.root.join(".claude.json");
    fs::write(&file, b"{}").unwrap();
    set_mtime(&file, 1_700_000_000, 123_456_789);
    let before = fs::symlink_metadata(&file).unwrap();
    assert!(same_version(&before, &fs::symlink_metadata(&file).unwrap()));

    // Same inode, size and nanoseconds; only the seconds differ.
    set_mtime(&file, 1_700_000_001, 123_456_789);
    let now = fs::symlink_metadata(&file).unwrap();
    assert_eq!(now.mtime_nsec(), before.mtime_nsec());
    assert_eq!((now.ino(), now.len()), (before.ino(), before.len()));
    assert!(!same_version(&before, &now));

    // And the nanoseconds alone.
    set_mtime(&file, 1_700_000_000, 987_654_321);
    assert!(!same_version(
        &before,
        &fs::symlink_metadata(&file).unwrap()
    ));
}

// --- Status-line pre-check on an overflow profile (issue #1606) ------------
//
// A session on a credit account at its plan limit is not worth the full
// check (recovery and the `/proc` scan): its own reading does not count.
// Only a spend block, or a plan profile it may move back to, is.

mod overflow_precheck {
    use super::super::super::config::Config;
    use super::super::super::overflow_inputs::OverflowInputs;
    use super::super::super::usage::{Reading, ReadingSource, Window};
    use super::super::overflow_may_move;
    use crate::commands::profile::name::ProfileName;
    use crate::commands::profile::overflow::{Gate, OverflowEntry};
    use std::collections::BTreeMap;

    const NOW: i64 = 1_760_000_000;
    const GATE: Gate = Gate {
        wait: 1800,
        unknown_reset: false,
        session_at: 85,
        week_at: 90,
    };

    fn n(s: &str) -> ProfileName {
        ProfileName::parse(s).unwrap()
    }

    fn reading(five: f64, seven: f64) -> Reading {
        let w = |pct, resets_in| {
            Some(Window {
                used_percentage: pct,
                resets_at: NOW + resets_in,
            })
        };
        Reading {
            five_hour: w(five, 4 * 3600),
            seven_day: w(seven, 86_400),
            observed_at: NOW - 10,
            source: ReadingSource::Statusline,
        }
    }

    fn cfg(threshold: u8) -> Config {
        Config {
            enabled: true,
            threshold,
            exclude: Vec::new(),
            overflow: vec![OverflowEntry::new(n("cred"), 1)],
        }
    }

    fn inputs(blocked: &[&str]) -> OverflowInputs {
        OverflowInputs {
            order: vec![n("cred")],
            blocked: blocked.iter().map(|b| b.to_string()).collect(),
            gate: Some(GATE),
        }
    }

    fn readings(work: Reading) -> BTreeMap<String, Reading> {
        [("work", work), ("cred", reading(100.0, 100.0))]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[test]
    fn a_credit_account_at_its_plan_limit_returns_early() {
        let r = readings(reading(95.0, 10.0));
        assert!(!overflow_may_move(
            &n("cred"),
            &cfg(90),
            &inputs(&[]),
            &r,
            NOW
        ));
    }

    #[test]
    fn a_spend_block_is_worth_the_full_check() {
        let r = readings(reading(95.0, 10.0));
        assert!(overflow_may_move(
            &n("cred"),
            &cfg(90),
            &inputs(&["cred"]),
            &r,
            NOW
        ));
    }

    #[test]
    fn a_plan_profile_to_move_back_to_is_worth_the_full_check() {
        // 75%: over a 70 threshold, but capacity by the gate's 85/90.
        let r = readings(reading(75.0, 10.0));
        assert!(overflow_may_move(
            &n("cred"),
            &cfg(70),
            &inputs(&[]),
            &r,
            NOW
        ));
    }
}
