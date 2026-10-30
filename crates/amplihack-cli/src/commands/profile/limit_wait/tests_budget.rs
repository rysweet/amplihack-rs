//! Issue #1761: the run budget record, `$HOME/.amplihack/rerun/
//! wait-budget-<run id>`. The run id is checked on its bytes before any
//! path is built (SR-3); the record is parsed strictly within 64 KiB with
//! saturating sums (SR-4); it is opened without following symlinks and
//! must be a private regular file with one link (SR-10, SR-11); a bad
//! record is never repaired. Without a usable record a step keeps its own
//! count from its last good read and writes one fixed warning (D3, SR-8).
//! Refused records and the per-step fallback are in
//! `tests_budget_refused.rs`, clean-up of old records in
//! `tests_budget_sweep.rs`; both use the helpers here.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_budget;`.

use super::budget::{Budget, parse_total, valid_run_id};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(super) const NOT_SET: &str = "warning: the usage-limit wait budget applies to this step only (AMPLIHACK_RECIPE_RUN_ID is not set)";
pub(super) const NOT_VALID: &str = "warning: the usage-limit wait budget applies to this step only (AMPLIHACK_RECIPE_RUN_ID is not a valid run id)";
pub(super) const UNUSABLE: &str = "warning: the usage-limit wait budget applies to this step only (the budget record cannot be used)";

pub(super) const RUN: &str = "6f1c2b9e-4b0e-4c47-9a63-1d2e3f4a5b6c";

pub(super) fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap().join("home");
    fs::create_dir_all(&home).unwrap();
    (tmp, home)
}

/// `$HOME/.amplihack/rerun`, created private as the supervisor creates it.
pub(super) fn rerun(home: &Path) -> PathBuf {
    let dir = home.join(".amplihack").join("rerun");
    fs::create_dir_all(&dir).unwrap();
    for d in [dir.parent().unwrap(), dir.as_path()] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

pub(super) fn record(home: &Path, id: &str) -> PathBuf {
    home.join(".amplihack")
        .join("rerun")
        .join(format!("wait-budget-{id}"))
}

pub(super) fn budget(id: &str, home: &Path) -> Budget {
    Budget::new(Some(OsStr::new(id)), Some(home))
}

/// `b.left(total)` and the warnings it wrote.
pub(super) fn left(b: &mut Budget, total: u64) -> (u64, Vec<String>) {
    let mut w = Vec::new();
    let n = b.left(total, &mut |l| w.push(l.to_string()));
    (n, w)
}

/// `b.add(secs)` and the warnings it wrote.
pub(super) fn add(b: &mut Budget, secs: u64) -> Vec<String> {
    let mut w = Vec::new();
    b.add(secs, &mut |l| w.push(l.to_string()));
    w
}

// --- run id ---------------------------------------------------------------

#[test]
fn a_run_id_is_1_to_64_letters_digits_or_dashes() {
    let max = "a".repeat(64);
    for good in [RUN, "a", "Z9", "-", "-leading", "tree-1-2", max.as_str()] {
        assert!(valid_run_id(OsStr::new(good)), "{good:?}");
    }
    let over = "a".repeat(65);
    for bad in [
        "",
        over.as_str(),
        ".",
        "..",
        "../x",
        "a/b",
        "a.b",
        "a_b",
        "a b",
        "a\nb",
        "a\0b",
        "é",
        "run\u{202e}id",
    ] {
        assert!(!valid_run_id(OsStr::new(bad)), "{bad:?}");
    }
    assert!(!valid_run_id(OsStr::from_bytes(b"ab\xff")), "non-UTF-8");
    assert!(!valid_run_id(OsStr::from_bytes(b"\xff")), "non-UTF-8");
}

// --- parse ----------------------------------------------------------------

#[test]
fn the_total_is_the_saturating_sum_of_decimal_lines() {
    for (raw, want) in [
        (&b""[..], 0),
        (b"\n", 0),
        (b"600\n", 600),
        (b"600\n30\n", 630),
        (b"600\n\n30\n", 630),
        (b"600", 600),
        (b"0\n0\n", 0),
        (b"00000000000000000001\n", 1),
        (b"18446744073709551615\n", u64::MAX),
        (b"18446744073709551615\n1\n", u64::MAX),
        (b"9223372036854775807\n9223372036854775807\n9\n", u64::MAX),
    ] {
        assert_eq!(
            parse_total(raw),
            Some(want),
            "{:?}",
            String::from_utf8_lossy(raw)
        );
    }
}

#[test]
fn any_other_line_makes_the_record_unusable() {
    for raw in [
        &b"18446744073709551616\n"[..],
        b"000000000000000000001\n",
        b"-1\n",
        b"+1\n",
        b" 1\n",
        b"1 \n",
        b"1\r\n",
        b"1.5\n",
        b"0x10\n",
        b"abc\n",
        b"600\nabc\n",
        b"\xff\n",
        b"1\x002\n",
    ] {
        assert_eq!(parse_total(raw), None, "{:?}", String::from_utf8_lossy(raw));
    }
}

#[test]
fn a_record_over_64_kib_is_unusable() {
    let full = "1\n".repeat(32 * 1024);
    assert_eq!(full.len(), 64 * 1024);
    assert_eq!(parse_total(full.as_bytes()), Some(32 * 1024));
    let over = format!("{full}1\n");
    assert_eq!(parse_total(over.as_bytes()), None);
}

// --- the record -----------------------------------------------------------

#[test]
fn every_step_of_a_run_shares_one_record() {
    let (_t, home) = home();
    let mut a = budget(RUN, &home);
    assert_eq!(left(&mut a, 21_600), (21_600, vec![]));
    let rec = record(&home, RUN);
    assert_eq!(fs::read(&rec).unwrap(), b"", "created at the budget check");
    assert_eq!(
        fs::metadata(&rec).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(add(&mut a, 600).is_empty());
    assert!(add(&mut a, 0).is_empty(), "a 0 s wait is a wait");
    assert_eq!(fs::read_to_string(&rec).unwrap(), "600\n0\n");
    let mut b = budget(RUN, &home);
    assert_eq!(left(&mut b, 21_600), (21_000, vec![]));
    assert_eq!(left(&mut b, 600), (0, vec![]), "never below 0");
    assert!(!a.per_step() && !b.per_step());
    let mut other = budget("another-run", &home);
    assert_eq!(left(&mut other, 21_600), (21_600, vec![]));
}

#[test]
fn a_zero_budget_has_nothing_left() {
    let (_t, home) = home();
    let mut b = budget(RUN, &home);
    assert_eq!(left(&mut b, 0), (0, vec![]));
}

#[test]
fn parallel_steps_never_lose_an_append() {
    let (_t, home) = home();
    left(&mut budget(RUN, &home), 1);
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let home = home.clone();
            std::thread::spawn(move || {
                let mut b = budget(RUN, &home);
                for _ in 0..50 {
                    assert!(add(&mut b, 1).is_empty());
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(left(&mut budget(RUN, &home), 1000), (900, vec![]));
    let raw = fs::read_to_string(record(&home, RUN)).unwrap();
    assert_eq!(raw, "1\n".repeat(100));
}
