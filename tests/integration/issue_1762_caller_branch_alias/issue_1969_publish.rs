//! #1969: step 15 pushes with `git push -u origin "HEAD:refs/heads/$TARGET"`
//! and decides by ancestry against `refs/remotes/origin/$TARGET` after a
//! fetch, never against `@{u}` (R2, R3, R4, R5; D5).
//!
//! Each test builds a task worktree the way step 4 does, runs step 15 exactly
//! as the recipe holds it, and reads the bare origin directly. Every run
//! parses the `commit_result` line with `Run::json`, so a test that passes has
//! also shown that the line is valid JSON on that path. The branch names avoid
//! `feature`, `feat`, `branch` and `HEAD`, which `verify_pushed_ref.sh`
//! refuses as placeholders.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::fixture::*;
use super::issue_1969_step04::step4_from;

const TARGET: &str = "feat/issue-1969-publish";

/// Run step 15 in `wt` with `RECIPE_VAR_worktree_setup` set to `setup`, and
/// `AMPLIHACK_HOME` set to `home` when one is given.
pub fn step15(fx: &Fx, wt: &Path, setup: &Value, home: Option<&Path>) -> Run {
    let mut command = fx.bash(wt);
    command
        .arg("-c")
        .arg(step_text(
            "workflow-publish",
            "step-15-commit-push",
            "command",
        ))
        .env("RECIPE_VAR_worktree_setup", setup.to_string())
        .env("REPO_PATH", &fx.caller)
        .env(
            "TASK_DESCRIPTION",
            "implement the delegation receiver for issue 1969",
        )
        .env("ISSUE_NUMBER", "1969")
        .env("REMOTE_HOST_TYPE", "github");
    if let Some(home) = home {
        command.env("AMPLIHACK_HOME", home);
    }
    run(&mut command)
}

pub fn setup_for(wt: &Path, branch: &str) -> Value {
    json!({ "worktree_path": wt.to_str().unwrap(), "branch_name": branch })
}

/// A linked task worktree at `<main>/worktrees/<name>` on new branch `name`,
/// cut from origin/main. `track` reproduces git's default
/// `branch.autoSetupMerge=true` (the #1969 incident); otherwise `--no-track`.
pub fn task_worktree(fx: &Fx, name: &str, track: bool) -> PathBuf {
    let dir = fx.caller.join("worktrees").join(name);
    fx.git(
        &fx.caller,
        &[
            "worktree",
            "add",
            "-q",
            if track { "--track" } else { "--no-track" },
            "-b",
            name,
            "--",
            dir.to_str().unwrap(),
            "origin/main",
        ],
    );
    dir
}

/// `@{u}` of the branch checked out in `wt`, or None.
fn upstream(fx: &Fx, wt: &Path) -> Option<String> {
    let out = fx.git_output(
        wt,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    );
    (out.code == Some(0)).then(|| out.stdout.trim().to_owned())
}

/// A second clone pushes one commit to origin/<branch>. The caller's
/// refs/remotes/origin/<branch> is left stale, so only a fetch sees it.
fn push_from_other_clone(fx: &Fx, branch: &str) -> String {
    let other = fx.root.join("other-clone");
    fx.git(
        &fx.root,
        &["clone", "-q", ORIGIN_URL, other.to_str().unwrap()],
    );
    for (key, value) in [
        ("user.email", "other@example.com"),
        ("user.name", "Other Worker"),
        ("commit.gpgsign", "false"),
    ] {
        fx.git(&other, &["config", key, value]);
    }
    fx.git(&other, &["switch", "-q", branch]);
    fx.commit_in(&other, "upstream.txt", "upstream work\n", "upstream work");
    fx.git(&other, &["push", "-q", "origin", branch]);
    fx.head(&other)
}

