//! Issue #2677 B1: a drive starts `amplihack recipe run` from inside a step
//! of a run that is already observed. The outer observer follows forks, so
//! the nested run is traced by a process it has no marker for
//! (`AMPLIHACK_OBSERVER_PID` is not passed into the step). On 1.2.163 the
//! nested run stopped before any step with "process is traced by an
//! unrelated observer".
//!
//! The nested run must instead try its own observer, have the kernel refuse
//! `PTRACE_TRACEME` for real, run its step unobserved and record coverage as
//! `unavailable`, naming the refused call and its errno. The test waits only
//! for process exit and keeps all state under the fixture's tempdir.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

use super::{Fixture, find_named, json_file, succeeds};

/// The single `git-recording.json` under `work_root`, read as JSON.
fn only_recording(work_root: &Path, which: &str) -> serde_json::Value {
    let mut found: Vec<PathBuf> = Vec::new();
    find_named(work_root, "git-recording.json", &mut found);
    assert_eq!(
        found.len(),
        1,
        "expected one {which} git-recording.json: {found:?}"
    );
    json_file(&found[0])
}

#[test]
fn a_run_started_from_an_observed_step_runs_unobserved_and_says_so() {
    let f = Fixture::new();
    let nested = f.root.join("nested");
    let out = f
        .command("issue-2677-outer")
        .env(
            "ISSUE_2677_NESTED_BIN",
            crate::support::amplihack_bin::amplihack_bin(),
        )
        .env("ISSUE_2677_NESTED_RECIPE", f.root.join("recipe.yaml"))
        .env("ISSUE_2677_NESTED_OUT", &nested)
        .output()
        .unwrap();
    succeeds(&out);

    // The reproduction only means something if the outer run was observed:
    // a host whose ptrace policy forbids that fails here instead of passing.
    let outer = only_recording(&f.root.join("work"), "outer");
    assert_eq!(
        outer["coverage"], "established",
        "outer run was not observed; this test needs ptrace (Yama ptrace_scope <= 1): {outer}"
    );

    let stderr = String::from_utf8_lossy(
        &std::fs::read(nested.join("stderr"))
            .unwrap_or_else(|e| panic!("the nested run never started: {e}")),
    )
    .into_owned();
    let status = std::fs::read_to_string(nested.join("status")).unwrap();
    assert_eq!(status, "0", "the nested run failed:\n{stderr}");
    assert!(
        nested.join("observations/startup_cwd").is_file(),
        "no step of the nested run ran:\n{stderr}"
    );

    let inner = only_recording(&nested.join("work"), "nested");
    assert_eq!(
        inner["coverage"], "unavailable",
        "the nested run claimed coverage it cannot have: {inner}"
    );

    assert!(stderr.contains("PTRACE_TRACEME"), "{stderr}");
    assert!(stderr.contains("errno"), "{stderr}");
    assert!(
        !stderr.contains("nested tracing"),
        "the notice states a cause the code cannot know:\n{stderr}"
    );
    let errno: i32 = stderr
        .split_once("(errno ")
        .and_then(|(_, rest)| rest.split_once(',').map(|(n, _)| n))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no '(errno N, NAME)' in the notice:\n{stderr}"));
    eprintln!("issue #2677 nested reproduction: PTRACE_TRACEME refused with errno {errno}");
    assert_eq!(
        errno,
        libc::EPERM,
        "PTRACE_TRACEME was refused with errno {errno}, not EPERM; report it in #2677 and do \
         not widen the refused list"
    );
}
