//! #1969: a run started from inside a #1762 alias worktree,
//! `<main>/worktrees/<B>--wt-<n>`, is not a caller branch of its own.
//!
//! Continuing `<B>--wt-<n>` as a branch would publish the alias to origin as a
//! second branch, and continuing `<B>` from inside it would make the planner
//! reuse that very worktree (#858). Step 4 therefore stops before creating
//! anything, names `<B>` and the checkout to re-run from, and never suggests
//! pushing the alias. A nested run started there keeps the #1134 behaviour.

use std::path::Path;

use super::fixture::*;
use super::issue_1969_helpers::{CALLER_BRANCH as X, continues, linked_on_new_branch};
use super::issue_1969_step04::step4_from;

/// Refs and registered worktrees: what "nothing created" is measured against.
fn state(fx: &Fx) -> (String, String, Vec<String>) {
    let mut origin = fx.origin_refs();
    origin.sort();
    (
        fx.git(
            &fx.caller,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads/",
            ],
        ),
        fx.git(&fx.caller, &["worktree", "list", "--porcelain"]),
        origin,
    )
}

/// No stderr line names the alias as something to push or as a branch on
/// origin (#1762: the alias is never pushed as a branch of its own).
fn assert_never_suggests_pushing(run: &Run, alias: &str, why: &str) {
    for line in run.stderr.lines() {
        for bad in [
            format!("origin {alias}"),
            format!("origin '{alias}'"),
            format!("refs/heads/{alias}"),
            format!("origin/{alias}"),
        ] {
            assert!(
                !line.contains(&bad),
                "{why}: a line suggests pushing the alias ({bad}): {line}\n{}",
                run.describe()
            );
        }
    }
}

/// Run step 4 from `alias_path` at depth 1 and check the stop: exit 1, no
/// JSON, nothing created, no continuation, the alias ERROR line naming the
/// alias and `<B>`, and no suggestion to push the alias. Returns the hint line.
fn assert_alias_stop(fx: &Fx, alias_path: &Path, branch: &str, alias: &str) -> String {
    let before = state(fx);
    let run = step4_from(fx, alias_path, Some("1"), &[], None);

    assert_eq!(run.code, Some(1), "{}", run.describe());
    assert!(!run.stdout.contains('{'), "no JSON\n{}", run.describe());
    assert_eq!(
        state(fx),
        before,
        "no branch, worktree or push\n{}",
        run.describe()
    );
    assert!(
        !fx.caller
            .join("worktrees")
            .join(format!("{alias}--wt-1"))
            .exists(),
        "no alias of the alias"
    );
    assert!(
        !continues(&run.stderr),
        "no continuation\n{}",
        run.describe()
    );
    assert!(
        !run.stderr.contains("caller-branch detection failed"),
        "the alias stop is not reported as a detection failure\n{}",
        run.describe()
    );
    assert!(
        run.stderr_line_with(&[
            "ERROR: repo_path is a #1762 alias worktree",
            &format!("'{alias}'"),
            &format!("'{branch}'"),
            &format!("origin/{branch}"),
            "creates nothing",
        ])
        .is_some(),
        "the stop names the alias, <B> and origin/<B>\n{}",
        run.describe()
    );
    assert_never_suggests_pushing(&run, alias, "alias stop");
    let to_continue = format!("ERROR: To continue '{branch}'");
    run.stderr_line_with(&[&to_continue, "-c repo_path="])
        .unwrap_or_else(|| panic!("the hint line\n{}", run.describe()))
        .to_owned()
}

