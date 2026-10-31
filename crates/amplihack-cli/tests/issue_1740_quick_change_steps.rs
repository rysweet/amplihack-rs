//! Issue #1740: steps 2, 4, 5 and 10 of `quick-change.yaml`, run from the live
//! YAML against a temporary repository on a `quick/` branch.
//!
//! - `allow_no_op`: smart-classify-route sets it to `true` for docs-only,
//!   audit and meta tasks, and smart-execute-routing passes it on. With it,
//!   an empty diff gives verdict `empty`, which no later step runs on, in
//!   place of failing the run with exit 3.
//! - The hand-over note: it must say where the change is and how to see it
//!   whether or not the agent committed, and its instruction to apply the
//!   patch must be correct in both cases.
//! - The commit identity: a run that could never commit stops before the
//!   agent, not after it.
//! - The PR: a run where no check ran is a draft and says so.
//!
//! Each command runs under `bash -c` started by
//! [`crate::isolated_amplihack::isolated`], with a temp `HOME`, the
//! repository's own bundle as `AMPLIHACK_HOME` and the runner's values in
//! `RECIPE_VAR_*`. No agent or network is involved, and `gh` is a stub.

use std::path::PathBuf;
use std::process::Output;

use serde_yaml::Value;

use crate::support::quick_change_repo::{Repo, git, stderr};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn recipe(name: &str) -> Value {
    let path = repo_root().join(format!("amplifier-bundle/recipes/{name}.yaml"));
    serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn step(r: &Value, id: &str) -> Value {
    r["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("step {id} missing"))
        .clone()
}

/// A [`Repo`] on a `quick/` branch, as `quick-change-worktree` leaves it.
fn quick_repo() -> Repo {
    let r = Repo::new();
    git(&r.wt, &["checkout", "-q", "-b", "quick/issue-0-test"]);
    r
}

fn run_step(r: &Repo, id: &str, vars: &[(&str, &str)]) -> Output {
    let command = step(&recipe("quick-change"), id)["command"]
        .as_str()
        .unwrap()
        .to_string();
    let wt = std::fs::canonicalize(&r.wt).unwrap();
    let setup = serde_json::json!({ "worktree_path": wt }).to_string();
    let mut c = crate::isolated_amplihack::isolated("bash");
    c.arg("-c")
        .arg(command)
        .env("HOME", r.tmp.path())
        .env(
            "AMPLIHACK_HOME",
            std::fs::canonicalize(repo_root()).unwrap(),
        )
        .env("RECIPE_VAR_worktree_setup", setup);
    // Each test states the identity it wants; none comes from the host.
    for k in [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "AMPLIHACK_GIT_AUTHOR_NAME",
        "AMPLIHACK_GIT_AUTHOR_EMAIL",
    ] {
        c.env_remove(k);
    }
    for (k, v) in vars {
        c.env(k, v);
    }
    c.output().unwrap_or_else(|e| panic!("run {id}: {e}"))
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn record_base(r: &Repo) -> String {
    let o = run_step(
        r,
        "quick-change-record-base",
        &[
            ("AMPLIHACK_GIT_AUTHOR_NAME", "Test"),
            ("AMPLIHACK_GIT_AUTHOR_EMAIL", "test@example.com"),
        ],
    );
    assert!(o.status.success(), "record-base: {}", stderr(&o));
    let base = stdout(&o);
    assert_eq!(base, r.base, "record-base pins HEAD");
    base
}

fn diff_limits(r: &Repo, base: &str, allow_no_op: &str) -> Output {
    run_step(
        r,
        "quick-change-diff-limits",
        &[
            ("RECIPE_VAR_quick_change_base", base),
            ("RECIPE_VAR_allow_no_op", allow_no_op),
        ],
    )
}

#[test]
fn allow_no_op_is_declared_forwarded_and_read_by_the_diff_limits_step() {
    let qc = recipe("quick-change");
    assert_eq!(qc["context"]["allow_no_op"].as_str(), Some("false"));
    let cmd = step(&qc, "quick-change-diff-limits")["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        cmd.contains("\"${RECIPE_VAR_allow_no_op:-}\" = true"),
        "{cmd}"
    );

    let routing = recipe("smart-execute-routing");
    let forwarded = step(&routing, "execute-single-round-1-quick-change")["context"]["allow_no_op"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert_eq!(forwarded, "{{allow_no_op}}");

    // An `empty` verdict must run nothing after step 4: every later step
    // runs only on `proceed` or `handover`.
    let ids: Vec<String> = qc["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    let at = ids
        .iter()
        .position(|id| id == "quick-change-diff-limits")
        .unwrap();
    for id in &ids[at + 1..] {
        let c = step(&qc, id)["condition"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(
            c == "quick_change_verdict == 'proceed'" || c == "quick_change_verdict == 'handover'",
            "{id} must run only on proceed or handover, not on empty: {c:?}"
        );
    }
}

#[test]
fn an_empty_diff_with_allow_no_op_finishes_with_verdict_empty() {
    let r = quick_repo();
    let base = record_base(&r);
    let o = diff_limits(&r, &base, "true");
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o), "empty");
    assert!(
        stderr(&o).contains("allow_no_op is true; finishing with no commit, push or PR"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn an_empty_diff_without_allow_no_op_still_fails_the_run() {
    let r = quick_repo();
    let base = record_base(&r);
    // Only the exact value smart-classify-route writes counts.
    for value in ["", "false", "True", "1", "yes"] {
        let o = diff_limits(&r, &base, value);
        assert_eq!(
            o.status.code(),
            Some(3),
            "allow_no_op={value:?}: {}",
            stderr(&o)
        );
        assert!(stdout(&o).is_empty(), "allow_no_op={value:?}");
    }
}

#[test]
fn allow_no_op_changes_nothing_when_there_is_a_change() {
    let r = quick_repo();
    let base = record_base(&r);
    r.write("README.md", "# demo, edited\n");
    let o = diff_limits(&r, &base, "true");
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), "proceed");

    for i in 0..4 {
        r.write(&format!("docs/n{i}.md"), "x\n");
    }
    let o = diff_limits(&r, &base, "true");
    assert_eq!(stdout(&o), "handover", "{}", stderr(&o));
}

#[test]
fn allow_no_op_does_not_hide_git_config_drift() {
    let r = quick_repo();
    let base = record_base(&r);
    git(
        &r.wt,
        &["config", "core.sshCommand", "ssh -o ProxyCommand=x"],
    );
    let o = diff_limits(&r, &base, "true");
    assert_eq!(o.status.code(), Some(4), "{}", stderr(&o));
    assert!(stdout(&o).is_empty());
}

/// Runs steps 4 and 5 and returns the note and the patch path.
fn hand_over(r: &Repo, base: &str, reason: &str) -> (String, String) {
    let o = diff_limits(r, base, "false");
    assert_eq!(stdout(&o), "handover", "{}", stderr(&o));
    assert!(
        stderr(&o).contains(&format!("QUICK_CHANGE_VERDICT=handover reason={reason}")),
        "{}",
        stderr(&o)
    );
    let o = run_step(
        r,
        "quick-change-write-handover",
        &[("RECIPE_VAR_quick_change_base", base)],
    );
    assert!(o.status.success(), "write-handover: {}", stderr(&o));
    let note = stdout(&o);
    let patch = note
        .lines()
        .find_map(|l| l.strip_prefix("Patch of the whole change against the base commit: "))
        .unwrap_or_else(|| panic!("no patch line in {note}"))
        .to_string();
    (note, patch)
}

fn git_ok(r: &Repo, args: &[&str]) -> bool {
    amplihack_git::command_in(&r.wt)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", r.tmp.path())
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn after_an_agent_commit_the_note_points_at_git_diff_and_does_not_apply_twice() {
    let r = quick_repo();
    let base = record_base(&r);
    r.write("README.md", "# demo, committed by the agent\n");
    git(&r.wt, &["commit", "-q", "-am", "agent"]);
    let (note, patch) = hand_over(&r, &base, "agent-commit");

    assert!(note.contains("(reason=agent-commit)"), "{note}");
    assert!(
        note.contains("1 commit(s) after the base commit, and possibly uncommitted changes"),
        "{note}"
    );
    assert!(!note.contains("The changes are uncommitted"), "{note}");
    assert!(note.contains(&format!("run `git diff {base}`")), "{note}");
    assert!(
        note.contains(&format!(
            "Apply the patch only if that diff is empty: git apply --index {patch}"
        )),
        "{note}"
    );

    // As it stands, the worktree differs from the base, so the note says not
    // to apply; applying would fail, because the commit already holds it.
    assert!(!git(&r.wt, &["diff", &base]).is_empty());
    assert!(!git_ok(&r, &["apply", "--check", "--index", &patch]));

    // Had the worktree been reset to the base, the diff is empty and the
    // patch restores the agent's change.
    git(&r.wt, &["reset", "-q", "--hard", &base]);
    assert!(git(&r.wt, &["diff", &base]).is_empty());
    git(&r.wt, &["apply", "--index", &patch]);
    let readme = std::fs::read_to_string(r.wt.join("README.md")).unwrap();
    assert_eq!(readme, "# demo, committed by the agent\n");
}

#[test]
fn without_a_commit_the_note_says_the_changes_are_uncommitted() {
    let r = quick_repo();
    let base = record_base(&r);
    for i in 0..4 {
        r.write(&format!("docs/{i}.md"), "x\n");
    }
    let (note, _) = hand_over(&r, &base, "files>3");
    assert!(
        note.contains("The changes are uncommitted in this worktree on branch quick/issue-0-test."),
        "{note}"
    );
    assert!(!note.contains("commit(s) after the base"), "{note}");
    assert!(note.contains(&format!("run `git diff {base}`")), "{note}");
}

#[test]
fn a_size_reason_that_comes_with_a_commit_still_reports_the_commit() {
    // Rules 4 and 5 are decided before rule 6, so an agent that committed a
    // large change hands over as files>N; the note is read from git, not
    // from the reason.
    let r = quick_repo();
    let base = record_base(&r);
    for i in 0..4 {
        r.write(&format!("docs/{i}.md"), "x\n");
    }
    git(&r.wt, &["add", "-A"]);
    git(&r.wt, &["commit", "-q", "-m", "agent"]);
    let (note, _) = hand_over(&r, &base, "files>3");
    assert!(note.contains("1 commit(s) after the base commit"), "{note}");
    assert!(!note.contains("The changes are uncommitted"), "{note}");
}

/// The repository's remote is a local path, so no provider identity can be
/// looked up, and the temp `HOME` holds no git identity.
#[test]
fn a_run_that_could_never_commit_stops_before_the_agent() {
    let r = quick_repo();
    let o = run_step(&r, "quick-change-record-base", &[]);
    assert!(!o.status.success(), "{}", stdout(&o));
    assert!(
        stderr(&o).contains("no safe commit identity"),
        "{}",
        stderr(&o)
    );
    assert!(stdout(&o).is_empty(), "no base is output");
}

#[test]
fn record_base_outputs_only_the_base_when_an_identity_resolves() {
    let r = quick_repo();
    assert_eq!(record_base(&r).len(), 40);
}

/// Runs `quick-change-create-pr` on a committed change with the given stored
/// checks results and a stub `gh`; returns its arguments and the PR body.
fn create_pr(summary: &str, table: &str) -> (String, String) {
    create_pr_repaired(summary, table, "no")
}

fn create_pr_repaired(summary: &str, table: &str, repaired: &str) -> (String, String) {
    let r = quick_repo();
    let base = record_base(&r);
    r.write("README.md", "# edited\n");
    git(&r.wt, &["add", "-A"]);
    git(&r.wt, &["commit", "-q", "-m", "change"]);

    let bin = r.tmp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let args_file = r.tmp.path().join("gh-args");
    let body_file = r.tmp.path().join("gh-body");
    let gh = bin.join("gh");
    let script = format!(
        "#!/bin/bash\nprintf '%s\\n' \"$@\" > '{a}'\nwhile [ $# -gt 0 ]; do [ \"$1\" = --body-file ] && cp \"$2\" '{b}'; shift; done\necho https://github.com/o/r/pull/1\n",
        a = args_file.display(),
        b = body_file.display()
    );
    std::fs::write(&gh, script).unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let root = r.tmp.path().join(".amplihack/state/quick-change");
    let state = std::fs::read_dir(root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(state.join("checks-summary"), format!("{summary}\n")).unwrap();
    std::fs::write(state.join("checks-table.md"), table).unwrap();

    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let o = run_step(
        &r,
        "quick-change-create-pr",
        &[
            ("PATH", &path),
            ("RECIPE_VAR_task_description", "Fix the link"),
            ("RECIPE_VAR_quick_change_base", &base),
            ("RECIPE_VAR_quick_change_checks", summary),
            ("RECIPE_VAR_quick_change_commit_repaired", repaired),
            ("RECIPE_VAR_quick_change_push_repaired", "no"),
        ],
    );
    assert!(o.status.success(), "create-pr: {}", stderr(&o));
    (
        std::fs::read_to_string(args_file).unwrap(),
        std::fs::read_to_string(body_file).unwrap(),
    )
}

#[test]
fn no_check_found_opens_a_draft_that_says_so() {
    let (args, body) = create_pr("QUICK_CHANGE_CHECKS=none ran=0 failed=0 skipped=0", "");
    assert!(args.lines().any(|l| l == "--draft"), "{args}");
    assert!(body.starts_with("No checks ran\n"), "{body}");
}

#[test]
fn every_check_skipped_opens_a_draft_that_says_so() {
    let (args, body) = create_pr(
        "QUICK_CHANGE_CHECKS=none ran=0 failed=0 skipped=1",
        "| `cargo fmt --all -- --check` | skipped (cargo not on PATH) |\n",
    );
    assert!(args.lines().any(|l| l == "--draft"), "{args}");
    assert!(body.starts_with("No checks ran\n"), "{body}");
    assert!(body.contains("skipped (cargo not on PATH)"), "{body}");
}

#[test]
fn failed_checks_still_open_a_draft_that_says_so() {
    let (args, body) = create_pr(
        "QUICK_CHANGE_CHECKS=failed ran=1 failed=1 skipped=0",
        "| `cargo fmt --all -- --check` | exit 1 |\n",
    );
    assert!(args.lines().any(|l| l == "--draft"), "{args}");
    assert!(body.starts_with("Checks failed\n\n## Request"), "{body}");
}

#[test]
fn hook_repair_opens_a_draft_that_says_so() {
    let (args, body) = create_pr_repaired(
        "QUICK_CHANGE_CHECKS=passed ran=1 failed=0 skipped=0",
        "| `cargo fmt --all -- --check` | exit 0 |\n",
        "yes",
    );
    assert!(args.lines().any(|l| l == "--draft"), "{args}");
    assert!(
        body.starts_with("Hook repair changed the code after the checks ran"),
        "{body}"
    );
}

#[test]
fn passed_checks_open_a_ready_pr() {
    let (args, body) = create_pr(
        "QUICK_CHANGE_CHECKS=passed ran=1 failed=0 skipped=0",
        "| `cargo fmt --all -- --check` | exit 0 |\n",
    );
    assert!(!args.lines().any(|l| l == "--draft"), "{args}");
    assert!(!body.contains("No checks ran"), "{body}");
}
