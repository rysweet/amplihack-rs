//! Finalize with a pull request into `dev` while the checkout's origin/HEAD is
//! `main` (#2389, #2520).
//!
//! When `base_branch` is set, every finalize check uses it. When it is empty,
//! the helpers take the base from the pull request this run opened: first
//! the `base_branch` field of step-16's publish result, then the pull
//! request's own `baseRefName` (one `gh pr view`). A base worked out that way
//! still has to name a remote-tracking branch `origin/<base>` that is not in
//! `forbidden_base_branches`. Once the run has recorded a pull request,
//! origin/HEAD is never used. See "Finding the base when base_branch is
//! empty" in docs/reference/workflow-delivery-inputs.md.
//!
//! Each run sets every RECIPE_VAR_* and WORKFLOW_* variable the helpers read,
//! empty where a test does not use it, so no value leaks in from the
//! environment the tests run in (#2471). gh is a fake; nothing reaches
//! GitHub, and bash runs under `isolated()` with a temporary HOME.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Output;

use serde_json::{Value as Json, json};

use super::{Repo, bash, describe, git, step_command, text, workspace_root};

/// Fake gh for finalize. Pull request 7 is `feat/x` into `$PR_BASE`, a draft
/// at `$HEAD_SHA`. Every call is logged. With `GH_BASE_VIEW_FAIL` set, a
/// `gh pr view` that asks for `baseRefName` alone fails as a rate limit
/// would; the scoped lookups, which ask for more fields, still answer.
/// Anything unexpected fails.
const FAKE_GH_FINALIZE: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
fields=""
prev=""
for a in "$@"; do
  [ "$prev" = "--json" ] && fields="$a"
  case "$a" in --json=*) fields="${a#--json=}" ;; esac
  prev="$a"
done
pr() {
  printf '{"number":7,"title":"Deliver to dev","body":"","state":"OPEN","createdAt":"2026-11-11T00:00:00Z","mergedAt":null,"url":"https://github.com/owner/repo/pull/7","headRefName":"feat/x","baseRefName":"%s","headRefOid":"%s","headRepositoryOwner":{"login":"owner"},"headRepository":{"name":"repo"},"isCrossRepository":false,"statusCheckRollup":[],"isDraft":true,"mergeable":"MERGEABLE","reviews":[]}' "$PR_BASE" "$HEAD_SHA"
}
case "$1 $2" in
  "auth status") exit 0 ;;
  "pr view")
    if [ "$fields" = "baseRefName" ] && [ -n "${GH_BASE_VIEW_FAIL:-}" ]; then
      echo "fake gh: API rate limit exceeded" >&2
      exit 1
    fi
    pr; echo ;;
  "pr list") printf '['; pr; echo ']' ;;
  "pr ready") echo "marked ready: $3" ;;
  "pr comment") echo "$3#issuecomment-1" ;;
  *) echo "fake gh: unexpected: $*" >&2; exit 7 ;;
esac
"#;

const PR_URL: &str = "https://github.com/owner/repo/pull/7";

/// A publish result as step-16 writes it after #2520: with the base it gave
/// the pull request.
fn published_on(base: &str) -> Json {
    json!({ "pr_url": PR_URL, "pr_number": "7", "state": "FOLLOWUP_CREATED", "base_branch": base })
}

/// A publish result from before #2520: no `base_branch` field.
fn published_without_base() -> Json {
    json!({ "pr_url": PR_URL, "pr_number": "7", "state": "FOLLOWUP_CREATED" })
}

/// How many `gh pr view` calls asked for the pull request's base alone: the
/// call the shared rule makes when the publish result records no base.
fn base_views(log: &str) -> usize {
    log.lines()
        .filter(|l| l.starts_with("pr view"))
        .filter(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            words.contains(&"--json=baseRefName")
                || words
                    .windows(2)
                    .any(|w| w[0] == "--json" && w[1] == "baseRefName")
        })
        .count()
}

fn marked_ready(log: &str) -> bool {
    log.lines().any(|l| l.starts_with("pr ready"))
}

/// `Repo` with `feat/x` checked out: one commit on top of `dev`, while
/// `origin/HEAD` is `main`. The origin URL is github.com's, as finalize needs.
pub(super) struct FinalizeFixture {
    repo: Repo,
    bin: PathBuf,
    log: PathBuf,
}

