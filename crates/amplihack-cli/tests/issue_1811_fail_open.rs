//! Issue #1811: fail-open admissions that happen before anything can be
//! saved still print one stderr line. See "Failure modes" in
//! docs/reference/capacity-governor.md.

#![cfg(unix)]

use crate::issue_1811_fixture::{Fixture, describe, text};
use std::fs;

/// P2 (PR 1831): a fault that happens before anything can be saved still
/// says so on stderr, without an absolute path.
#[test]
fn unsafe_state_dir_fails_open_with_one_stderr_line() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 30.0), ("beta", 10.0, 30.0)]);
    let elsewhere = f.tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.gov()).unwrap();
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 1, "the work still starts");
    let err = text(&out.stderr);
    let lines: Vec<&str> = err
        .lines()
        .filter(|l| l.starts_with("amplihack: governor: admitted without a decision (state-dir): "))
        .collect();
    assert_eq!(lines.len(), 1, "{}", describe(&out));
    assert!(lines[0].len() <= 600, "{}", lines[0]);
    let tmp = f.tmp.path().to_string_lossy();
    assert!(!lines[0].contains(&*tmp), "absolute path in {:?}", lines[0]);
    assert!(
        !text(&out.stdout).contains("governor"),
        "{}",
        describe(&out)
    );
}
