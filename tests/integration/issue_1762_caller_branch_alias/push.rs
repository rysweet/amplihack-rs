//! The push steps publish an alias worktree's commits to origin/<B> and never
//! push the alias name (#1762 R10, D9, S1, S2, S4). Each test commits one new
//! file in an alias worktree made as step 4's create pass makes it, runs the
//! step exactly as the recipe holds it, and reads origin directly.
//!
//! `push.default=current` is the setting under which a plain `git push` from
//! an alias would create a branch named after the alias, so the tests that
//! guard against that run under it.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::json;

use super::fixture::*;

/// An alias worktree with one new, uncommitted file, under `push_default`.
fn alias_with_work(fx: &Fx, push_default: Option<&str>) -> PathBuf {
    fx.on_pushed_branch();
    if let Some(mode) = push_default {
        fx.git(&fx.caller, &["config", "push.default", mode]);
    }
    let wt = fx.alias_worktree(&alias(1), BRANCH);
    fx.write_in(&wt, "alias-work.txt", "work done in the alias worktree\n");
    wt
}

/// Run `recipe`'s step `id` in `wt` with worktree_setup naming `branch_name`.
fn run_step(fx: &Fx, recipe: &str, id: &str, wt: &Path, branch_name: &str) -> Run {
    run_step_in(fx, recipe, id, wt, branch_name, None)
}

/// [`run_step`], with `AMPLIHACK_HOME` set to `home` when one is given.
fn run_step_in(
    fx: &Fx,
    recipe: &str,
    id: &str,
    wt: &Path,
    branch_name: &str,
    home: Option<&Path>,
) -> Run {
    let setup = json!({
        "worktree_path": wt.to_str().unwrap(),
        "branch_name": branch_name,
        "local_branch": alias(1),
    })
    .to_string();
    let mut command = fx.bash(wt);
    command
        .arg("-c")
        .arg(step_text(recipe, id, "command"))
        .env("RECIPE_VAR_worktree_setup", setup)
        .env("REPO_PATH", &fx.caller)
        .env("TASK_DESCRIPTION", "fix the fixture branch for issue 1762")
        .env("ISSUE_NUMBER", "1762")
        .env("REMOTE_HOST_TYPE", "github");
    if let Some(home) = home {
        command.env("AMPLIHACK_HOME", home);
    }
    run(&mut command)
}

/// origin/<B> now holds the alias worktree's HEAD, no alias reached origin,
/// and the alias still tracks origin/<B>.
fn assert_published_to_remote_branch(fx: &Fx, wt: &Path, run: &Run, why: &str) {
    let head = fx.head(wt);
    assert_eq!(
        fx.origin_sha(BRANCH).as_deref(),
        Some(head.as_str()),
        "{why}: origin/<B> must receive the alias worktree's commit\n{}",
        run.describe()
    );
    fx.assert_no_alias_on_origin(why);
    assert_eq!(
        fx.config_get(&format!("branch.{}.merge", alias(1))),
        format!("refs/heads/{BRANCH}"),
        "{why}: the alias keeps origin/<B> as its upstream"
    );
}

/// The issue's first test case, second half: publish pushes to the original
/// remote branch.
#[test]
fn publish_pushes_alias_head_to_original_remote_branch() {
    for push_default in [None, Some("current")] {
        let why = format!("step 15, push.default={}", push_default.unwrap_or("unset"));
        let fx = Fx::new();
        let wt = alias_with_work(&fx, push_default);

        let run = run_step(&fx, "workflow-publish", "step-15-commit-push", &wt, BRANCH);

        assert_eq!(run.code, Some(0), "{why}\n{}", run.describe());
        let result = run.json();
        assert_eq!(result["pushed"], "true", "{why}\n{}", run.describe());
        assert_eq!(result["reason"], "pushed", "{why}\n{}", run.describe());
        assert_published_to_remote_branch(&fx, &wt, &run, &why);
    }
}