/// The incident's pattern: `cd` into a kept alias task worktree and run from
/// there. Step 4 stops before creating anything and points at the original
/// caller worktree. Following that hint reuses the same alias through the
/// planner; nothing is pushed as `<X>--wt-1`.
#[test]
fn run_from_alias_of_linked_caller_stops_and_points_at_the_caller() {
    let fx = Fx::new();
    let wt = linked_on_new_branch(&fx, X);
    fx.git(&wt, &["push", "-q", "-u", "origin", X]);
    let first = step4_from(&fx, &wt, Some("1"), &[], None);
    assert_eq!(
        first.code,
        Some(0),
        "setup: alias run\n{}",
        first.describe()
    );
    let alias = format!("{X}--wt-1");
    let alias_path = fx.caller.join("worktrees").join(&alias);
    assert_eq!(
        first.json()["local_branch"],
        alias.as_str(),
        "{}",
        first.describe()
    );

    // Work done in the alias, not yet pushed, must not change the advice.
    fx.commit_in(&alias_path, "alias-work.txt", "alias work\n", "alias work");
    let hint = assert_alias_stop(&fx, &alias_path, X, &alias);
    assert!(
        hint.contains(&format!(
            "re-run from '{}', which has '{X}' checked out",
            wt.display()
        )),
        "the hint names the caller worktree on <X>: {hint}"
    );
    assert!(
        hint.contains(&format!("-c repo_path='{}'", fx.caller.display())),
        "the hint names the main clone for a new branch: {hint}"
    );
    assert!(
        hint.contains(&format!("git push origin 'HEAD:refs/heads/{X}'")),
        "unpushed alias commits are published to origin/<X>: {hint}"
    );
    assert!(!hint.contains("existing_branch"), "{hint}");
    fx.assert_no_alias_on_origin("after the alias stop");

    // Follow the hint: publish the alias's commit to origin/<X>, re-run from
    // the caller. The planner reuses the alias; the caller stays as it was.
    fx.git(
        &alias_path,
        &["push", "-q", "origin", &format!("HEAD:refs/heads/{X}")],
    );
    let caller_head = fx.head(&wt);
    let again = step4_from(&fx, &wt, Some("1"), &[], None);
    assert_eq!(again.code, Some(0), "{}", again.describe());
    let json = again.json();
    assert_eq!(json["branch_name"], X, "{}", again.describe());
    assert_eq!(json["local_branch"], alias.as_str(), "{}", again.describe());
    assert_eq!(
        json["worktree_path"],
        alias_path.to_str().unwrap(),
        "{}",
        again.describe()
    );
    assert_eq!(
        fx.head(&wt),
        caller_head,
        "the caller worktree is untouched"
    );
    fx.assert_no_alias_on_origin("after following the hint");
}

/// An alias of a branch checked out in the main clone (#1762's own shape):
/// the hint names the main clone with `existing_branch=<B>`. That re-run is
/// Gate C (the main clone is the run's caller), so it goes through the
/// planner to an alias and never reuses the main clone in place.
#[test]
fn run_from_alias_of_main_clone_branch_points_at_the_main_clone() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let first = fx.step4(BRANCH, &[]);
    assert_eq!(
        first.code,
        Some(0),
        "setup: alias run\n{}",
        first.describe()
    );
    let alias = alias(1);
    let alias_path = fx.caller.join("worktrees").join(&alias);
    assert_eq!(
        first.json()["local_branch"],
        alias.as_str(),
        "{}",
        first.describe()
    );

    let hint = assert_alias_stop(&fx, &alias_path, BRANCH, &alias);
    let follow = format!(
        "re-run from the main clone with -c repo_path='{}' -c existing_branch='{BRANCH}'",
        fx.caller.display()
    );
    assert!(hint.contains(&follow), "expected `{follow}` in: {hint}");

    let main_head = fx.head(&fx.caller);
    let again = step4_from(
        &fx,
        &fx.caller,
        Some("1"),
        &[("EXISTING_BRANCH", BRANCH)],
        None,
    );
    assert_eq!(again.code, Some(0), "{}", again.describe());
    let json = again.json();
    assert_eq!(json["local_branch"], alias.as_str(), "{}", again.describe());
    assert_ne!(
        json["worktree_path"],
        fx.caller.to_str().unwrap(),
        "never the main clone in place\n{}",
        again.describe()
    );
    assert_eq!(
        fx.head(&fx.caller),
        main_head,
        "the main clone is untouched"
    );
    fx.assert_no_alias_on_origin("after following the hint");
}

