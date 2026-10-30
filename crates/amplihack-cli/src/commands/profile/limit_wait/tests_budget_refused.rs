//! Issue #1761: a budget record that cannot be used, and the per-step
//! fallback (D3, SR-8, SR-13, SR-14), using the helpers of
//! `tests_budget.rs`. A record that fails a check is refused and left
//! exactly as it was. A step that falls back keeps its own count from the
//! amount left at its last good read (the whole budget if it had none),
//! stays in the fallback, touches no file again and writes one fixed
//! warning. With no run id, an invalid one or an unusable HOME, nothing is
//! opened or created.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_budget_refused;`.

use super::budget::Budget;
use super::tests_budget::{
    NOT_SET, NOT_VALID, RUN, UNUSABLE, add, budget, home, left, record, rerun,
};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// Runs `f` on another thread and fails if it takes more than 10 s, so a
/// record that blocks (a FIFO) fails the test instead of hanging it.
fn within<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the budget blocked")
}

/// The record at `rec` cannot be used: the first budget check falls back
/// with one warning, starts from the whole budget, and leaves the record
/// exactly as it was.
fn assert_unusable(home: &Path, rec: &Path) {
    let before = fs::symlink_metadata(rec).unwrap();
    let home = home.to_path_buf();
    let (n, w, per_step) = within(move || {
        let mut b = budget(RUN, &home);
        let (n, w) = left(&mut b, 21_600);
        (n, w, b.per_step())
    });
    assert_eq!((n, w), (21_600, vec![UNUSABLE.to_string()]));
    assert!(per_step);
    let after = fs::symlink_metadata(rec).unwrap();
    assert_eq!(before.permissions().mode(), after.permissions().mode());
    assert_eq!(before.len(), after.len());
    assert_eq!(before.file_type(), after.file_type());
}

#[test]
fn a_bad_record_is_refused_and_never_repaired() {
    for body in ["abc\n", "600\n-5\n", "000000000000000000001\n"] {
        let (_t, home) = home();
        let rec = record(&home, RUN);
        rerun(&home);
        fs::write(&rec, body).unwrap();
        fs::set_permissions(&rec, fs::Permissions::from_mode(0o600)).unwrap();
        assert_unusable(&home, &rec);
        assert_eq!(fs::read_to_string(&rec).unwrap(), body);
    }
    let (_t, home) = home();
    let rec = record(&home, RUN);
    rerun(&home);
    fs::write(&rec, "1\n".repeat(32 * 1024 + 1)).unwrap();
    fs::set_permissions(&rec, fs::Permissions::from_mode(0o600)).unwrap();
    assert_unusable(&home, &rec);
}

