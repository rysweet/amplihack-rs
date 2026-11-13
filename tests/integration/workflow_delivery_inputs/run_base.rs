//! `delivery_run_base` in workflow_delivery_inputs.sh, the one rule that works
//! out which base a run's pull request has (#2520), and the recipes that
//! pass `base_branch` down to default-workflow.
//!
//!   delivery_run_base EXPLICIT RECORDED PR_REF REPO BRANCH FORBIDDEN
//!
//! 1. EXPLICIT (the run's base_branch), when set, is the base.
//! 2. Else RECORDED (step-16's publish result `base_branch`).
//! 3. Else the `baseRefName` of PR_REF, read with one `gh pr view`. PR_REF
//!    must be a pull request number or a github.com pull request URL in REPO.
//! 4. A base from 2 or 3 must pass `delivery_base_ref`: a plain branch name,
//!    with a remote-tracking ref origin/<base>, not in FORBIDDEN and not
//!    BRANCH itself.
//! 5. Only with no RECORDED and no PR_REF: origin/HEAD, then origin/main,
//!    origin/master, origin/develop.
//!
//! It prints the branch name, without `origin/`. See
//! docs/reference/workflow-delivery-inputs.md.

use std::fs;
use std::path::PathBuf;
use std::process::Output;

use serde_yaml::Value;

use super::{Repo, bash, describe, git, helper, text, workspace_root};

/// Fake gh for `delivery_run_base`: `gh pr view` prints a pull request whose
/// base is `$PR_BASE`, or fails when `$GH_FAIL` is set. Every call is logged.
const FAKE_GH_VIEW: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
[ -z "${GH_FAIL:-}" ] || { echo "fake gh: HTTP 502" >&2; exit 1; }
case "$1 $2" in
  "pr view") printf '{"baseRefName":"%s","number":7}\n' "$PR_BASE" ;;
  *) echo "fake gh: unexpected: $*" >&2; exit 7 ;;
esac
"#;

struct Unit {
    repo: Repo,
    bin: PathBuf,
    log: PathBuf,
}