/// An alias-shaped branch whose upstream configuration was removed, with
/// `<B>` checked out nowhere, still stops; the hint says to check `<B>` out.
/// The stop applies to the name, so it cannot be bypassed by losing config.
#[test]
fn alias_shaped_branch_without_upstream_still_stops() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let alias = alias(1);
    let alias_path = fx.alias_worktree(&alias, BRANCH);
    fx.git(&alias_path, &["branch", "--unset-upstream"]);
    fx.git(&fx.caller, &["switch", "-q", "main"]);

    let hint = assert_alias_stop(&fx, &alias_path, BRANCH, &alias);
    assert!(
        hint.contains(&format!(
            "check '{BRANCH}' out in a linked worktree and re-run from there"
        )),
        "{hint}"
    );
}

/// A nested run started in a parent's alias task worktree keeps the #1134
/// behaviour: a fresh derived branch, no alias stop.
#[test]
fn nested_run_from_alias_worktree_starts_a_fresh_branch() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let alias = alias(1);
    let alias_path = fx.alias_worktree(&alias, BRANCH);

    let run = step4_from(&fx, &alias_path, Some("2"), &[], None);

    assert_eq!(run.code, Some(0), "{}", run.describe());
    let branch = run.json()["branch_name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(branch.starts_with("feat/issue-1969-"), "{}", run.describe());
    assert!(
        !run.stderr.contains("#1762 alias worktree"),
        "no alias stop for a nested run\n{}",
        run.describe()
    );
    assert!(
        run.stderr_line_with(&["INFO: nested run (depth 2)", &alias])
            .is_some(),
        "{}",
        run.describe()
    );
}

/// Names that only look like aliases (`--wt-0`, `--wt-01`, `--wt-x`) are
/// ordinary caller branches and are continued.
#[test]
fn near_alias_names_are_continued() {
    for name in ["feat/x--wt-0", "feat/x--wt-01", "feat/x--wt-x"] {
        let fx = Fx::new();
        let wt = linked_on_new_branch(&fx, name);
        let run = fx.tool(
            DETECT,
            &fx.root,
            &[wt.to_str().unwrap()],
            &[("AMPLIHACK_SESSION_DEPTH", "1")],
        );
        assert_eq!(run.code, Some(0), "{name}\n{}", run.describe());
        assert_eq!(
            run.stdout,
            format!("{name}\n"),
            "{name}\n{}",
            run.describe()
        );
    }
}

/// A default branch never gets a #1762 alias, so `<default>--wt-<n>` is an
/// ordinary branch. Step 4 must agree with `workflow_remote_branch.sh`, which
/// maps such a name to itself: it continues the branch, and no line names the
/// default branch as a push target or as `existing_branch` (crusty round 3:
/// the alias stop used to print `git push origin 'HEAD:refs/heads/main'`).
fn assert_default_prefix_is_not_an_alias(fx: &Fx, name: &str, default: &str, why: &str) {
    let wt = linked_on_new_branch(fx, name);
    // The real #1762 shape: the alias-looking branch tracks origin/<default>.
    fx.git(&wt, &["config", &format!("branch.{name}.remote"), "origin"]);
    fx.git(
        &wt,
        &[
            "config",
            &format!("branch.{name}.merge"),
            &format!("refs/heads/{default}"),
        ],
    );

    let run = fx.tool(
        DETECT,
        &fx.root,
        &[wt.to_str().unwrap()],
        &[("AMPLIHACK_SESSION_DEPTH", "1")],
    );
    assert_eq!(run.code, Some(0), "{why}: continued\n{}", run.describe());
    assert_eq!(run.stdout, format!("{name}\n"), "{why}\n{}", run.describe());
    assert!(continues(&run.stderr), "{why}\n{}", run.describe());
    assert!(
        !run.stderr.contains("#1762 alias worktree"),
        "{why}: no alias stop\n{}",
        run.describe()
    );
    for bad in [
        format!("refs/heads/{default}'"),
        format!("refs/heads/{default}\""),
        format!("existing_branch='{default}'"),
        format!("publishes to origin/{default} "),
    ] {
        assert!(
            !run.stderr.contains(&bad),
            "{why}: names the default branch ({bad})\n{}",
            run.describe()
        );
    }

    let resolved = fx.tool(RESOLVER, &fx.root, &[wt.to_str().unwrap()], &[]);
    assert_eq!(resolved.code, Some(0), "{why}\n{}", resolved.describe());
    assert_eq!(
        resolved.stdout,
        format!("{name}\n"),
        "{why}: step 15's resolver agrees it is not an alias\n{}",
        resolved.describe()
    );
}