impl FinalizeFixture {
    pub(super) fn new() -> Self {
        let repo = Repo::new();
        let path = repo.path();
        git(&path, &["checkout", "-b", "feat/x", "origin/dev"]);
        fs::write(path.join("feature.txt"), "feature\n").unwrap();
        git(&path, &["add", "feature.txt"]);
        git(&path, &["commit", "-m", "feature"]);
        git(&path, &["push", "origin", "feat/x"]);
        git(
            &path,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        );
        assert_eq!(
            git(
                &path,
                &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]
            ),
            "origin/main"
        );
        let bin = repo.root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        fs::write(&gh, FAKE_GH_FINALIZE).unwrap();
        fs::set_permissions(
            &gh,
            <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .unwrap();
        let log = repo.root.path().join("gh.log");
        FinalizeFixture { repo, bin, log }
    }

    pub(super) fn worktree(&self) -> PathBuf {
        fs::canonicalize(self.repo.path()).unwrap()
    }

    /// Every variable the finalize helpers and the probe read, set on
    /// purpose. The helper paths point at this checkout's tools, so no copy
    /// installed elsewhere is used.
    fn base_env(&self) -> BTreeMap<String, String> {
        let worktree = self.worktree();
        let wt = worktree.to_str().unwrap().to_owned();
        let tools = workspace_root().join("amplifier-bundle/tools");
        let tool = |name: &str| tools.join(name).to_str().unwrap().to_owned();
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let setup = json!({ "worktree_path": worktree, "branch_name": "feat/x" }).to_string();
        let mut env = BTreeMap::new();
        for (k, v) in [
            ("PATH", path),
            ("GH_LOG", self.log.to_str().unwrap().to_owned()),
            ("HEAD_SHA", self.repo.rev("HEAD")),
            ("PR_BASE", "dev".to_owned()),
            ("GH_BASE_VIEW_FAIL", String::new()),
            (
                "AMPLIHACK_HOME",
                workspace_root().to_str().unwrap().to_owned(),
            ),
            ("REPO_PATH", wt.clone()),
            ("REMOTE_HOST_TYPE", "github".to_owned()),
            ("RECIPE_VAR_repo_path", wt),
            ("RECIPE_VAR_worktree_setup", setup),
            ("RECIPE_VAR_worktree_setup__worktree_path", String::new()),
            ("RECIPE_VAR_base_branch", String::new()),
            ("RECIPE_VAR_forbidden_base_branches", String::new()),
            ("RECIPE_VAR_strict_delivery", String::new()),
            ("RECIPE_VAR_pr_url", String::new()),
            ("RECIPE_VAR_pr_number", String::new()),
            ("RECIPE_VAR_pr_publish_result", String::new()),
            ("RECIPE_VAR_issue_number", String::new()),
            ("RECIPE_VAR_terminal_fail_mode", "report".to_owned()),
            ("RECIPE_VAR_terminal_gate_mode", "probe".to_owned()),
            ("RECIPE_VAR_branch_name", String::new()),
            ("RECIPE_VAR_base_ref", String::new()),
            ("RECIPE_VAR_goal_already_met", "false".to_owned()),
            ("RECIPE_VAR_accept_pr_head_ahead", "false".to_owned()),
            ("RECIPE_VAR_allow_no_op", "false".to_owned()),
            ("RECIPE_VAR_terminal_adjudication_narrative", String::new()),
            ("WORKFLOW_STARTED_AT", String::new()),
            ("RECIPE_STARTED_AT", String::new()),
            ("TASK_STARTED_AT", String::new()),
            ("EXPECTED_PR_TITLE_PREFIX", String::new()),
            ("PR_EXPECTED_TITLE_PREFIX", String::new()),
            ("WORKFLOW_PR_SCOPE_HELPER", tool("workflow_pr_scope.sh")),
            ("WORKFLOW_GH_RETRY_HELPER", tool("workflow_gh_retry.sh")),
            (
                "WORKFLOW_GITHUB_REPO_HELPER",
                tool("workflow_github_repo.sh"),
            ),
            (
                "WORKFLOW_DELIVERY_INPUTS_HELPER",
                tool("workflow_delivery_inputs.sh"),
            ),
            (
                "WORKFLOW_TERMINAL_EVIDENCE_HELPER",
                tool("workflow_terminal_evidence.sh"),
            ),
            (
                "WORKFLOW_RUNTIME_ARTIFACT_HELPER",
                tool("workflow_runtime_artifacts.sh"),
            ),
        ] {
            env.insert(k.to_owned(), v);
        }
        env
    }