impl Unit {
    fn new() -> Self {
        let repo = Repo::new();
        let path = repo.path();
        git(&path, &["checkout", "-b", "feat/x", "origin/dev"]);
        let bin = repo.root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        fs::write(&gh, FAKE_GH_VIEW).unwrap();
        fs::set_permissions(
            &gh,
            <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .unwrap();
        let log = repo.root.path().join("gh.log");
        Unit { repo, bin, log }
    }

    /// Sources the helper and runs `delivery_run_base` with `args` (already
    /// shell-quoted), the fake gh answering with base `pr_base`.
    fn call(&self, args: &str, pr_base: &str, gh_fail: bool) -> (Output, String) {
        let _ = fs::remove_file(&self.log);
        let script = format!(". \"$HELPER\"\ndelivery_run_base {args}\n");
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = bash(
            &script,
            &self.repo.path(),
            &[
                ("HELPER", helper().to_str().unwrap()),
                ("PATH", &path),
                ("GH_LOG", self.log.to_str().unwrap()),
                ("PR_BASE", pr_base),
                ("GH_FAIL", if gh_fail { "1" } else { "" }),
            ],
        );
        (out, fs::read_to_string(&self.log).unwrap_or_default())
    }
}

const URL: &str = "https://github.com/owner/repo/pull/7";

#[test]
fn run_base_prefers_explicit_then_recorded_then_the_pull_request() {
    let u = Unit::new();
    for (args, expected, gh_calls) in [
        // 1. An explicit base wins over everything, and gh is never asked.
        ("dev main 7 owner/repo feat/x ''", "dev", 0),
        ("main dev 7 owner/repo feat/x ''", "main", 0),
        // 2. The recorded base, with no gh call.
        ("'' dev 7 owner/repo feat/x ''", "dev", 0),
        // 3. The pull request's own base, read once, by number or URL.
        ("'' '' 7 owner/repo feat/x ''", "dev", 1),
        (&format!("'' '' {URL} owner/repo feat/x ''"), "dev", 1),
        (&format!("'' '' {URL}/ owner/repo feat/x ''"), "dev", 1),
    ] {
        let (out, log) = u.call(args, "dev", false);
        assert!(out.status.success(), "{args}: {}\n{log}", describe(&out));
        assert_eq!(text(&out.stdout), format!("{expected}\n"), "{args}");
        assert_eq!(log.lines().count(), gh_calls, "{args}: gh calls\n{log}");
        if gh_calls == 1 {
            assert!(log.starts_with("pr view"), "{args}: {log}");
            assert!(log.contains("baseRefName"), "{args}: {log}");
        }
    }
}

#[test]
fn run_base_without_a_recorded_pull_request_falls_back_to_discovery() {
    // 5. No pull request recorded: the old discovery, origin/HEAD first.
    let u = Unit::new();
    let (out, log) = u.call("'' '' '' owner/repo feat/x ''", "dev", false);
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    assert_eq!(text(&out.stdout), "main\n");
    assert!(log.is_empty(), "no gh call\n{log}");
}

#[test]
fn run_base_never_uses_origin_head_once_a_pull_request_is_recorded() {
    // gh fails; origin/HEAD is moved to dev, the pull request's real base, so
    // a fall-back to discovery would print it.
    let u = Unit::new();
    git(&u.repo.path(), &["remote", "set-head", "origin", "dev"]);
    let (out, log) = u.call("'' '' 7 owner/repo feat/x ''", "dev", true);
    assert_eq!(out.status.code(), Some(1), "{}\n{log}", describe(&out));
    assert_eq!(text(&out.stdout), "", "nothing printed on failure");
    assert_eq!(log.lines().count(), 1, "one call, no retry\n{log}");
}

#[test]
fn run_base_checks_a_derived_base_like_an_explicit_one() {
    let u = Unit::new();
    // A local branch and a tag named origin/<b>: `origin/<b>` resolves for
    // rev-parse, refs/remotes/origin/<b> does not.
    git(&u.repo.path(), &["branch", "origin/dev2", "HEAD"]);
    git(&u.repo.path(), &["tag", "origin/dev3", "HEAD"]);
    for (args, pr_base, reason) in [
        // Recorded base: no remote-tracking ref, forbidden, the branch itself,
        // a ref, a bad name, only a local ref of that name.
        (
            "'' ghost '' owner/repo feat/x ''",
            "dev",
            "has no remote-tracking ref origin/ghost",
        ),
        (
            "'' dev '' owner/repo feat/x 'release/* dev'",
            "dev",
            "forbidden_base_branches",
        ),
        (
            "'' feat/x '' owner/repo feat/x ''",
            "dev",
            "is the branch being delivered",
        ),
        (
            "'' refs/heads/dev '' owner/repo feat/x ''",
            "dev",
            "plain branch name",
        ),
        (
            "'' 'a..b' '' owner/repo feat/x ''",
            "dev",
            "is not a valid branch name",
        ),
        (
            "'' dev2 '' owner/repo feat/x ''",
            "dev",
            "has no remote-tracking ref origin/dev2",
        ),
        (
            "'' dev3 '' owner/repo feat/x ''",
            "dev",
            "has no remote-tracking ref origin/dev3",
        ),
        // The pull request's base, read from gh, gets the same checks.
        (
            "'' '' 7 owner/repo feat/x ''",
            "ghost",
            "has no remote-tracking ref origin/ghost",
        ),
        (
            "'' '' 7 owner/repo feat/x dev",
            "dev",
            "forbidden_base_branches",
        ),
        ("'' '' 7 owner/repo feat/x ''", "@", "plain branch name"),
        ("'' '' 7 owner/repo feat/x ''", "", ""),
    ] {
        let (out, log) = u.call(args, pr_base, false);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{args} (pr base {pr_base:?}): {}\n{log}",
            describe(&out)
        );
        assert_eq!(text(&out.stdout), "", "{args}: nothing printed");
        assert!(
            text(&out.stderr).contains(reason),
            "{args}: expected {reason:?}\n{}",
            describe(&out)
        );
    }
}

