//! tests/integration/issue_2362_merge_evidence_head_ci.rs
//!
//! Issue #2362: auto-drive-to-merge's merge-ready QA ran before CI at the head
//! had finished, and every round merged the base and pushed, which restarted
//! CI. Since #2329 the merge-evidence steps are agent steps, so these tests
//! check the contract their prompts state, one behaviour per test:
//!
//! - sync policy by merge state: merge only for `BEHIND` or `DIRTY`, and push
//!   a `BEHIND` merge even when it changes no file content;
//! - head binding: GitHub's `headRefOid`, never the local `HEAD`;
//! - CI is read once per round, before QA, and never watched or polled;
//! - newest run per workflow, so a superseded run does not decide;
//! - unfinished runs give `PENDING` with a measured `ci_pending`;
//! - unreadable is never GREEN;
//! - QA runs no suite while CI at the head is `PENDING`, and the fix step
//!   pushes nothing for it.

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn recipe(name: &str) -> Value {
    serde_yaml::from_str(&read(&format!("amplifier-bundle/recipes/{name}.yaml")))
        .unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn step_ids(recipe: &Value) -> Vec<String> {
    recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .map(|s| s["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn prompt(recipe: &Value, id: &str) -> String {
    recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing step `{id}`"))["prompt"]
        .as_str()
        .unwrap_or_else(|| panic!("step `{id}` has no prompt"))
        .to_string()
}

fn evidence() -> Value {
    recipe("autodrive-merge-evidence")
}

fn assert_has(text: &str, needle: &str, what: &str) {
    assert!(text.contains(needle), "{what}: missing `{needle}`");
}

#[test]
fn ci_is_read_before_qa() {
    assert_eq!(
        step_ids(&evidence()),
        [
            "step-01-sync-with-base",
            "step-02-read-head-ci",
            "step-03-qa-team-scenarios"
        ],
        "the round must sync, then read CI, then run QA"
    );
}

#[test]
fn merge_evidence_has_no_shell_steps() {
    // The bash 3.2 guard (mapfile, readarray) applies to `command:` bodies;
    // this recipe has none, so it cannot reintroduce one.
    for s in evidence()["steps"].as_sequence().expect("steps") {
        assert_eq!(s["type"].as_str(), Some("agent"), "{:?}", s["id"]);
        assert!(s.get("command").is_none(), "{:?} has a command", s["id"]);
    }
}

#[test]
fn sync_policy_follows_merge_state() {
    let p = prompt(&evidence(), "step-01-sync-with-base");
    let what = "step-01 sync policy";
    assert_has(&p, "only when GitHub requires it (issue #2362)", what);
    assert_has(
        &p,
        "`mergeStateStatus` `BEHIND`: merge the observed base and push, even when the merge changes no file content",
        what,
    );
    assert_has(
        &p,
        "`mergeStateStatus` `DIRTY`, or `mergeable` `CONFLICTING`",
        what,
    );
    assert_has(
        &p,
        "Any other state (`CLEAN`, `BLOCKED`, `UNSTABLE`, `HAS_HOOKS`, `UNKNOWN`, `DRAFT`): do not merge, commit or push",
        what,
    );
}

#[test]
fn head_is_bound_to_github_head_ref() {
    let p = prompt(&evidence(), "step-01-sync-with-base");
    assert_has(&p, "headRefOid", "step-01");
    assert_has(&p, "the local `git rev-parse HEAD` is not", "step-01");
    assert_has(&p, "`head-mismatch`", "step-01");
    let ci = prompt(&evidence(), "step-02-read-head-ci");
    assert_has(
        &ci,
        "at the head step-01 reported (`head_sha` in {{merge_sync}}",
        "step-02",
    );
    assert_has(&ci, "Discard any run whose `headSha` differs", "step-02");
}

#[test]
fn ci_is_read_once_and_never_watched() {
    let text = read("amplifier-bundle/recipes/autodrive-merge-evidence.yaml");
    let ci = prompt(&evidence(), "step-02-read-head-ci");
    assert_has(&ci, "ONCE in this round", "step-02");
    assert_has(
        &ci,
        "no `gh run watch`, no `gh pr checks --watch`, no sleep and no loop of `gh` calls",
        "step-02",
    );
    for banned in [
        "Poll pending work",
        "polling interval",
        "autodrive_ci_watch_interval",
    ] {
        assert!(
            !text.contains(banned),
            "merge evidence still says `{banned}`"
        );
    }
    for name in [
        "auto-drive-to-merge",
        "autodrive-merge-loop",
        "autodrive-merge-round",
    ] {
        let r = read(&format!("amplifier-bundle/recipes/{name}.yaml"));
        assert!(
            !r.contains("autodrive_ci_watch_interval"),
            "{name} forwards a watch interval"
        );
    }
}

#[test]
fn newest_run_per_workflow_decides() {
    let ci = prompt(&evidence(), "step-02-read-head-ci");
    assert_has(&ci, "Keep only the newest run per workflow", "step-02");
    assert_has(
        &ci,
        "superseded by a re-run neither fails nor passes CI",
        "step-02",
    );
}

#[test]
fn unfinished_runs_are_pending_with_a_count() {
    let ci = prompt(&evidence(), "step-02-read-head-ci");
    assert_has(
        &ci,
        "`ci_pending` set to the number of such runs",
        "step-02",
    );
    assert_has(
        &ci,
        "`ci_pending` is the measured count, never a constant",
        "step-02",
    );
    assert_has(
        &ci,
        "When no run exists at the head, return `PENDING`",
        "step-02",
    );
    assert_has(&ci, "this step never blocks on it", "step-02");
}

#[test]
fn unreadable_is_never_green() {
    let ci = prompt(&evidence(), "step-02-read-head-ci");
    assert_has(
        &ci,
        "unknown conclusions and unreadable responses are never GREEN",
        "step-02",
    );
    assert_has(&ci, "anything else is UNREADABLE", "step-02");
    assert_has(
        &ci,
        "Keep each failed call's stderr, redacted, as the cause",
        "step-02",
    );
}

#[test]
fn qa_waits_for_finished_ci_and_the_round_does_not_push_for_it() {
    let qa = prompt(&evidence(), "step-03-qa-team-scenarios");
    assert_has(
        &qa,
        "When `ci_status` in {{ci_evidence}} is `PENDING`",
        "step-03",
    );
    assert_has(
        &qa,
        "run no suite and return `qa_status` `BLOCKED` with `qa_reason` `head-ci-pending`",
        "step-03",
    );
    let fix = prompt(&recipe("autodrive-merge-round"), "step-04-address-blockers");
    assert_has(
        &fix,
        "**`ci_status` `PENDING`** or **`head-ci-pending`** (#2362): nothing to fix. Do not push",
        "merge-round step-04",
    );
}

#[test]
fn docs_describe_the_round_order() {
    let doc = read("docs/reference/auto-drive-to-merge.md");
    assert_has(
        &doc,
        "### Base sync and CI at the head (#2362)",
        "reference doc",
    );
    assert_has(&doc, "`gh run watch`", "reference doc");
    let skill = read("amplifier-bundle/skills/auto-drive-to-merge/SKILL.md");
    assert_has(&skill, "never watched or polled (#2362)", "SKILL.md");
    assert_eq!(
        skill,
        read("docs/claude/skills/auto-drive-to-merge/SKILL.md"),
        "the two SKILL.md copies must match"
    );
}