/// Step 15 pushed `wt`'s HEAD to origin/<target>, set the upstream to it, and
/// left origin/main where it was.
pub fn assert_pushed(fx: &Fx, wt: &Path, run: &Run, target: &str, main_before: &str, why: &str) {
    assert_eq!(run.code, Some(0), "{why}\n{}", run.describe());
    let result = run.json();
    assert_eq!(result["pushed"], "true", "{why}\n{}", run.describe());
    assert_eq!(result["reason"], "pushed", "{why}\n{}", run.describe());
    let head = fx.head(wt);
    assert_eq!(
        fx.origin_sha(target).as_deref(),
        Some(head.as_str()),
        "{why}: origin/<target> must hold HEAD\n{}",
        run.describe()
    );
    assert_eq!(
        result["sha"],
        head.as_str(),
        "{why}: sha is the pushed HEAD\n{}",
        run.describe()
    );
    assert_eq!(
        upstream(fx, wt).as_deref(),
        Some(format!("origin/{target}").as_str()),
        "{why}: `-u` makes origin/<target> the upstream"
    );
    assert_eq!(
        fx.origin_sha("main").as_deref(),
        Some(main_before),
        "{why}: origin/main must not move"
    );
}

pub fn origin_main(fx: &Fx) -> String {
    fx.origin_sha("main").expect("origin/main exists")
}

/// R11(a): a branch that step 4 cut from origin/<base> with no setup push
/// has no upstream, and step 15 publishes it with a first push.
#[test]
fn new_branch_without_setup_push_is_first_pushed() {
    let fx = Fx::new();
    fx.git(&fx.caller, &["config", "push.default", "simple"]);
    let main_before = origin_main(&fx);
    // Step 4's best-effort setup push fails: the pushurl leads nowhere.
    let nowhere = fx.root.join("no-such-remote.git");
    fx.git(
        &fx.caller,
        &["config", "remote.origin.pushurl", nowhere.to_str().unwrap()],
    );

    let step4 = step4_from(&fx, &fx.caller, Some("1"), &[], None);
    assert_eq!(step4.code, Some(0), "step 4\n{}", step4.describe());
    let setup = step4.json();
    let b = setup["branch_name"]
        .as_str()
        .expect("branch_name")
        .to_owned();
    let wt = PathBuf::from(setup["worktree_path"].as_str().expect("worktree_path"));
    assert!(
        b.starts_with("feat/issue-1969-"),
        "the main clone gets a derived branch\n{}",
        step4.describe()
    );
    assert_eq!(
        fx.config_get(&format!("branch.{b}.merge")),
        "",
        "--no-track: with no setup push the new branch has no upstream, never origin/main\n{}",
        step4.describe()
    );
    assert_eq!(fx.config_get(&format!("branch.{b}.remote")), "");
    assert_eq!(fx.origin_sha(&b), None, "nothing reached origin in step 4");

    fx.git(&fx.caller, &["config", "--unset", "remote.origin.pushurl"]);
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let run = step15(&fx, &wt, &setup, None);

    assert_pushed(&fx, &wt, &run, &b, &main_before, "first push");
    let info = format!("INFO: first push of '{b}' to origin (#1969)");
    assert!(
        run.stderr.lines().any(|l| l.contains(&info)),
        "expected `{info}`\n{}",
        run.describe()
    );
    assert_eq!(run.json()["branch"], b.as_str(), "{}", run.describe());
}

/// R11(b): the #1969 incident. The branch's upstream is origin/main, as
/// `worktree add -b <b> <wt> origin/main` leaves it, and the push still goes
/// to origin/<b>. Under `simple` a bare push exits 128, under `nothing` it
/// refuses, and under `upstream` it would publish to main.
#[test]
fn upstream_preset_to_base_still_pushes_to_own_branch() {
    for push_default in [None, Some("simple"), Some("nothing"), Some("upstream")] {
        let why = format!("push.default={}", push_default.unwrap_or("unset"));
        let fx = Fx::new();
        if let Some(mode) = push_default {
            fx.git(&fx.caller, &["config", "push.default", mode]);
        }
        let main_before = origin_main(&fx);
        let wt = task_worktree(&fx, TARGET, true);
        assert_eq!(
            upstream(&fx, &wt).as_deref(),
            Some("origin/main"),
            "{why}: fixture reproduces the incident's upstream"
        );
        fx.write_in(&wt, "delegation.txt", "receiver model\n");

        let run = step15(&fx, &wt, &setup_for(&wt, TARGET), None);

        assert_ne!(run.code, Some(128), "{why}: not the incident's exit 128");
        assert_pushed(&fx, &wt, &run, TARGET, &main_before, &why);
        assert!(
            run.stderr_line_with(&["first push of", TARGET, "(#1969)"])
                .is_some(),
            "{why}\n{}",
            run.describe()
        );
    }
}

