//! Defect 2 end to end: the real `step-15-commit-push` command commits only
//! task changes under a subject built from the staged index, then pushes
//! through `workflow_sync_pr_head.sh --mode step15`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value as JsonValue;
use serde_yaml::Value;

use super::{Sandbox, dump, helper_path, read, recipe_path, text, write_file};

fn step_command(id: &str) -> String {
    let recipe: Value = serde_yaml::from_str(&read(&recipe_path("workflow-publish")))
        .expect("parse workflow-publish");
    recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|s| s.get("command").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("step {id} must be a bash command step"))
        .to_string()
}

const CARGO_TOML: &str = "[package]\nname = \"demo\"\nversion = \"0.5.0\"\nedition = \"2021\"\n";
const CARGO_LOCK: &str = "version = 4\n\n[[package]]\nname = \"demo\"\nversion = \"0.5.0\"\n";

struct Fixture {
    origin: PathBuf,
    worktree: PathBuf,
}

/// Bare origin, a clone with a published `feature` branch, and the linked
/// task worktree step 15 requires (#1528).
fn fixture(sb: &Sandbox) -> Fixture {
    let fx = sb.remote_fixture();
    write_file(&fx.repo.join("Cargo.toml"), CARGO_TOML);
    write_file(&fx.repo.join("Cargo.lock"), CARGO_LOCK);
    write_file(&fx.repo.join("src/lib.rs"), "pub fn a() {}\n");
    sb.git(&fx.repo, &["add", "-A"]);
    sb.git(&fx.repo, &["commit", "-q", "-m", "crate"]);
    sb.git(&fx.repo, &["push", "-q", "origin", "feature"]);
    sb.git(&fx.repo, &["switch", "-q", "main"]);
    let worktree = sb.root().join("worktree");
    sb.git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "feature",
        ],
    );
    Fixture {
        origin: fx.origin,
        worktree,
    }
}

fn run_step15(sb: &Sandbox, worktree: &Path, env: &[(&str, &str)]) -> (Output, JsonValue) {
    let mut cmd = sb.command("bash", worktree);
    cmd.arg("-c")
        .arg(step_command("step-15-commit-push"))
        .env("WORKTREE_SETUP_WORKTREE_PATH", worktree)
        .env(
            "WORKFLOW_RUNTIME_ARTIFACT_HELPER",
            helper_path("workflow_runtime_artifacts.sh"),
        )
        .env(
            "TASK_DESCRIPTION",
            "Continue fixing the bug in issue #130 on the existing branch",
        )
        .env("ISSUE_NUMBER", "130")
        .env("REMOTE_HOST_TYPE", "github");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run step-15");
    let stdout = text(&output.stdout);
    let json = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .map(|l| serde_json::from_str(l).expect("commit_result JSON"))
        .unwrap_or(JsonValue::Null);
    (output, json)
}

#[test]
fn step15_commits_only_task_changes_and_pushes() {
    let sb = Sandbox::new();
    let fx = fixture(&sb);
    let wt = &fx.worktree;
    write_file(&wt.join("src/lib.rs"), "pub fn a() {}\npub fn b() {}\n");
    write_file(
        &wt.join("Cargo.toml"),
        &CARGO_TOML.replace("0.5.0", "0.6.0"),
    );
    write_file(
        &wt.join("Cargo.lock"),
        &CARGO_LOCK.replace("0.5.0", "0.6.0"),
    );
    write_file(
        &wt.join(".github/hooks/pre-commit"),
        "#!/bin/sh\n# amplihack stub\n",
    );

    let (output, json) = run_step15(&sb, wt, &[]);
    assert!(output.status.success(), "{}", dump(&output));
    assert_eq!(json["pushed"], "true", "{json}\n{}", dump(&output));

    let committed = sb.git(wt, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(
        committed, "src/lib.rs",
        "only the task change may be committed"
    );
    assert_eq!(
        fs::read_to_string(wt.join("Cargo.toml")).unwrap(),
        CARGO_TOML
    );
    assert_eq!(
        fs::read_to_string(wt.join("Cargo.lock")).unwrap(),
        CARGO_LOCK
    );
    assert!(
        wt.join(".github/hooks/pre-commit").exists(),
        "the stub stays on disk"
    );
    let status = sb.git(wt, &["status", "--porcelain", "--untracked-files=all"]);
    assert!(
        status.is_empty(),
        "later git add -A steps must find nothing to sweep\n{status}"
    );
    let head = sb.head(wt);
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature").as_deref(),
        Some(head.as_str())
    );
    assert_eq!(json["sha"], head.as_str());
}