    /// Runs `step` of `recipe` as the runner would, with the base variables
    /// replaced by `vars`.
    pub(super) fn run_with(
        &self,
        recipe: &str,
        step: &str,
        vars: &[(&str, &str)],
    ) -> (Output, String) {
        let _ = fs::remove_file(&self.log);
        let mut env = self.base_env();
        for (k, v) in vars {
            assert!(
                env.contains_key(*k),
                "{k} is not a variable the fixture sets"
            );
            env.insert((*k).to_owned(), (*v).to_owned());
        }
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let out = bash(&step_command(recipe, step), &self.worktree(), &pairs);
        (out, fs::read_to_string(&self.log).unwrap_or_default())
    }

    /// `run_with` for a run whose base_branch is `base_branch`, with the
    /// pull request recorded in `pr_url` and a publish result from before
    /// #2520.
    pub(super) fn run(&self, recipe: &str, step: &str, base_branch: &str) -> (Output, String) {
        let publish = published_without_base().to_string();
        self.run_with(
            recipe,
            step,
            &[
                ("RECIPE_VAR_base_branch", base_branch),
                ("RECIPE_VAR_pr_url", PR_URL),
                ("RECIPE_VAR_pr_number", "7"),
                ("RECIPE_VAR_pr_publish_result", &publish),
            ],
        )
    }
}

fn probe_state(out: &Output) -> Json {
    let stdout = text(&out.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| panic!("probe JSON\n{}", describe(out)));
    serde_json::from_str(line).unwrap()
}

// ---------------------------------------------------------------------------
// step-21: the pull request is found and marked ready
// ---------------------------------------------------------------------------

#[test]
fn step21_marks_a_pull_request_into_base_branch_ready() {
    let fx = FinalizeFixture::new();
    let (out, log) = fx.run("workflow-finalize", "step-21-pr-ready", "dev");
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    assert!(
        log.lines().any(|l| l == format!("pr ready {PR_URL}")),
        "the draft is marked ready\n{log}"
    );

    // Control (rewritten for #2520): base_branch set to `main` on purpose is
    // never replaced by the pull request's base. The pull request into dev
    // does not match, and nothing is changed on GitHub.
    let (out, log) = fx.run("workflow-finalize", "step-21-pr-ready", "main");
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        text(&out.stderr).contains("no_scoped_pr"),
        "{}",
        describe(&out)
    );
    assert!(!marked_ready(&log), "{log}");
}

#[test]
fn step21_finds_pr_on_dev_when_base_branch_is_dev() {
    // Test A of #2520: the repository's default branch is main, the run's
    // base_branch is dev, and step-16 opened the pull request on dev.
    let fx = FinalizeFixture::new();
    let publish = published_on("dev").to_string();
    let (out, log) = fx.run_with(
        "workflow-finalize",
        "step-21-pr-ready",
        &[
            ("RECIPE_VAR_base_branch", "dev"),
            ("RECIPE_VAR_pr_url", PR_URL),
            ("RECIPE_VAR_pr_number", "7"),
            ("RECIPE_VAR_pr_publish_result", &publish),
        ],
    );
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    assert!(
        log.lines()
            .any(|l| l == format!("pr ready {PR_URL}") || l == "pr ready 7"),
        "the draft is marked ready\n{log}"
    );
    assert!(
        !text(&out.stderr).contains("no_scoped_pr"),
        "{}",
        describe(&out)
    );
    assert_eq!(
        base_views(&log),
        0,
        "a set base_branch is used as it is; the pull request's base is not read\n{log}"
    );
}

#[test]
fn step21_uses_recorded_publish_base_when_base_branch_empty() {
    // Test B of #2520: base_branch unset. The pull request comes only from
    // the publish result, which records the base step-16 gave it.
    let fx = FinalizeFixture::new();
    let publish = published_on("dev").to_string();
    let (out, log) = fx.run_with(
        "workflow-finalize",
        "step-21-pr-ready",
        &[("RECIPE_VAR_pr_publish_result", &publish)],
    );
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    assert!(
        log.lines()
            .any(|l| l == format!("pr ready {PR_URL}") || l == "pr ready 7"),
        "the draft into dev is marked ready although origin/HEAD is main\n{log}"
    );
    assert!(
        !text(&out.stderr).contains("no_scoped_pr"),
        "{}",
        describe(&out)
    );
    assert_eq!(
        base_views(&log),
        0,
        "the recorded base is enough; no extra GitHub call\n{log}"
    );
}