/// S4: from an alias worktree, step 15 pushes only to
/// worktree_setup.branch_name.
#[test]
fn step15_refuses_unbound_push_target() {
    let fx = Fx::new();
    let wt = alias_with_work(&fx, Some("current"));
    let before = fx.origin_refs();

    let run = run_step(
        &fx,
        "workflow-publish",
        "step-15-commit-push",
        &wt,
        "feat/some-other-branch",
    );

    assert_ne!(run.code, Some(0), "{}", run.describe());
    assert!(
        run.stderr_line_with(&["ERROR:", "refusing to push", "#1762"])
            .is_some(),
        "{}",
        run.describe()
    );
    assert_eq!(fx.origin_refs(), before, "nothing may reach origin");
}

#[test]
fn review_feedback_push_targets_remote_branch() {
    let fx = Fx::new();
    let wt = alias_with_work(&fx, Some("current"));

    let run = run_step(
        &fx,
        "workflow-pr-review",
        "step-18c-push-feedback-changes",
        &wt,
        BRANCH,
    );

    assert_eq!(run.code, Some(0), "{}", run.describe());
    assert_published_to_remote_branch(&fx, &wt, &run, "step 18c");
}

/// S2: with refs/remotes/origin/<B> missing, @{u} does not resolve and step
/// 18c takes its `push -u` fallback, which must still name <B>.
#[test]
fn push_fallback_keeps_alias_mapping() {
    let fx = Fx::new();
    let wt = alias_with_work(&fx, None);
    fx.git(
        &fx.caller,
        &["update-ref", "-d", &format!("refs/remotes/origin/{BRANCH}")],
    );

    let run = run_step(
        &fx,
        "workflow-pr-review",
        "step-18c-push-feedback-changes",
        &wt,
        BRANCH,
    );

    assert_eq!(run.code, Some(0), "{}", run.describe());
    assert_published_to_remote_branch(&fx, &wt, &run, "step 18c -u fallback");
}

/// A bundle without the resolver stops every push step before it pushes. A
/// fallback to the local name would push the alias: here push.default is
/// `current` and origin/<B> is missing locally, so even the `-u` fallbacks
/// would name the alias.
#[test]
fn missing_resolver_stops_push_steps() {
    for (recipe, id) in [
        ("workflow-publish", "step-15-commit-push"),
        ("workflow-pr-review", "step-18c-push-feedback-changes"),
        ("workflow-finalize", "step-20b-push-cleanup"),
    ] {
        let fx = Fx::new();
        let wt = alias_with_work(&fx, Some("current"));
        fx.git(
            &fx.caller,
            &["update-ref", "-d", &format!("refs/remotes/origin/{BRANCH}")],
        );
        let home = fx.bundle_without(RESOLVER);
        let before = fx.origin_refs();

        let run = run_step_in(&fx, recipe, id, &wt, BRANCH, Some(&home));

        assert_ne!(run.code, Some(0), "{id}\n{}", run.describe());
        assert!(
            run.stderr_line_with(&[RESOLVER]).is_some(),
            "{id}: stderr names the missing resolver\n{}",
            run.describe()
        );
        assert_eq!(fx.origin_refs(), before, "{id}: nothing may reach origin");
    }
}

#[test]
fn cleanup_push_targets_remote_branch() {
    let fx = Fx::new();
    let wt = alias_with_work(&fx, Some("current"));

    let run = run_step(
        &fx,
        "workflow-finalize",
        "step-20b-push-cleanup",
        &wt,
        BRANCH,
    );

    assert_eq!(run.code, Some(0), "{}", run.describe());
    assert_published_to_remote_branch(&fx, &wt, &run, "step 20b");
}

/// S1: step 16b's code block gets <B> from the resolver when it runs; no
/// branch name reaches it through `{{...}}`.
#[test]
fn step16b_prompt_does_not_template_branch_name() {
    let prompt = step_text("workflow-publish", "step-16b-outside-in-fix-loop", "prompt");
    let block = prompt
        .split("```bash")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("step 16b's prompt has a bash code block");
    assert!(
        !block.contains("{{"),
        "no template placeholder may reach the shell text:\n{block}"
    );
    assert!(
        block.contains("workflow_remote_branch.sh"),
        "the push target comes from the resolver:\n{block}"
    );
    let push = Regex::new(r#"git push [^\n]*origin "HEAD:\$"#).unwrap();
    assert!(
        block
            .lines()
            .any(|l| l.contains("git pull --rebase") && push.is_match(l)),
        "the fix loop pushes HEAD to the resolved remote branch:\n{block}"
    );
}