#[test]
fn step15_subject_and_body_describe_the_commit_not_the_task() {
    let sb = Sandbox::new();
    let fx = fixture(&sb);
    let wt = &fx.worktree;
    write_file(&wt.join("src/lib.rs"), "pub fn a() {}\npub fn c() {}\n");
    let (output, _) = run_step15(
        &sb,
        wt,
        &[("RECIPE_VAR_version_bump__change_classification", "PATCH")],
    );
    assert!(output.status.success(), "{}", dump(&output));

    let subject = sb.git(wt, &["log", "-1", "--format=%s"]);
    assert_eq!(subject, "fix: update src/lib.rs");
    let body = sb.git(wt, &["log", "-1", "--format=%b"]);
    assert!(
        body.contains("src/lib.rs") && body.contains("1 file changed"),
        "stat body\n{body}"
    );
    for stale in [
        "Continue fixing",
        "Implements",
        "Implementation as per design specification",
        "Changes:",
    ] {
        assert!(
            !body.contains(stale) && !subject.contains(stale),
            "`{stale}` must be gone\n{body}"
        );
    }
    let last = body
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    assert!(
        last == "Closes #130" || last == "Refs #130",
        "issue reference last\n{body}"
    );
}

#[test]
fn step15_long_multi_dir_subject_stays_within_72_characters() {
    let sb = Sandbox::new();
    let fx = fixture(&sb);
    let wt = &fx.worktree;
    for dir in [
        "amplifier-bundle/tools",
        "docs/reference",
        "tests/integration",
        "crates/x",
    ] {
        write_file(&wt.join(dir).join("some_rather_long_file_name.rs"), "x\n");
    }
    let (output, _) = run_step15(&sb, wt, &[]);
    assert!(output.status.success(), "{}", dump(&output));
    let subject = sb.git(wt, &["log", "-1", "--format=%s"]);
    assert!(subject.chars().count() <= 72, "{subject}");
    assert!(subject.starts_with("fix: update 4 files in "), "{subject}");
}

#[test]
fn step15_pushes_earlier_unpushed_commits_when_nothing_is_left_to_commit() {
    let sb = Sandbox::new();
    let fx = fixture(&sb);
    let wt = &fx.worktree;
    sb.commit_file(
        wt,
        "src/lib.rs",
        "pub fn already_committed() {}\n",
        "work committed earlier",
    );
    let head = sb.head(wt);
    let (output, json) = run_step15(&sb, wt, &[]);
    assert!(output.status.success(), "{}", dump(&output));
    assert_eq!(sb.head(wt), head, "no new commit");
    assert_eq!(json["pushed"], "true", "{json}");
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature").as_deref(),
        Some(head.as_str())
    );
}

#[test]
fn step15_fails_closed_without_staging_when_a_helper_is_missing() {
    let sb = Sandbox::new();
    let fx = fixture(&sb);
    let wt = &fx.worktree;
    write_file(&wt.join("src/lib.rs"), "pub fn z() {}\n");
    let head = sb.head(wt);
    let empty_home = sb.root().join("empty-amplihack-home");
    fs::create_dir_all(&empty_home).unwrap();

    let (output, json) = run_step15(&sb, wt, &[("AMPLIHACK_HOME", empty_home.to_str().unwrap())]);
    assert_eq!(output.status.code(), Some(2), "{}", dump(&output));
    let reason = json["reason"].as_str().unwrap_or_default();
    assert!(reason.starts_with("helper-missing:workflow_"), "{json}");
    assert_eq!(json["pushed"], "false");
    assert!(text(&output.stderr).contains("ERROR"), "{}", dump(&output));
    assert_eq!(sb.head(wt), head, "nothing committed");
    assert!(
        sb.git(wt, &["diff", "--cached", "--name-only"]).is_empty(),
        "nothing staged"
    );
}
