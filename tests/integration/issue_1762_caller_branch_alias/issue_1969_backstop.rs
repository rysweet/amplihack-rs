//! #1969: step 15's pre-staging checks that `issue_1969_publish.rs` does not
//! cover: the default-branch backstop (A5), the order of the five checks, and
//! a branch name full of shell metacharacters (V1, V4, V7).
//!
//! The checks run before anything is staged, in this order, and the first
//! that fails decides the exit status and the `reason`
//! (docs/features/workflow-publish-ancestry-aware-step15.md):
//!
//! 1. the resolver is present (exit 2);
//! 2. `TARGET` is not empty (exit 1, `no-push-target`);
//! 3. `TARGET` equals `worktree_setup.branch_name` (exit 1, #1762);
//! 4. `TARGET` is not a default branch (exit 1, `default-branch-target`), as
//!    `workflow_default_branch.sh --ask-origin` decides; a missing or failing
//!    rule helper exits 2;
//! 5. `workflow_push_plan.sh` is present (exit 2).
//!
//! Step 15 runs the one shared rule with `--ask-origin`: `main` and `master`
//! always; the target of the local `refs/remotes/origin/HEAD` when that
//! resolves; otherwise what `git ls-remote --symref origin HEAD` reports, and
//! `develop` only when origin gives no answer. A resolvable local ref is
//! trusted without asking `origin`.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::fixture::*;
use super::issue_1969_publish::{assert_pushed, origin_main, setup_for, step15, task_worktree};

/// The exact refusal line; it is a pinned contract string.
fn default_branch_error(target: &str) -> String {
    format!(
        "ERROR: push target '{target}' is the default branch; step 15 never pushes to it (#1969, #858)."
    )
}

/// A linked worktree at `<main>/worktrees/<branch>` on the existing local
/// branch `branch`.
fn worktree_on(fx: &Fx, branch: &str) -> PathBuf {
    let dir = fx.caller.join("worktrees").join(branch);
    fx.git(
        &fx.caller,
        &["worktree", "add", "-q", "--", dir.to_str().unwrap(), branch],
    );
    dir
}

/// Local branch `name` cut from origin/main with no upstream, and pushed to
/// origin/<name>, so that refs/remotes/origin/<name> exists too.
fn published_branch(fx: &Fx, name: &str) {
    fx.git(
        &fx.caller,
        &["branch", "-q", "--no-track", name, "origin/main"],
    );
    fx.git(
        &fx.caller,
        &["push", "-q", "origin", &format!("{name}:refs/heads/{name}")],
    );
}

/// Point the bare origin's own HEAD at `refs/heads/<branch>`.
fn set_origin_head(fx: &Fx, branch: &str) {
    fx.git(
        &fx.origin_bare(),
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
    );
}

/// Remove the caller's `refs/remotes/origin/HEAD`, as in a clone made
/// without it or after `git remote set-head origin --delete`.
fn unset_local_origin_head(fx: &Fx) {
    fx.git(&fx.caller, &["remote", "set-head", "origin", "--delete"]);
    let out = fx.git_output(
        &fx.caller,
        &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"],
    );
    assert_ne!(out.code, Some(0), "fixture: origin/HEAD must not resolve");
}

fn staged(fx: &Fx, wt: &Path) -> String {
    fx.git(wt, &["diff", "--cached", "--name-only"])
}

/// What must hold after any pre-staging refusal: nothing staged, committed
/// or pushed.
struct Before {
    head: String,
    refs: Vec<String>,
}

fn before(fx: &Fx, wt: &Path) -> Before {
    Before {
        head: fx.head(wt),
        refs: fx.origin_refs(),
    }
}

fn assert_untouched(fx: &Fx, wt: &Path, run: &Run, before: &Before, why: &str) {
    assert_eq!(
        fx.head(wt),
        before.head,
        "{why}: nothing committed\n{}",
        run.describe()
    );
    assert_eq!(
        staged(fx, wt),
        "",
        "{why}: the refusal comes before staging\n{}",
        run.describe()
    );
    assert_eq!(
        fx.origin_refs(),
        before.refs,
        "{why}: nothing pushed\n{}",
        run.describe()
    );
}