#[test]
fn run_base_checks_an_explicit_base_too() {
    let u = Unit::new();
    for (args, reason) in [
        (
            "'a..b' '' 7 owner/repo feat/x ''",
            "is not a valid branch name",
        ),
        (
            "refs/heads/dev '' 7 owner/repo feat/x ''",
            "plain branch name",
        ),
        ("@ '' 7 owner/repo feat/x ''", "plain branch name"),
    ] {
        let (out, log) = u.call(args, "dev", false);
        assert_eq!(out.status.code(), Some(1), "{args}: {}", describe(&out));
        assert!(
            text(&out.stderr).contains(reason),
            "{args}: {}",
            describe(&out)
        );
        assert!(log.is_empty(), "{args}: no gh call\n{log}");
    }
}

#[test]
fn run_base_refuses_a_pull_request_reference_it_cannot_trust() {
    let u = Unit::new();
    for pr_ref in [
        "--repo=x/y",
        "-7",
        "07",
        "0",
        "7x",
        "'7 8'",
        "https://github.com/other/repo/pull/7",
        "https://github.com/owner/repo/pull/7/files",
        "https://example.com/owner/repo/pull/7",
        "http://github.com/owner/repo/pull/7",
        "'https://github.com/owner/repo/pull/7;id'",
    ] {
        let (out, log) = u.call(
            &format!("'' '' {pr_ref} owner/repo feat/x ''"),
            "dev",
            false,
        );
        assert_eq!(out.status.code(), Some(1), "{pr_ref}: {}", describe(&out));
        assert_eq!(text(&out.stdout), "", "{pr_ref}");
        assert!(log.is_empty(), "{pr_ref}: gh is never called\n{log}");
    }
}

#[test]
fn run_base_treats_the_pull_request_base_as_data() {
    // A base name from GitHub that holds shell text is neither run nor
    // word-split; it fails the ref check and the message quotes it.
    let u = Unit::new();
    let marker = u.repo.root.path().join("pwned");
    let base = format!("x$(touch${{IFS}}{})", marker.display());
    let (out, log) = u.call("'' '' 7 owner/repo feat/x ''", &base, false);
    assert_eq!(out.status.code(), Some(1), "{}\n{log}", describe(&out));
    assert!(!marker.exists(), "the base name was run as a command");
}

#[test]
fn run_base_when_sourced_defines_functions_only() {
    let u = Unit::new();
    let (out, log) = u.call("dev '' '' owner/repo feat/x ''", "dev", false);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(log.is_empty());
    let out = bash(
        ". \"$HELPER\"; false; echo still-running; declare -F delivery_run_base",
        &u.repo.path(),
        &[("HELPER", helper().to_str().unwrap())],
    );
    assert_eq!(
        text(&out.stdout),
        "still-running\ndelivery_run_base\n",
        "{}",
        describe(&out)
    );
    assert_eq!(text(&out.stderr), "", "nothing printed when sourced");
}

// ---------------------------------------------------------------------------
// Recipes: base_branch is declared on the way down and passed in each call
// ---------------------------------------------------------------------------

fn recipe(name: &str) -> Value {
    let path = workspace_root().join(format!("amplifier-bundle/recipes/{name}.yaml"));
    serde_yaml::from_str(&fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|e| panic!("{name}.yaml does not parse: {e}"))
}

/// The recipes between the entry points and default-workflow (#2520), and
/// the ones under default-workflow that finalize uses (#2389).
const DECLARES_BASE_BRANCH: &[&str] = &[
    "default-workflow",
    "workflow-publish",
    "workflow-finalize",
    "workflow-terminal-state",
    "workflow-terminal-adjudication",
    "auto-drive-to-merge",
    "autodrive-build",
    "auto-drive-light",
    "quick-change",
    "smart-orchestrator",
    "smart-classify-route",
    "smart-execute-routing",
    "quality-loop",
    "oxidizer-workflow",
];