#[test]
fn a_record_that_is_not_a_private_regular_file_is_refused() {
    // Each case has its own HOME; a block keeps `home` from hiding `home()`.
    {
        // Group- and world-readable.
        let (_t, home) = home();
        let rec = record(&home, RUN);
        rerun(&home);
        fs::write(&rec, "5\n").unwrap();
        fs::set_permissions(&rec, fs::Permissions::from_mode(0o644)).unwrap();
        assert_unusable(&home, &rec);
        assert_eq!(
            fs::metadata(&rec).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
    {
        // A symlink to a private file.
        let (_t, home) = home();
        let rec = record(&home, RUN);
        let target = rerun(&home).join("target");
        fs::write(&target, "5\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, &rec).unwrap();
        assert_unusable(&home, &rec);
        assert!(fs::symlink_metadata(&rec).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "5\n");
    }
    {
        // A second hard link.
        let (_t, home) = home();
        let rec = record(&home, RUN);
        fs::write(rerun(&home).join("first"), "5\n").unwrap();
        fs::set_permissions(
            rerun(&home).join("first"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        fs::hard_link(rerun(&home).join("first"), &rec).unwrap();
        assert_unusable(&home, &rec);
    }
    {
        // A FIFO, which must not block the step.
        let (_t, home) = home();
        let rec = record(&home, RUN);
        rerun(&home);
        let c = std::ffi::CString::new(rec.as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_unusable(&home, &rec);
    }
}

#[test]
fn a_rerun_directory_that_fails_its_checks_is_not_used() {
    let (_t, home) = home();
    let dir = rerun(&home);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    let mut b = budget(RUN, &home);
    assert_eq!(left(&mut b, 600), (600, vec![UNUSABLE.to_string()]));
    assert!(!record(&home, RUN).exists());
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o777
    );
}

// --- the per-step fallback ------------------------------------------------

#[test]
fn with_no_run_id_the_step_counts_alone_warns_once_and_touches_nothing() {
    let (_t, home) = home();
    for id in [None, Some(OsStr::new(""))] {
        let mut b = Budget::new(id, Some(&home));
        assert_eq!(left(&mut b, 600), (600, vec![NOT_SET.to_string()]));
        assert!(b.per_step());
        assert!(add(&mut b, 100).is_empty());
        assert_eq!(left(&mut b, 600), (500, vec![]));
        assert!(add(&mut b, 600).is_empty());
        assert_eq!(left(&mut b, 600), (0, vec![]), "never below 0");
    }
    assert!(!home.join(".amplihack").exists(), "nothing was created");
}

#[test]
fn an_invalid_run_id_is_never_used_for_a_path_or_printed() {
    let (_t, home) = home();
    for id in ["../x", "a/b", ".", "a.b"] {
        let mut b = budget(id, &home);
        let (n, w) = left(&mut b, 600);
        assert_eq!((n, w.clone()), (600, vec![NOT_VALID.to_string()]), "{id}");
        assert!(!w[0].contains(id), "{id:?} printed");
        assert!(add(&mut b, 60).is_empty());
    }
    let mut b = Budget::new(Some(OsStr::from_bytes(b"run\xff")), Some(&home));
    assert_eq!(left(&mut b, 600), (600, vec![NOT_VALID.to_string()]));
    assert!(!home.join(".amplihack").exists(), "nothing was created");
}

/// `p`, relative to this process's working directory.
fn relative(p: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap();
    let mut rel = PathBuf::new();
    for _ in cwd.components().skip(1) {
        rel.push("..");
    }
    rel.push(p.strip_prefix("/").unwrap());
    rel
}

#[test]
fn an_unset_empty_or_relative_home_cannot_hold_the_record() {
    let (_t, home) = home();
    let rel = relative(&home);
    assert!(rel.is_relative() && rel.join(".").is_dir(), "{rel:?}");
    for h in [None, Some(Path::new("")), Some(rel.as_path())] {
        let mut b = Budget::new(Some(OsStr::new(RUN)), h);
        assert_eq!(
            left(&mut b, 600),
            (600, vec![UNUSABLE.to_string()]),
            "{h:?}"
        );
        assert!(add(&mut b, 60).is_empty());
        assert_eq!(left(&mut b, 600), (540, vec![]));
    }
    assert!(
        !home.join(".amplihack").exists(),
        "a relative HOME was used"
    );
}

#[test]
fn a_failed_append_falls_back_from_the_last_good_read() {
    let (_t, home) = home();
    let rec = record(&home, RUN);
    rerun(&home);
    fs::write(&rec, "18000\n").unwrap();
    fs::set_permissions(&rec, fs::Permissions::from_mode(0o600)).unwrap();
    let mut b = budget(RUN, &home);
    assert_eq!(left(&mut b, 21_600), (3600, vec![]));
    assert!(!b.per_step());
    // The record stops being usable during the wait.
    fs::set_permissions(&rec, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(add(&mut b, 600), vec![UNUSABLE.to_string()]);
    assert!(b.per_step());
    assert_eq!(
        left(&mut b, 21_600),
        (3000, vec![]),
        "3600 left, 600 waited"
    );
    // It stays in the fallback even once the record is usable again, and
    // writes nothing more to it.
    fs::set_permissions(&rec, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(add(&mut b, 1000).is_empty());
    assert_eq!(left(&mut b, 21_600), (2000, vec![]));
    assert!(add(&mut b, 5000).is_empty());
    assert_eq!(left(&mut b, 21_600), (0, vec![]));
    assert_eq!(fs::read_to_string(&rec).unwrap(), "18000\n");
}

#[test]
fn a_failed_read_falls_back_from_the_last_good_read_less_the_waits_since() {
    let (_t, home) = home();
    let rec = record(&home, RUN);
    rerun(&home);
    fs::write(&rec, "18000\n").unwrap();
    fs::set_permissions(&rec, fs::Permissions::from_mode(0o600)).unwrap();
    let mut b = budget(RUN, &home);
    assert_eq!(left(&mut b, 21_600), (3600, vec![]));
    assert!(add(&mut b, 600).is_empty(), "appended");
    assert_eq!(fs::read_to_string(&rec).unwrap(), "18000\n600\n");
    fs::write(&rec, "18000\n600\ntorn").unwrap();
    assert_eq!(left(&mut b, 21_600), (3000, vec![UNUSABLE.to_string()]));
    assert!(b.per_step());
    assert_eq!(left(&mut b, 21_600), (3000, vec![]), "one warning");
    assert_eq!(fs::read_to_string(&rec).unwrap(), "18000\n600\ntorn");
    // Once fallen back, none of the step's later waits reach the record,
    // even after it is usable again, so other steps of the run do not
    // count them.
    fs::write(&rec, "18000\n600\n").unwrap();
    assert!(add(&mut b, 1000).is_empty());
    assert_eq!(left(&mut b, 21_600), (2000, vec![]));
    assert_eq!(fs::read_to_string(&rec).unwrap(), "18000\n600\n");
    let mut other = budget(RUN, &home);
    assert_eq!(left(&mut other, 21_600), (3000, vec![]), "1000 not counted");
}

#[test]
fn a_step_that_never_read_the_record_starts_from_the_whole_budget() {
    let (_t, home) = home();
    let rec = record(&home, RUN);
    rerun(&home);
    fs::write(&rec, "garbage\n").unwrap();
    fs::set_permissions(&rec, fs::Permissions::from_mode(0o600)).unwrap();
    let mut b = budget(RUN, &home);
    assert_eq!(left(&mut b, 21_600), (21_600, vec![UNUSABLE.to_string()]));
    assert!(add(&mut b, 100).is_empty());
    assert_eq!(left(&mut b, 21_600), (21_500, vec![]));
    assert_eq!(fs::read_to_string(&rec).unwrap(), "garbage\n");
}

#[test]
fn saturating_waits_never_wrap_the_count() {
    let (_t, home) = home();
    let mut b = budget(RUN, &home);
    left(&mut b, u64::MAX);
    assert!(add(&mut b, u64::MAX).is_empty());
    assert!(add(&mut b, 1).is_empty());
    assert_eq!(left(&mut b, u64::MAX), (0, vec![]));
    let mut alone = Budget::new(None, Some(&home));
    left(&mut alone, 10);
    add(&mut alone, u64::MAX);
    add(&mut alone, u64::MAX);
    assert_eq!(left(&mut alone, 10).0, 0);
}