#[test]
fn step21_reads_pr_base_when_publish_result_has_none() {
    // Test B′ of #2520: base_branch unset and a publish result from before
    // the base was recorded. The base is read from the pull request, once.
    let fx = FinalizeFixture::new();
    let (out, log) = fx.run("workflow-finalize", "step-21-pr-ready", "");
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    assert!(
        log.lines().any(|l| l == format!("pr ready {PR_URL}")),
        "the draft into dev is marked ready although origin/HEAD is main\n{log}"
    );
    assert!(
        !text(&out.stderr).contains("no_scoped_pr"),
        "{}",
        describe(&out)
    );
    assert_eq!(
        base_views(&log),
        1,
        "one gh pr view for the pull request's base\n{log}"
    );
}

#[test]
fn step21_finds_pr_by_number_alone_when_base_branch_empty() {
    // The pull request recorded by number only, with no URL anywhere.
    let fx = FinalizeFixture::new();
    let publish = json!({ "pr_number": "7", "state": "FOLLOWUP_CREATED" }).to_string();
    let (out, log) = fx.run_with(
        "workflow-finalize",
        "step-21-pr-ready",
        &[
            ("RECIPE_VAR_pr_number", "7"),
            ("RECIPE_VAR_pr_publish_result", &publish),
        ],
    );
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    assert!(marked_ready(&log), "{log}");
    assert_eq!(base_views(&log), 1, "{log}");
}