fn assert_default_branch_refused(
    fx: &Fx,
    wt: &Path,
    run: &Run,
    target: &str,
    before: &Before,
    why: &str,
) {
    assert_eq!(run.code, Some(1), "{why}\n{}", run.describe());
    let result = run.json();
    assert_eq!(result["pushed"], "false", "{why}\n{}", run.describe());
    assert_eq!(
        result["reason"],
        "default-branch-target",
        "{why}\n{}",
        run.describe()
    );
    let line = default_branch_error(target);
    assert!(
        run.stderr.lines().any(|l| l.contains(&line)),
        "{why}: expected the full line\n  {line}\n{}",
        run.describe()
    );
    assert_untouched(fx, wt, run, before, why);
}

/// A5: `main` and `master` are refused whatever origin/HEAD says, even with a
/// matching `worktree_setup.branch_name`.
#[test]
fn main_and_master_are_never_push_targets() {
    // master: origin/HEAD is main, so only the fixed list can refuse it.
    let fx = Fx::new();
    fx.git(
        &fx.caller,
        &["branch", "-q", "--no-track", "master", "origin/main"],
    );
    let wt = worktree_on(&fx, "master");
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let was = before(&fx, &wt);
    let run = step15(&fx, &wt, &setup_for(&wt, "master"), None);
    assert_default_branch_refused(&fx, &wt, &run, "master", &was, "master");
    assert_eq!(fx.origin_sha("master"), None, "master never reaches origin");

    // main: the caller moves off main so a task worktree can hold it.
    let fx = Fx::new();
    let main_before = origin_main(&fx);
    fx.git(&fx.caller, &["switch", "-q", "-c", "caller-side"]);
    let wt = worktree_on(&fx, "main");
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let was = before(&fx, &wt);
    let run = step15(&fx, &wt, &setup_for(&wt, "main"), None);
    assert_default_branch_refused(&fx, &wt, &run, "main", &was, "main");
    assert_eq!(fx.origin_sha("main").as_deref(), Some(main_before.as_str()));

    // main with no worktree_setup.branch_name at all: the #1762 guard is not
    // armed, so only the backstop stands between HEAD and origin/main.
    let run = step15(
        &fx,
        &wt,
        &json!({ "worktree_path": wt.to_str().unwrap() }),
        None,
    );
    assert_default_branch_refused(&fx, &wt, &run, "main", &was, "main, no branch_name");
}

/// A5: when the local `refs/remotes/origin/HEAD` resolves, its target is the
/// default branch, here `trunk`, though origin's own HEAD is still main.
#[test]
fn local_origin_head_target_is_refused() {
    let fx = Fx::new();
    published_branch(&fx, "trunk");
    fx.git(&fx.caller, &["remote", "set-head", "origin", "trunk"]);
    let wt = worktree_on(&fx, "trunk");
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let trunk_before = fx.origin_sha("trunk");
    let was = before(&fx, &wt);

    let run = step15(&fx, &wt, &setup_for(&wt, "trunk"), None);

    assert_default_branch_refused(&fx, &wt, &run, "trunk", &was, "local origin/HEAD is trunk");
    assert_eq!(
        fx.origin_sha("trunk"),
        trunk_before,
        "origin/trunk is unchanged"
    );
}