/// The #282 report, counted from the push plan: `ahead of origin.` before the
/// first push, `ahead of origin/<TARGET>.` afterwards.
#[test]
fn nothing_staged_reports_commits_ahead_of_origin() {
    let fx = Fx::new();
    let main_before = origin_main(&fx);
    let wt = task_worktree(&fx, TARGET, false);
    fx.commit_in(&wt, "one.txt", "1\n", "wip: delegation receiver model");
    fx.commit_in(&wt, "two.txt", "2\n", "wip: delegation receiver tests");

    let run = step15(&fx, &wt, &setup_for(&wt, TARGET), None);

    assert_pushed(
        &fx,
        &wt,
        &run,
        TARGET,
        &main_before,
        "nothing staged, first push",
    );
    let first = format!("current branch '{TARGET}' is 2 commit(s) ahead of origin.");
    assert!(
        run.stderr.lines().any(|l| l.trim_end().ends_with(&first)),
        "expected a line ending `{first}`\n{}",
        run.describe()
    );
    assert!(
        run.stderr.contains("wip: delegation receiver model")
            && run.stderr.contains("wip: delegation receiver tests"),
        "the commits are listed\n{}",
        run.describe()
    );

    fx.commit_in(&wt, "three.txt", "3\n", "wip: third");
    let run = step15(&fx, &wt, &setup_for(&wt, TARGET), None);

    assert_pushed(
        &fx,
        &wt,
        &run,
        TARGET,
        &main_before,
        "nothing staged, fast-forward",
    );
    let later = format!("current branch '{TARGET}' is 1 commit(s) ahead of origin/{TARGET}.");
    assert!(
        run.stderr.lines().any(|l| l.trim_end().ends_with(&later)),
        "expected a line ending `{later}`\n{}",
        run.describe()
    );
}

/// The plan is computed after the commit: a pushed branch that gains a commit
/// in step 15 is a fast-forward of one, not `already-pushed`.
#[test]
fn new_commit_on_pushed_branch_is_fast_forwarded() {
    let fx = Fx::new();
    let main_before = origin_main(&fx);
    let wt = task_worktree(&fx, TARGET, false);
    fx.commit_in(&wt, "one.txt", "1\n", "first");
    fx.git(&wt, &["push", "-q", "-u", "origin", TARGET]);
    let pushed_before = fx.head(&wt);
    fx.write_in(&wt, "delegation.txt", "receiver model\n");

    let run = step15(&fx, &wt, &setup_for(&wt, TARGET), None);

    assert_ne!(
        fx.head(&wt),
        pushed_before,
        "step 15 committed the staged change\n{}",
        run.describe()
    );
    assert_pushed(
        &fx,
        &wt,
        &run,
        TARGET,
        &main_before,
        "new commit on a pushed branch",
    );
    assert!(
        run.stderr_line_with(&["fast-forward push, no rebase (#978)"])
            .is_some(),
        "{}",
        run.describe()
    );
}