#[test]
fn step21_refuses_a_recorded_base_that_differs_from_the_pull_request() {
    // Control (b): the publish result says main, the pull request is into
    // dev. The recorded base is checked against the live pull request.
    let fx = FinalizeFixture::new();
    let publish = published_on("main").to_string();
    let (out, log) = fx.run_with(
        "workflow-finalize",
        "step-21-pr-ready",
        &[
            ("RECIPE_VAR_pr_url", PR_URL),
            ("RECIPE_VAR_pr_publish_result", &publish),
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        text(&out.stderr).contains("no_scoped_pr"),
        "{}",
        describe(&out)
    );
    assert!(!marked_ready(&log), "{log}");
}

/// Runs step-21 with base_branch empty and the pull request into `pr_base`,
/// recorded in the publish result as `recorded`, and checks that it fails
/// closed before any lookup with "unable to resolve expected base branch".
fn assert_step21_cannot_prove_base(fx: &FinalizeFixture, pr_base: &str, extra: &[(&str, &str)]) {
    let publish = published_on(pr_base).to_string();
    let mut vars = vec![
        ("PR_BASE", pr_base),
        ("RECIPE_VAR_pr_url", PR_URL),
        ("RECIPE_VAR_pr_publish_result", publish.as_str()),
    ];
    vars.extend_from_slice(extra);
    let (out, log) = fx.run_with("workflow-finalize", "step-21-pr-ready", &vars);
    assert_ne!(
        out.status.code(),
        Some(0),
        "{pr_base}: {}\n{log}",
        describe(&out)
    );
    assert!(
        text(&out.stderr).contains("unable to resolve expected base branch"),
        "{pr_base}: {}\n{log}",
        describe(&out)
    );
    assert!(!marked_ready(&log), "{pr_base}: {log}");
}

#[test]
fn step21_rejects_derived_base_without_origin_ref() {
    // Control (c): the pull request's base has no remote-tracking ref here,
    // so it cannot be checked, and no other base is put in its place.
    let fx = FinalizeFixture::new();
    assert_step21_cannot_prove_base(&fx, "ghost", &[]);
}

#[test]
fn step21_rejects_derived_base_matching_forbidden_list() {
    // A base worked out from data still goes through forbidden_base_branches.
    let fx = FinalizeFixture::new();
    assert_step21_cannot_prove_base(
        &fx,
        "dev",
        &[("RECIPE_VAR_forbidden_base_branches", "release/* dev")],
    );
}

#[test]
fn step21_rejects_derived_base_resolving_only_as_local_ref() {
    // A local branch and a tag named origin/dev2 make `origin/dev2` resolve
    // for `git rev-parse`, but there is no refs/remotes/origin/dev2.
    let fx = FinalizeFixture::new();
    let wt = fx.worktree();
    git(&wt, &["branch", "origin/dev2", "HEAD"]);
    git(&wt, &["tag", "origin/dev3", "HEAD"]);
    for base in ["dev2", "dev3"] {
        assert_step21_cannot_prove_base(&fx, base, &[]);
    }
}

#[test]
fn step21_fails_closed_when_pr_view_fails() {
    // The pull request's base cannot be read. origin/HEAD is moved to the
    // pull request's real base, so a fall-back to origin/HEAD would pass:
    // it must not be used once the run has recorded a pull request.
    let fx = FinalizeFixture::new();
    git(&fx.worktree(), &["remote", "set-head", "origin", "dev"]);
    let publish = published_without_base().to_string();
    let (out, log) = fx.run_with(
        "workflow-finalize",
        "step-21-pr-ready",
        &[
            ("GH_BASE_VIEW_FAIL", "1"),
            ("RECIPE_VAR_pr_url", PR_URL),
            ("RECIPE_VAR_pr_publish_result", &publish),
        ],
    );
    assert_ne!(out.status.code(), Some(0), "{}\n{log}", describe(&out));
    assert!(
        text(&out.stderr).contains("unable to resolve expected base branch"),
        "{}\n{log}",
        describe(&out)
    );
    assert!(base_views(&log) >= 1, "the base was asked for\n{log}");
    assert!(!marked_ready(&log), "{log}");
}

#[test]
fn step21_rejects_malformed_pr_ref() {
    // A pull request number that is an option is never handed to gh.
    let fx = FinalizeFixture::new();
    for pr_number in ["--repo=x/y", "07", "7 8", "-1"] {
        let publish = json!({ "pr_number": pr_number, "state": "FOLLOWUP_CREATED" }).to_string();
        let (out, log) = fx.run_with(
            "workflow-finalize",
            "step-21-pr-ready",
            &[
                ("RECIPE_VAR_pr_number", pr_number),
                ("RECIPE_VAR_pr_publish_result", &publish),
            ],
        );
        assert_ne!(
            out.status.code(),
            Some(0),
            "{pr_number}: {}",
            describe(&out)
        );
        assert!(!log.contains("--repo=x/y"), "{pr_number}: {log}");
        assert_eq!(base_views(&log), 0, "{pr_number}: {log}");
        assert!(!marked_ready(&log), "{pr_number}: {log}");
    }
}

#[test]
fn step21_rejects_a_pr_url_for_another_repository() {
    let fx = FinalizeFixture::new();
    let other = "https://github.com/other/repo/pull/7";
    let publish = json!({ "pr_url": other, "state": "FOLLOWUP_CREATED" }).to_string();
    let (out, log) = fx.run_with(
        "workflow-finalize",
        "step-21-pr-ready",
        &[
            ("RECIPE_VAR_pr_url", other),
            ("RECIPE_VAR_pr_publish_result", &publish),
        ],
    );
    assert_ne!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(base_views(&log), 0, "{log}");
    assert!(!marked_ready(&log), "{log}");
}

#[test]
fn step21_without_a_recorded_pull_request_keeps_the_discovered_base() {
    // No pull request recorded anywhere: origin/HEAD (main) is still the
    // last fallback, and the branch's pull request into dev is not one this
    // run can claim. Nothing is changed on GitHub.
    let fx = FinalizeFixture::new();
    let (out, log) = fx.run_with("workflow-finalize", "step-21-pr-ready", &[]);
    assert_eq!(out.status.code(), Some(0), "{}\n{log}", describe(&out));
    assert!(
        text(&out.stderr).contains("no scoped PR matched"),
        "{}",
        describe(&out)
    );
    assert_eq!(base_views(&log), 0, "{log}");
    assert!(!marked_ready(&log), "{log}");
}

// ---------------------------------------------------------------------------
// step-22b, the terminal-state probe and the finalization evidence
// ---------------------------------------------------------------------------

#[test]
fn step22b_matches_the_pull_request_on_base_branch() {
    let fx = FinalizeFixture::new();
    for base in ["dev", ""] {
        let (out, log) = fx.run("workflow-finalize", "step-22b-final-status", base);
        assert!(out.status.success(), "{base:?}: {}\n{log}", describe(&out));
        let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
        assert!(
            !all.contains("scoped final PR match did not resolve"),
            "{base:?}: the scoped match finds the pull request into dev\n{}",
            describe(&out)
        );
        assert!(
            text(&out.stdout).contains("\"baseRefName\":\"dev\""),
            "{base:?}: the final status shows the matched pull request\n{}",
            describe(&out)
        );
    }

    // Control (rewritten for #2520): base_branch=main on purpose, the pull
    // request is into dev; the match misses.
    let (out, _) = fx.run("workflow-finalize", "step-22b-final-status", "main");
    assert!(
        text(&out.stderr).contains("scoped final PR match did not resolve: no_scoped_pr"),
        "control: an explicit base that differs from the pull request misses\n{}",
        describe(&out)
    );
}

#[test]
fn step22b_uses_recorded_publish_base_when_base_branch_empty() {
    let fx = FinalizeFixture::new();
    let publish = published_on("dev").to_string();
    let (out, log) = fx.run_with(
        "workflow-finalize",
        "step-22b-final-status",
        &[("RECIPE_VAR_pr_publish_result", &publish)],
    );
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        !all.contains("scoped final PR match did not resolve"),
        "{}",
        describe(&out)
    );
    assert_eq!(base_views(&log), 0, "{log}");
}