/// A5, the "Unrecognised default branch" row: origin's HEAD is `trunk`, the
/// clone's `refs/remotes/origin/HEAD` is unset and the task worktree is on
/// `trunk`. Step 4's local-only detection cannot see this; step 15 asks
/// origin with `ls-remote --symref` and refuses.
#[test]
fn remote_head_is_asked_when_local_origin_head_is_unset() {
    let fx = Fx::new();
    published_branch(&fx, "trunk");
    set_origin_head(&fx, "trunk");
    unset_local_origin_head(&fx);
    let symref = fx.git(&fx.caller, &["ls-remote", "--symref", "origin", "HEAD"]);
    assert!(
        symref.lines().any(|l| l == "ref: refs/heads/trunk\tHEAD"),
        "fixture: origin reports HEAD -> trunk, got {symref:?}"
    );
    let wt = worktree_on(&fx, "trunk");
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let trunk_before = fx.origin_sha("trunk");
    let was = before(&fx, &wt);

    let run = step15(&fx, &wt, &setup_for(&wt, "trunk"), None);

    assert_default_branch_refused(&fx, &wt, &run, "trunk", &was, "origin's HEAD is trunk");
    assert_eq!(
        fx.origin_sha("trunk"),
        trunk_before,
        "origin/trunk is unchanged"
    );
    assert!(
        !run.stderr.contains(fx.origin_bare().to_str().unwrap()),
        "the ls-remote query's stderr is discarded, never printed\n{}",
        run.describe()
    );
}

/// Step 15 runs the shared rule with `--ask-origin`: with origin/HEAD unset
/// locally and origin's HEAD on main, origin's answer replaces the `develop`
/// guess, so a task branch called `develop` is pushed. (The read-only mode,
/// which never asks origin, treats it as a default branch; see
/// `ask_origin_without_an_answer_falls_back_to_develop`.)
#[test]
fn develop_is_not_a_default_branch_for_step15() {
    let fx = Fx::new();
    unset_local_origin_head(&fx);
    let main_before = origin_main(&fx);
    let wt = task_worktree(&fx, "develop", false);
    fx.write_in(&wt, "delegation.txt", "receiver model\n");

    let run = step15(&fx, &wt, &setup_for(&wt, "develop"), None);

    assert_pushed(
        &fx,
        &wt,
        &run,
        "develop",
        &main_before,
        "develop, origin/HEAD unset",
    );
}

/// The `ls-remote` query runs only when the local ref is missing: a local
/// `refs/remotes/origin/HEAD` that still says main is trusted even though
/// origin has moved its HEAD to `trunk`, and the push goes ahead. This is the
/// documented bound on the backstop's network access; branch protection on
/// the server is the real guard.
#[test]
fn resolvable_local_origin_head_is_trusted_without_asking_origin() {
    let fx = Fx::new();
    published_branch(&fx, "trunk");
    set_origin_head(&fx, "trunk");
    let main_before = origin_main(&fx);
    let wt = worktree_on(&fx, "trunk");
    fx.write_in(&wt, "delegation.txt", "receiver model\n");

    let run = step15(&fx, &wt, &setup_for(&wt, "trunk"), None);

    assert_pushed(
        &fx,
        &wt,
        &run,
        "trunk",
        &main_before,
        "stale local origin/HEAD -> main",
    );
}

/// The first failing check decides the exit status and the `reason`. Each
/// case breaks two or three checks at once and expects the earliest.
#[test]
fn checks_run_in_documented_order() {
    // 2 before 5: a detached HEAD in a bundle without the push plan exits 1
    // with no-push-target, not 2.
    let fx = Fx::new();
    let home = fx.bundle_without(PUSH_PLAN);
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
    let was = before(&fx, &wt);
    let run = step15(
        &fx,
        &wt,
        &setup_for(&wt, "feat/issue-1969-order"),
        Some(&home),
    );
    assert_eq!(run.code, Some(1), "detached + no plan\n{}", run.describe());
    assert_eq!(run.json()["reason"], "no-push-target", "{}", run.describe());
    assert!(
        !run.stderr.contains("refusing to push (issue #1762)"),
        "2 before 3: the #1762 guard needs a target to compare\n{}",
        run.describe()
    );
    assert_untouched(&fx, &wt, &run, &was, "detached + no plan");

    // 3 before 4 and 5: a worktree on master whose setup names another
    // branch is refused by the #1762 guard, not the backstop.
    let fx = Fx::new();
    let home = fx.bundle_without(PUSH_PLAN);
    fx.git(
        &fx.caller,
        &["branch", "-q", "--no-track", "master", "origin/main"],
    );
    let wt = worktree_on(&fx, "master");
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let was = before(&fx, &wt);
    let run = step15(
        &fx,
        &wt,
        &setup_for(&wt, "feat/issue-1969-order"),
        Some(&home),
    );
    assert_eq!(run.code, Some(1), "mismatch on master\n{}", run.describe());
    assert!(
        run.stderr_line_with(&[
            "ERROR: push target 'master' does not match worktree_setup.branch_name 'feat/issue-1969-order'",
            "refusing to push (issue #1762)."
        ])
        .is_some(),
        "mismatch on master\n{}",
        run.describe()
    );
    let result = run.json();
    assert_eq!(result["pushed"], "false", "{}", run.describe());
    assert_ne!(
        result["reason"],
        "default-branch-target",
        "{}",
        run.describe()
    );
    assert!(
        !run.stderr.contains("is the default branch"),
        "{}",
        run.describe()
    );
    assert_untouched(&fx, &wt, &run, &was, "mismatch on master");

    // 4 before 5: master with a matching setup and no push plan exits 1 with
    // default-branch-target, not 2.
    let run = step15(&fx, &wt, &setup_for(&wt, "master"), Some(&home));
    assert_default_branch_refused(&fx, &wt, &run, "master", &was, "master + no plan");
    assert!(
        !run.stderr.contains("workflow push plan helper not found"),
        "{}",
        run.describe()
    );
}