/// Sub-recipes that lead to default-workflow; a call to any of them passes
/// the caller's base_branch on.
const LEADS_TO_DEFAULT_WORKFLOW: &[&str] = &[
    "default-workflow",
    "autodrive-build",
    "auto-drive-to-merge",
    "smart-classify-route",
    "smart-execute-routing",
];

#[test]
fn every_finalize_recipe_declares_base_branch() {
    // The runner lays a parent's whole context over each sub-recipe's
    // defaults when it runs in the same process. Each recipe on the way
    // declares base_branch anyway: the empty default applies when it runs on
    // its own, and a phase run as a separate process sees the parent's value
    // only as an inherited RECIPE_VAR_base_branch, which the child's own
    // declaration replaces (#1561). See "How the inputs reach each phase" in
    // docs/reference/workflow-delivery-inputs.md.
    for name in DECLARES_BASE_BRANCH {
        assert_eq!(
            recipe(name)["context"]["base_branch"].as_str(),
            Some(""),
            "{name} must declare base_branch: \"\""
        );
    }
}

#[test]
fn every_caller_passes_base_branch_to_default_workflow() {
    // `-c base_branch=dev` on any entry point reaches default-workflow: each
    // call on the way passes "{{base_branch}}" in its context. The two #2389
    // clears are calls to workflow-worktree, which is not in this set, so
    // they stay as they are (covered by
    // workflow_worktree_callers_outside_default_workflow_clear_base_branch).
    let mut seen = Vec::new();
    for name in DECLARES_BASE_BRANCH {
        let doc = recipe(name);
        for step in doc["steps"].as_sequence().into_iter().flatten() {
            if step["type"].as_str() != Some("recipe") {
                continue;
            }
            let Some(target) = step["recipe"].as_str() else {
                continue;
            };
            if !LEADS_TO_DEFAULT_WORKFLOW.contains(&target) {
                continue;
            }
            let id = step["id"].as_str().unwrap_or("?");
            assert_eq!(
                step["context"]["base_branch"].as_str(),
                Some("{{base_branch}}"),
                "{name}/{id} calls {target} without base_branch: \"{{{{base_branch}}}}\""
            );
            seen.push(format!("{name}/{id}->{target}"));
        }
    }
    // Each entry point reaches default-workflow through a call checked above.
    for (caller, target, at_least) in [
        ("auto-drive-to-merge", "autodrive-build", 1),
        ("autodrive-build", "default-workflow", 1),
        ("auto-drive-light", "auto-drive-to-merge", 1),
        ("quick-change", "default-workflow", 1),
        ("smart-orchestrator", "smart-classify-route", 1),
        ("smart-orchestrator", "smart-execute-routing", 1),
        ("smart-execute-routing", "default-workflow", 3),
        ("quality-loop", "default-workflow", 1),
        ("oxidizer-workflow", "default-workflow", 13),
    ] {
        let found = seen
            .iter()
            .filter(|s| s.starts_with(&format!("{caller}/")) && s.ends_with(&format!("->{target}")))
            .count();
        assert!(
            found >= at_least,
            "{caller} has {found} checked call(s) to {target}, expected {at_least}: {seen:?}"
        );
    }
}

#[test]
fn base_branch_appears_only_in_context_maps() {
    // "{{base_branch}}" is passed as a value, never pasted into shell text,
    // where a crafted name would run as a command.
    for name in DECLARES_BASE_BRANCH {
        let doc = recipe(name);
        for step in doc["steps"].as_sequence().into_iter().flatten() {
            let id = step["id"].as_str().unwrap_or("?");
            for key in ["command", "prompt"] {
                if let Some(body) = step[key].as_str() {
                    assert!(
                        !body.contains("{{base_branch}}"),
                        "{name}/{id} puts {{{{base_branch}}}} in its {key}"
                    );
                }
            }
        }
    }
}