/// R11(b2): behind origin/<TARGET>. Step 15 fetches first, so a commit that
/// another clone pushed is seen; with a local commit too, the #978 refusal.
#[test]
fn diverged_from_origin_target_is_refused_with_evidence() {
    let fx = Fx::new();
    let wt = task_worktree(&fx, TARGET, false);
    fx.commit_in(&wt, "one.txt", "1\n", "first");
    fx.git(&wt, &["push", "-q", "-u", "origin", TARGET]);
    let theirs = push_from_other_clone(&fx, TARGET);
    fx.commit_in(&wt, "local.txt", "local\n", "local work");
    let head_before = fx.head(&wt);

    let run = step15(&fx, &wt, &setup_for(&wt, TARGET), None);

    assert_eq!(run.code, Some(1), "{}", run.describe());
    let result = run.json();
    assert_eq!(result["pushed"], "false", "{}", run.describe());
    let reason = result["reason"].as_str().unwrap_or_default();
    assert!(
        reason.starts_with("diverged-upstream: ahead=1 behind=1 merge_base=")
            && reason.ends_with(&format!(" upstream=origin/{TARGET}")),
        "reason must carry the evidence against origin/<TARGET>: {reason:?}\n{}",
        run.describe()
    );
    assert!(
        run.stderr.contains("refuses to rebase divergent history")
            && run.stderr.contains("issue #978"),
        "{}",
        run.describe()
    );
    assert_eq!(
        fx.origin_sha(TARGET).as_deref(),
        Some(theirs.as_str()),
        "origin/<TARGET> is untouched"
    );
    assert_eq!(fx.head(&wt), head_before, "no rebase, no rewrite");
}

/// R11(b2): strictly behind and nothing to commit is `already-pushed`; no
/// push, origin unchanged.
#[test]
fn behind_origin_target_with_nothing_new_is_already_pushed() {
    let fx = Fx::new();
    let wt = task_worktree(&fx, TARGET, false);
    fx.commit_in(&wt, "one.txt", "1\n", "first");
    fx.git(&wt, &["push", "-q", "-u", "origin", TARGET]);
    let theirs = push_from_other_clone(&fx, TARGET);
    let head_before = fx.head(&wt);

    let run = step15(&fx, &wt, &setup_for(&wt, TARGET), None);

    assert_eq!(run.code, Some(0), "{}", run.describe());
    let result = run.json();
    assert_eq!(result["pushed"], "true", "{}", run.describe());
    assert_eq!(result["reason"], "already-pushed", "{}", run.describe());
    assert!(
        run.stderr_line_with(&["not ahead of", &format!("origin/{TARGET}")])
            .is_some(),
        "{}",
        run.describe()
    );
    assert_eq!(fx.origin_sha(TARGET).as_deref(), Some(theirs.as_str()));
    assert_eq!(fx.head(&wt), head_before);
}

/// R4: a stale `@{u}` steers neither the decision nor the destination.
#[test]
fn stale_upstream_does_not_steer_decision_or_destination() {
    // (1) @{u} is origin/main, which has moved on: against @{u} the branch
    // looks diverged, against origin/<TARGET> it is a fast-forward.
    let fx = Fx::new();
    let wt = task_worktree(&fx, TARGET, false);
    fx.commit_in(&wt, "one.txt", "1\n", "first");
    fx.git(&wt, &["push", "-q", "origin", TARGET]);
    fx.commit_in(&fx.caller, "base-moves.txt", "base\n", "base moves on");
    fx.git(&fx.caller, &["push", "-q", "origin", "main"]);
    let main_before = origin_main(&fx);
    fx.git(&wt, &["branch", "-q", "--set-upstream-to=origin/main"]);
    fx.write_in(&wt, "delegation.txt", "receiver model\n");

    let run = step15(&fx, &wt, &setup_for(&wt, TARGET), None);
    assert_pushed(
        &fx,
        &wt,
        &run,
        TARGET,
        &main_before,
        "@{u} is a moved-on origin/main",
    );

    // (2) @{u} is another remote branch already at HEAD: against @{u} there
    // is nothing to push, but origin/<TARGET> does not exist yet.
    let fx = Fx::new();
    let main_before = origin_main(&fx);
    let wt = task_worktree(&fx, TARGET, false);
    fx.commit_in(&wt, "one.txt", "1\n", "first");
    let mirror = "feat/issue-1969-mirror";
    fx.git(
        &wt,
        &["push", "-q", "origin", &format!("HEAD:refs/heads/{mirror}")],
    );
    fx.git(
        &wt,
        &[
            "branch",
            "-q",
            &format!("--set-upstream-to=origin/{mirror}"),
        ],
    );
    let mirror_before = fx.origin_sha(mirror);

    let run = step15(&fx, &wt, &setup_for(&wt, TARGET), None);
    assert_pushed(
        &fx,
        &wt,
        &run,
        TARGET,
        &main_before,
        "@{u} is a mirror at HEAD",
    );
    assert_eq!(
        fx.origin_sha(mirror),
        mirror_before,
        "the mirror is untouched"
    );
}