/// V1, V4, V7: a branch name that `check-ref-format` accepts but that holds a
/// single quote, `$(..)` and backticks is pushed as data. `commit_result` is
/// valid JSON carrying the name unchanged and nothing is executed.
#[test]
fn hostile_branch_name_is_pushed_as_data() {
    let fx = Fx::new();
    let name = format!("feat/issue-1969-{HOSTILE}");
    let check = fx.git(&fx.caller, &["check-ref-format", "--branch", &name]);
    assert_eq!(check.trim_end(), name, "fixture: git accepts the name");
    let main_before = origin_main(&fx);
    let wt = task_worktree(&fx, &name, false);
    fx.write_in(&wt, "delegation.txt", "receiver model\n");

    let run = step15(&fx, &wt, &setup_for(&wt, &name), None);

    fx.assert_nothing_executed(&run, "step 15 with a hostile branch name");
    assert_pushed(&fx, &wt, &run, &name, &main_before, "hostile branch name");
    assert_eq!(run.json()["branch"], name.as_str(), "{}", run.describe());
    assert!(
        run.stderr_line_with(&["first push of '", &name, "(#1969)"])
            .is_some(),
        "the INFO line carries the name verbatim\n{}",
        run.describe()
    );

    // The fast-forward path handles it too.
    fx.write_in(&wt, "more.txt", "more\n");
    let run = step15(&fx, &wt, &setup_for(&wt, &name), None);
    fx.assert_nothing_executed(&run, "fast-forward with a hostile branch name");
    assert_pushed(
        &fx,
        &wt,
        &run,
        &name,
        &main_before,
        "hostile name, fast-forward",
    );
}

/// Check 4 fails closed: without `workflow_default_branch.sh`, step 15 exits 2
/// before staging, with its own `ERROR` line, and pushes nothing. It never
/// treats a missing rule as "not a default branch".
#[test]
fn missing_default_rule_stops_before_staging() {
    let fx = Fx::new();
    let home = fx.bundle_without(DEFAULT_RULE);
    let wt = task_worktree(&fx, "feat/issue-1969-no-rule", false);
    fx.write_in(&wt, "delegation.txt", "receiver model\n");
    let was = before(&fx, &wt);

    let run = step15(
        &fx,
        &wt,
        &setup_for(&wt, "feat/issue-1969-no-rule"),
        Some(&home),
    );

    assert_eq!(run.code, Some(2), "{}", run.describe());
    assert!(
        run.stderr_line_with(&[
            "ERROR: cannot decide whether push target 'feat/issue-1969-no-rule' is the default branch",
            "refusing to push (#1969).",
        ])
        .is_some(),
        "{}",
        run.describe()
    );
    assert_eq!(run.json()["pushed"], "false", "{}", run.describe());
    assert_untouched(&fx, &wt, &run, &was, "no rule helper");
}
