//! #3053, #2996 and #2997: every record of auto-drive-to-merge and
//! auto-drive-light goes to the one state folder the runner bound. A folder an
//! agent step reports (here A, from the build stage) is information only and
//! never decides where records go.
//!
//! #3129 re-pointed these tests from the retired loops (and the YAML-rewriting
//! stub agents they needed) to the goal loop's harness. What is real: the CLI,
//! the pinned recipe runner, the drive recipes, the goal loop, the trusted
//! merge tool, the authorisation helper and the report, from a byte copy of
//! the bundle. What is a stand-in: `gh` and the agent (see
//! `issue_3129_goal_loop::support`); the build stage's agent steps report
//! folder A. Assertions are on what the run wrote and where.

#![cfg(target_os = "linux")]

use crate::issue_3129_goal_loop::support::{Harness, MERGE_WHEN_GREEN, PR, describe};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The build stage's answer: the PR, its head and base, and folder A as its
/// state folder, which no record may use.
fn report_folder_a(h: &Harness) -> PathBuf {
    let a = h.base.join("reported-a");
    fs::create_dir_all(&a).unwrap();
    fs::set_permissions(&a, fs::Permissions::from_mode(0o700)).unwrap();
    h.default_reply(&json!({
        "should_run": "true", "pr_found": "true", "pr": PR, "pr_url": "https://github.com/o/r/pull/7",
        "pr_state": "OPEN", "branch": "feat/widget", "head_sha": h.initial_head,
        "base_branch": "main", "base_sha": h.base_head(), "is_cross_repository": false,
        "state_dir": a.display().to_string(), "repo": h.repo.display().to_string(),
        "merged_before_run": false, "blockers": [], "attempts": []
    }));
    a
}

fn assert_untouched(a: &Path, out: &std::process::Output) {
    let left: Vec<_> = fs::read_dir(a)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        left.is_empty(),
        "the reported folder A was written: {left:?}\n{}",
        describe(out)
    );
}

fn read(path: &Path) -> Value {
    let raw = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(raw.trim()).unwrap()
}

/// After a refused merge (merging off), say the PR is ready.
const MERGE_THEN_READY: &str = r#"
prev=$(jq -r '.previous.outcome // ""' "$TURN/reading.json")
if [ "$prev" = "not-authorised" ]; then
  echo '{"action":"ready","args":{"text":"The pull request is ready. Merging is off for this run."},"why":"Merging is off."}'
else
  head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
  printf '{"action":"merge","args":{"head":"%s"},"why":"Every check passed."}\n' "$head"
fi
"#;

// T1 (#3053): the build reports A; the runner binds B; every record is in B.
#[test]
fn a_drive_whose_build_reports_another_folder_keeps_every_record_in_the_bound_one() {
    let h = Harness::new();
    let a = report_folder_a(&h);
    h.policy(MERGE_WHEN_GREEN);
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_untouched(&a, &out);
    assert!(
        h.state.join("turns/001/reading.json").is_file(),
        "{}",
        describe(&out)
    );
    assert_eq!(h.loop_end()["end"], json!("merged"));
    let record = h.merge_result();
    let o = Harness::outcome(&out);
    assert_eq!(record["result"], json!("merged"), "{record}");
    assert_eq!(record["run_id"], o["run_id"], "{record}\n{o}");
    assert_eq!(h.loop_end()["run_id"], o["run_id"]);
    assert_eq!(o["requested_action"], json!("merge"), "{o}");
    assert_eq!(o["stop_reason"], json!("completed"), "{o}");
    let merge = h.merge_calls();
    assert_eq!(merge.len(), 1, "{:?}", h.calls());
    assert!(
        merge[0].contains(&format!("--match-head-commit {}", h.initial_head)),
        "{merge:?}"
    );
}

// T2a (#2997): auto-drive-light allowed to merge measures into B and merges.
#[test]
fn a_light_drive_allowed_to_merge_keeps_its_records_in_the_bound_folder() {
    let h = Harness::new();
    let a = report_folder_a(&h);
    h.policy(MERGE_WHEN_GREEN);
    let out = h.drive("auto-drive-light", &["no_merge=false"]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_untouched(&a, &out);
    assert!(
        h.state.join("light-eligibility.json").is_file(),
        "{}",
        describe(&out)
    );
    assert_eq!(h.merge_result()["result"], json!("merged"));
    assert_eq!(Harness::outcome(&out)["stop_reason"], json!("completed"));
}

// T2b (#2997): light's default is merging-off. The merge action is refused
// as not-authorised, the agent says the PR is ready, and the run is completed.
#[test]
fn a_light_drive_left_merging_off_completes_from_the_bound_folder() {
    let h = Harness::new();
    let a = report_folder_a(&h);
    h.policy(MERGE_THEN_READY);
    let out = h.drive("auto-drive-light", &[]);
    assert_untouched(&a, &out);
    assert!(h.merge_calls().is_empty(), "{:?}", h.calls());
    let record = h.merge_result();
    assert_eq!(record["result"], json!("not-authorised"), "{record}");
    assert_eq!(h.loop_end()["end"], json!("ready"));
    let o = Harness::outcome(&out);
    assert_eq!(o["requested_action"], json!("verify_readiness"), "{o}");
    assert_eq!(
        o["stop_reason"],
        json!("completed"),
        "{o}\n{}",
        describe(&out)
    );
}

// T5 (#3053, as on #1516): the operator gives no folder, so the runner binds
// <work root>/runs/<id>/autodrive-state, and the build reports A. Every
// record is in the runner's folder.
#[test]
fn a_drive_given_no_folder_keeps_every_record_in_the_one_the_runner_bound() {
    for recipe in ["auto-drive-to-merge", "auto-drive-light"] {
        let h = Harness::new();
        let a = report_folder_a(&h);
        h.policy(MERGE_WHEN_GREEN);
        let glob = format!("{}/runs/*/autodrive-state", h.work.display());
        let out = h
            .command(&[
                "recipe",
                "run",
                recipe,
                "-c",
                &format!("pr_number={PR}"),
                "-c",
                "task_description=Get this pull request merged.",
                "-c",
                "no_merge=false",
                "--format",
                "json",
                "-w",
                h.repo.to_str().unwrap(),
                "--no-detach",
            ])
            .env("STUB_STATE_GLOB", &glob)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{recipe}: {}", describe(&out));
        assert_untouched(&a, &out);
        let bound: Vec<PathBuf> = fs::read_dir(h.work.join("runs"))
            .unwrap()
            .map(|e| e.unwrap().path().join("autodrive-state"))
            .filter(|p| p.join("loop-end.json").is_file())
            .collect();
        assert_eq!(
            bound.len(),
            1,
            "{recipe}: one runner-bound folder holds the end: {bound:?}"
        );
        let bound = &bound[0];
        assert!(bound.join("turns/001/record.md").is_file(), "{recipe}");
        assert_eq!(
            read(&bound.join("loop-end.json"))["end"],
            json!("merged"),
            "{recipe}"
        );
        assert_eq!(
            read(&bound.join("merge-result.json"))["result"],
            json!("merged"),
            "{recipe}"
        );
        assert!(
            !h.state.join("loop-end.json").exists(),
            "{recipe}: the test's own folder is not used"
        );
        let merge = h.merge_calls();
        assert!(
            merge.len() == 1
                && merge[0].contains(&format!("--match-head-commit {}", h.initial_head)),
            "{recipe}: {merge:?}"
        );
        assert_eq!(
            Harness::outcome(&out)["stop_reason"],
            json!("completed"),
            "{recipe}: {}",
            describe(&out)
        );
    }
}
