//! Issue #1761: clean-up of old budget records (SR-12), using the helpers
//! of `tests_budget.rs`. Each time a step opens its run's record, other
//! runs' records more than 7 days old are deleted. Only a regular file
//! named `wait-budget-<valid run id>` (per `lstat`) qualifies; a future
//! modification time keeps it; the step's own record is kept; at most 1024
//! entries are looked at; nothing is cleaned without a usable run id.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_budget_sweep;`.

use super::budget::Budget;
use super::tests_budget::{RUN, add, budget, home, left, record, rerun};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

const DAY: u64 = 24 * 3600;

/// A private file at `path` whose modification time is `days` days ago
/// (from now when negative).
fn file_aged(path: &Path, days: i64) {
    fs::write(path, "60\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    age(path, days);
}

fn age(path: &Path, days: i64) {
    let d = Duration::from_secs(days.unsigned_abs() * DAY);
    let t = if days >= 0 {
        SystemTime::now() - d
    } else {
        SystemTime::now() + d
    };
    fs::File::open(path).unwrap().set_modified(t).unwrap();
}

#[test]
fn another_runs_record_older_than_seven_days_is_deleted_at_a_read() {
    let (_t, home) = home();
    let dir = rerun(&home);
    file_aged(&dir.join("wait-budget-old-run"), 8);
    file_aged(&dir.join("wait-budget-recent-run"), 6);
    left(&mut budget(RUN, &home), 600);
    assert!(!dir.join("wait-budget-old-run").exists(), "8 days old");
    assert!(dir.join("wait-budget-recent-run").exists(), "6 days old");
}

#[test]
fn an_append_also_cleans_up() {
    let (_t, home) = home();
    let dir = rerun(&home);
    let mut b = budget(RUN, &home);
    left(&mut b, 600);
    file_aged(&dir.join("wait-budget-later"), 30);
    add(&mut b, 10);
    assert!(!dir.join("wait-budget-later").exists());
}

#[test]
fn the_steps_own_record_is_kept_however_old() {
    let (_t, home) = home();
    rerun(&home);
    let own = record(&home, RUN);
    file_aged(&own, 30);
    let mut b = budget(RUN, &home);
    assert_eq!(left(&mut b, 600), (540, vec![]));
    assert!(own.exists());
    let mtime = |p: &Path| fs::metadata(p).unwrap().modified().unwrap();
    let before = mtime(&own);
    left(&mut b, 600);
    assert_eq!(mtime(&own), before, "a read does not touch the record");
}

#[test]
fn only_old_regular_files_with_a_valid_run_id_are_deleted() {
    let (_t, home) = home();
    let dir = rerun(&home);
    let elsewhere = home.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let target = elsewhere.join("target");
    file_aged(&target, 30);
    std::os::unix::fs::symlink(&target, dir.join("wait-budget-link")).unwrap();
    fs::create_dir(dir.join("wait-budget-subdir")).unwrap();
    file_aged(
        &dir.join("wait-budget-subdir").join("wait-budget-inner"),
        30,
    );
    age(&dir.join("wait-budget-subdir"), 30);
    let long = format!("wait-budget-{}", "a".repeat(65));
    let kept = [
        "wait-budget-",
        "wait-budget-a.b",
        "wait-budget-a_b",
        long.as_str(),
        "wait-budget-future",
        "held-back-output",
        "budget-wait-old",
    ];
    for name in kept {
        file_aged(&dir.join(name), 30);
    }
    age(&dir.join("wait-budget-future"), -30);
    left(&mut budget(RUN, &home), 600);
    for name in kept {
        assert!(dir.join(name).exists(), "{name} was deleted");
    }
    assert!(
        fs::symlink_metadata(dir.join("wait-budget-link")).is_ok(),
        "a symlink is never followed or removed"
    );
    assert!(target.exists(), "the symlink's target was deleted");
    assert!(
        dir.join("wait-budget-subdir")
            .join("wait-budget-inner")
            .exists(),
        "subdirectories are not entered"
    );
}

#[test]
fn at_most_1024_entries_are_looked_at() {
    let (_t, home) = home();
    let dir = rerun(&home);
    for i in 0..1100 {
        file_aged(&dir.join(format!("wait-budget-old-{i:04}")), 30);
    }
    left(&mut budget(RUN, &home), 600);
    let left_over = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("wait-budget-old-")
        })
        .count();
    assert!(
        (76..=77).contains(&left_over),
        "{left_over} old records left of 1100"
    );
}

#[test]
fn nothing_is_cleaned_without_a_usable_run_id() {
    for id in [None, Some("../x"), Some("a.b")] {
        let (_t, home) = home();
        let dir = rerun(&home);
        file_aged(&dir.join("wait-budget-old-run"), 30);
        let mut b = Budget::new(id.map(std::ffi::OsStr::new), Some(&home));
        left(&mut b, 600);
        add(&mut b, 60);
        assert!(dir.join("wait-budget-old-run").exists(), "{id:?}");
    }
}