#[test]
fn main_alias_shape_is_not_an_alias() {
    let fx = Fx::new();
    assert_default_prefix_is_not_an_alias(&fx, "main--wt-1", "main", "main--wt-1");
}

#[test]
fn master_alias_shape_is_not_an_alias() {
    let fx = Fx::new();
    assert_default_prefix_is_not_an_alias(&fx, "master--wt-1", "master", "master--wt-1");
}

/// `develop` is a default branch only while the local origin/HEAD is unknown.
#[test]
fn develop_alias_shape_without_origin_head_is_not_an_alias() {
    let fx = Fx::new();
    fx.git(&fx.caller, &["remote", "set-head", "origin", "--delete"]);
    assert_default_prefix_is_not_an_alias(
        &fx,
        "develop--wt-1",
        "develop",
        "develop--wt-1, origin/HEAD unset",
    );
}

/// The target of the local origin/HEAD is a default branch whatever its name.
#[test]
fn origin_head_target_alias_shape_is_not_an_alias() {
    let fx = Fx::new();
    fx.git(&fx.caller, &["push", "-q", "origin", "main:trunk"]);
    fx.git(&fx.caller, &["remote", "set-head", "origin", "trunk"]);
    assert_default_prefix_is_not_an_alias(
        &fx,
        "trunk--wt-1",
        "trunk",
        "trunk--wt-1, origin/HEAD -> trunk",
    );
}

/// The contrast: while origin/HEAD resolves to main, `develop` is an ordinary
/// branch, so `develop--wt-1` is its alias and stops (no hint to push main).
#[test]
fn develop_alias_with_origin_head_set_still_stops() {
    let fx = Fx::new();
    let name = "develop--wt-1";
    let wt = linked_on_new_branch(&fx, name);
    let run = fx.tool(
        DETECT,
        &fx.root,
        &[wt.to_str().unwrap()],
        &[("AMPLIHACK_SESSION_DEPTH", "1")],
    );
    assert_eq!(run.code, Some(3), "{}", run.describe());
    assert!(
        run.stderr_line_with(&["ERROR: repo_path is a #1762 alias worktree", "'develop'"])
            .is_some(),
        "{}",
        run.describe()
    );
    assert!(
        !run.stderr.contains("refs/heads/main"),
        "{}",
        run.describe()
    );
}

/// The prefix check fails closed: a default-branch helper that cannot decide
/// about `<P>` is an error (exit 2), never "not a default branch".
#[test]
fn alias_prefix_default_check_failure_is_an_error() {
    let fx = Fx::new();
    let name = "feat/x--wt-1";
    let wt = linked_on_new_branch(&fx, name);
    let home = fx.bundle_with(
        DEFAULT_RULE,
        "#!/usr/bin/env bash\n[ \"$2\" = feat/x ] && exit 9\nexit 1\n",
    );
    let mut command = fx.bash(&fx.root);
    command
        .arg(home.join("amplifier-bundle/tools").join(DETECT))
        .arg(&wt)
        .env("AMPLIHACK_SESSION_DEPTH", "1");
    let run = run(&mut command);
    assert_eq!(run.code, Some(2), "{}", run.describe());
    assert!(run.stdout.is_empty(), "{}", run.describe());
    assert!(
        run.stderr_line_with(&["ERROR: cannot decide whether 'feat/x'", "exit 9"])
            .is_some(),
        "{}",
        run.describe()
    );
    assert!(
        !run.stderr.contains("#1762 alias worktree"),
        "{}",
        run.describe()
    );
}