/// R9, D5: without the push plan helper, step 15 exits 2 before it commits
/// or pushes.
#[test]
fn missing_push_plan_stops_before_commit_or_push() {
    let fx = Fx::new();
    let wt = task_worktree(&fx, TARGET, true);
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let home = fx.bundle_without(PUSH_PLAN);
    let head_before = fx.head(&wt);
    let refs_before = fx.origin_refs();

    let run = step15(&fx, &wt, &setup_for(&wt, TARGET), Some(&home));

    assert_eq!(run.code, Some(2), "{}", run.describe());
    assert!(
        run.stderr_line_with(&[
            "ERROR:",
            "workflow push plan helper not found",
            PUSH_PLAN,
            "(#1969)"
        ])
        .is_some(),
        "{}",
        run.describe()
    );
    assert_eq!(fx.head(&wt), head_before, "nothing committed");
    assert_eq!(fx.origin_refs(), refs_before, "nothing pushed");
    assert_eq!(run.json()["pushed"], "false", "{}", run.describe());
}

/// D5.1: an empty TARGET (detached HEAD) refuses before anything is staged.
#[test]
fn detached_head_has_no_push_target() {
    let fx = Fx::new();
    let wt = fx.caller.join("worktrees/detached-task");
    fx.git(
        &fx.caller,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            wt.to_str().unwrap(),
            "origin/main",
        ],
    );
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let head_before = fx.head(&wt);
    let refs_before = fx.origin_refs();

    let run = step15(
        &fx,
        &wt,
        &json!({ "worktree_path": wt.to_str().unwrap() }),
        None,
    );

    assert_eq!(run.code, Some(1), "{}", run.describe());
    let result = run.json();
    assert_eq!(result["pushed"], "false", "{}", run.describe());
    assert_eq!(result["reason"], "no-push-target", "{}", run.describe());
    assert!(
        run.stderr_line_with(&["ERROR:", "cannot resolve the branch to push", "(#1969)"])
            .is_some(),
        "{}",
        run.describe()
    );
    assert_eq!(fx.head(&wt), head_before, "nothing committed");
    assert_eq!(fx.origin_refs(), refs_before, "nothing pushed");
}

/// R5: the #1762 guard now covers every branch, not only aliases. A task
/// worktree whose branch is not worktree_setup.branch_name is refused.
#[test]
fn push_target_must_match_setup_branch_for_every_branch() {
    let fx = Fx::new();
    let wt = task_worktree(&fx, TARGET, false);
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let head_before = fx.head(&wt);
    let refs_before = fx.origin_refs();

    let run = step15(&fx, &wt, &setup_for(&wt, "feat/issue-1969-other"), None);

    assert_eq!(run.code, Some(1), "{}", run.describe());
    assert!(
        run.stderr_line_with(&["ERROR:", "refusing to push", "#1762", TARGET])
            .is_some(),
        "{}",
        run.describe()
    );
    assert_eq!(fx.head(&wt), head_before, "nothing committed");
    assert_eq!(fx.origin_refs(), refs_before, "nothing pushed");
    assert_eq!(run.json()["pushed"], "false", "{}", run.describe());
}