#[test]
fn terminal_state_probe_checks_the_pull_request_against_base_branch() {
    let fx = FinalizeFixture::new();
    for base in ["dev", ""] {
        let (out, log) = fx.run("workflow-terminal-state", "probe-terminal-state", base);
        assert!(out.status.success(), "{base:?}: {}\n{log}", describe(&out));
        let state = probe_state(&out);
        assert_eq!(
            state["terminal_state"], "FOLLOWUP_CREATED",
            "{base:?}: {state}"
        );
        assert_eq!(state["should_run_ci_wait"], "true", "{base:?}: {state}");
        assert_eq!(state["pr_url"], PR_URL, "{base:?}: {state}");
        assert_eq!(
            state["commits_ahead"], "1",
            "{base:?}: counted from origin/dev, not origin/main: {state}"
        );
    }

    // Control (rewritten for #2520): base_branch=main on purpose, the pull
    // request is into dev; the probe rejects it and never reports success.
    let (out, _) = fx.run("workflow-terminal-state", "probe-terminal-state", "main");
    let state = probe_state(&out);
    assert_eq!(state["terminal_state"], "FAILED_INVALID_INPUT", "{state}");
}

#[test]
fn terminal_state_probe_never_succeeds_when_the_base_cannot_be_read() {
    let fx = FinalizeFixture::new();
    let publish = published_without_base().to_string();
    let (out, log) = fx.run_with(
        "workflow-terminal-state",
        "probe-terminal-state",
        &[
            ("GH_BASE_VIEW_FAIL", "1"),
            ("RECIPE_VAR_pr_url", PR_URL),
            ("RECIPE_VAR_pr_publish_result", &publish),
        ],
    );
    let state = probe_state(&out);
    assert_ne!(
        state["terminal_state"], "FOLLOWUP_CREATED",
        "{state}\n{log}"
    );
}

#[test]
fn finalization_evidence_measures_the_diff_from_base_branch() {
    let fx = FinalizeFixture::new();
    for (base, expected_ref, ahead) in [
        ("dev", "origin/dev", "1"),
        ("", "origin/dev", "1"),
        ("main", "origin/main", "2"),
    ] {
        let (out, log) = fx.run("workflow-finalize", "collect-finalization-evidence", base);
        assert!(out.status.success(), "{base:?}: {}\n{log}", describe(&out));
        let evidence: Json = serde_json::from_str(&text(&out.stdout))
            .unwrap_or_else(|e| panic!("{base:?}: evidence JSON: {e}\n{}", describe(&out)));
        assert_eq!(
            evidence["git"]["base_ref"], expected_ref,
            "{base:?}: {evidence}"
        );
        assert_eq!(
            evidence["git"]["commits_ahead"], ahead,
            "{base:?}: {evidence}"
        );
    }
}

#[test]
fn finalize_helpers_refuse_a_malformed_base() {
    let fx = FinalizeFixture::new();
    for (helper, mode) in [
        ("workflow_pr_ready.sh", ""),
        ("workflow_final_status.sh", ""),
        ("workflow_agentic_finalization.sh", "collect "),
    ] {
        let script = format!("bash \"$TOOLS/{helper}\" {mode}--base 'a..b'");
        let tools = workspace_root().join("amplifier-bundle/tools");
        let out = bash(
            &script,
            &fx.worktree(),
            &[("TOOLS", tools.to_str().unwrap())],
        );
        assert_eq!(out.status.code(), Some(2), "{helper}: {}", describe(&out));
        assert!(
            text(&out.stderr).contains("is not a valid branch name"),
            "{helper}"
        );
    }
}
