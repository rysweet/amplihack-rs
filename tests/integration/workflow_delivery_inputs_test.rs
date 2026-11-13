//! `base_branch` and `strict_delivery` (docs/reference/workflow-delivery-inputs.md).
//!
//! Covers the shared helper `amplifier-bundle/tools/workflow_delivery_inputs.sh`,
//! the step-15/16 protected-branch guard, step-15's issue reference, step-06d
//! (goal already met) for an issue owned by another repository, step-04's base
//! for a new and for an existing branch, and finalize (the terminal-state
//! probe, step-21 and step-22b) accepting a pull request opened on
//! `base_branch`. It also covers the paths that do not deliver to
//! `base_branch`: quick-change and auto-drive-light clear it in their call to
//! workflow-worktree, and smart-orchestrator keeps a strict run off the
//! parallel launcher. Step-03 is covered in `issue_1881_local_default_tracking_test.rs`
//! and step-16 in `issue_2306_publish/recovery.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_yaml::Value;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn helper() -> PathBuf {
    workspace_root().join("amplifier-bundle/tools/workflow_delivery_inputs.sh")
}

fn step_command(recipe: &str, step_id: &str) -> String {
    let path = workspace_root().join(format!("amplifier-bundle/recipes/{recipe}.yaml"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let doc: Value = serde_yaml::from_str(&text).expect("recipe parses");
    doc["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(step_id))
        .and_then(|s| s["command"].as_str())
        .unwrap_or_else(|| panic!("{recipe}: no bash step {step_id}"))
        .to_owned()
}

fn bash(script: &str, cwd: &Path, env: &[(&str, &str)]) -> Output {
    let mut cmd = crate::isolated_amplihack::isolated("bash");
    cmd.arg("-c").arg(script).current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn bash")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn describe(out: &Output) -> String {
    format!(
        "status={}\nstdout={}\nstderr={}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    )
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command()
        .args(["-c", "user.email=test@test", "-c", "user.name=test"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", describe(&out));
    text(&out.stdout).trim().to_owned()
}

/// A checkout whose bare origin has `main` and a `dev` that is one commit
/// ahead of it, with remote-tracking refs for both. No network.
struct Repo {
    _origin: TempDir,
    root: TempDir,
}

impl Repo {
    fn new() -> Self {
        let origin = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let repo = root.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(origin.path(), &["init", "--bare", "-b", "main"]);
        git(&repo, &["init", "-b", "main"]);
        git(
            &repo,
            &["remote", "add", "origin", origin.path().to_str().unwrap()],
        );
        fs::write(repo.join("README.md"), "init\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "init"]);
        git(&repo, &["push", "-u", "origin", "HEAD:main"]);
        git(&repo, &["checkout", "-b", "dev"]);
        fs::write(repo.join("dev-only.txt"), "dev\n").unwrap();
        git(&repo, &["add", "dev-only.txt"]);
        git(&repo, &["commit", "-m", "dev work"]);
        git(&repo, &["push", "origin", "HEAD:dev"]);
        git(&repo, &["checkout", "main"]);
        git(&repo, &["branch", "-D", "dev"]);
        git(&repo, &["remote", "set-head", "origin", "main"]);
        Repo {
            _origin: origin,
            root,
        }
    }

    fn path(&self) -> PathBuf {
        self.root.path().join("repo")
    }

    fn rev(&self, name: &str) -> String {
        git(&self.path(), &["rev-parse", name])
    }
}

// ---------------------------------------------------------------------------
// workflow_delivery_inputs.sh
// ---------------------------------------------------------------------------

fn source_and_run(repo: &Repo, call: &str) -> Output {
    let script = format!("set -euo pipefail\n. \"$HELPER\"\n{call}\n");
    bash(
        &script,
        &repo.path(),
        &[("HELPER", helper().to_str().unwrap())],
    )
}

#[test]
fn delivery_base_ref_prints_the_remote_tracking_base() {
    let repo = Repo::new();
    // The control for the multi-line refusals below: a list over several
    // lines, with tabs, CRLF line ends and blank lines, that does not name
    // `dev` accepts it.
    for call in [
        "delivery_base_ref dev feat/x 'release/*'",
        "delivery_base_ref dev feat/x $'release/*\\nprod'",
        "delivery_base_ref dev feat/x $'  release/*\\r\\n\\tprod \\n\\n'",
        "delivery_base_ref dev feat/x ''",
    ] {
        let out = source_and_run(&repo, call);
        assert!(out.status.success(), "{call}: {}", describe(&out));
        assert_eq!(text(&out.stdout), "origin/dev\n", "{call}");
    }
}

#[test]
fn delivery_base_ref_refuses_without_substituting_another_base() {
    let repo = Repo::new();
    // A file that the forbidden glob `*` would expand to, were the list split
    // by an unquoted expansion: the pattern must still match `dev` itself.
    fs::write(repo.path().join("zz-not-a-branch"), "").unwrap();
    for (call, reason) in [
        (
            "delivery_base_ref 'a..b' '' ''",
            "is not a valid branch name",
        ),
        ("delivery_base_ref '-x' '' ''", "is not a valid branch name"),
        (
            "delivery_base_ref refs/heads/dev '' ''",
            "must be a plain branch name",
        ),
        ("delivery_base_ref @ '' ''", "must be a plain branch name"),
        (
            "delivery_base_ref dev dev ''",
            "is the branch being delivered",
        ),
        (
            "delivery_base_ref dev '' 'release/* dev'",
            "matches forbidden_base_branches pattern 'dev'",
        ),
        (
            "delivery_base_ref dev '' '*'",
            "matches forbidden_base_branches pattern '*'",
        ),
        // Every line of the list is enforced, not only the first: a YAML
        // block scalar, entries joined with newlines, CRLF line ends, tabs.
        (
            "delivery_base_ref dev '' $'release/*\\ndev'",
            "matches forbidden_base_branches pattern 'dev'",
        ),
        (
            "delivery_base_ref dev '' $'release/*\\r\\ndev\\r\\n'",
            "matches forbidden_base_branches pattern 'dev'",
        ),
        (
            "delivery_base_ref dev '' $'release/*\\tdev'",
            "matches forbidden_base_branches pattern 'dev'",
        ),
        (
            "IFS=,; delivery_base_ref dev '' 'release/* dev'",
            "matches forbidden_base_branches pattern 'dev'",
        ),
        // A list in another syntax is refused, never read as globs that match
        // almost nothing: even when no entry would match the base.
        (
            "delivery_base_ref dev '' '[\"dev\"]'",
            "entry '[\"dev\"]' is not a branch glob",
        ),
        (
            "delivery_base_ref dev '' '[\"release/*\",\"prod\"]'",
            "is not a branch glob",
        ),
        (
            "delivery_base_ref dev '' '[ \"release/*\", \"prod\" ]'",
            "entry '[' is not a branch glob",
        ),
        (
            "delivery_base_ref dev '' '[release/*, prod]'",
            "entry '[release/*,' is not a branch glob",
        ),
        (
            "delivery_base_ref dev '' 'release/*,dev'",
            "entry 'release/*,dev' is not a branch glob",
        ),
        (
            "delivery_base_ref dev '' $'release/*\\n\\'prod\\''",
            "entry ''prod'' is not a branch glob",
        ),
        (
            "delivery_base_ref nope '' ''",
            "has no remote-tracking ref origin/nope",
        ),
    ] {
        let out = source_and_run(&repo, call);
        assert_eq!(out.status.code(), Some(1), "{call}: {}", describe(&out));
        assert!(out.stdout.is_empty(), "{call}: no base is printed");
        assert!(
            text(&out.stderr).contains(reason),
            "{call}: expected {reason:?}\n{}",
            describe(&out)
        );
    }
}

#[test]
fn strict_issue_tag_qualifies_only_an_issue_in_another_repository() {
    let repo = Repo::new();
    for (call, expected) in [
        ("strict_issue_tag 42 owner/repo owner/repo", "#42"),
        ("strict_issue_tag 42 Owner/Repo owner/repo", "#42"),
        ("strict_issue_tag 42 plan/tasks owner/repo", "plan/tasks#42"),
    ] {
        let out = source_and_run(&repo, call);
        assert!(out.status.success(), "{call}: {}", describe(&out));
        assert_eq!(text(&out.stdout).trim(), expected, "{call}");
    }
}

#[test]
fn sourcing_the_helper_defines_functions_only() {
    let repo = Repo::new();
    let out = source_and_run(&repo, "true");
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        out.stdout.is_empty() && out.stderr.is_empty(),
        "{}",
        describe(&out)
    );
}

#[test]
fn worktree_mode_requires_existing_branch_under_strict_delivery() {
    let repo = Repo::new();
    let run = |args: &[&str]| {
        let mut cmd = crate::isolated_amplihack::isolated("bash");
        cmd.arg(helper()).args(args).current_dir(repo.path());
        cmd.output().unwrap()
    };
    let out = run(&["worktree", "", "true", "", ""]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(text(&out.stderr).contains("strict_delivery=true requires existing_branch"));
    let out = run(&["worktree", "", "false", "", ""]);
    assert!(
        out.status.success() && out.stdout.is_empty(),
        "{}",
        describe(&out)
    );
    let out = run(&["worktree", "dev", "true", "feat/x", ""]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(text(&out.stdout), "origin/dev\n");
}

// ---------------------------------------------------------------------------
// step-15 / step-16 protected-branch guard
// ---------------------------------------------------------------------------

fn guard_line(step_id: &str) -> String {
    let command = step_command("workflow-publish", step_id);
    let lines: Vec<&str> = command
        .lines()
        .filter(|l| {
            l.trim_start()
                .starts_with("if [ \"$_CURRENT_BRANCH\" = \"main\" ]")
        })
        .collect();
    assert_eq!(lines.len(), 1, "{step_id}: one protected-branch guard");
    lines[0].trim().to_owned()
}

#[test]
fn protected_branch_guard_refuses_main_master_and_base_branch_together() {
    let dir = TempDir::new().unwrap();
    for step in ["step-15-commit-push", "step-16-create-draft-pr"] {
        let guard = guard_line(step);
        for (base, current, refused) in [
            ("", "main", true),
            ("", "master", true),
            ("", "feat/x", false),
            ("dev", "main", true),
            ("dev", "master", true),
            ("dev", "dev", true),
            ("dev", "feat/x", false),
        ] {
            let script = format!("_CURRENT_BRANCH=\"$CUR\"\n{guard}\necho allowed\n");
            let out = bash(
                &script,
                dir.path(),
                &[("CUR", current), ("RECIPE_VAR_base_branch", base)],
            );
            assert_eq!(
                !out.status.success(),
                refused,
                "{step}: base={base:?} current={current:?}\n{}",
                describe(&out)
            );
            if refused {
                assert!(text(&out.stderr).contains("protected branch"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// step-06d: goal already met
// ---------------------------------------------------------------------------

/// Fake gh for step-06d. Without `--repo`, issue N is this repository's
/// issue N: CLOSED by a merged pull request. With `--repo github.com/plan/tasks`
/// it is the strict run's issue: OPEN. Prints what `--jq` would.
const FAKE_GH_06D: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
case "$*" in
  "issue view 2306 --repo github.com/plan/tasks --json state --jq .state") echo OPEN ;;
  "issue view 2306 --json state --jq .state") echo CLOSED ;;
  "issue view 2306 --json closedByPullRequestsReferences --jq "*) echo "2313 rysweet/amplihack-rs" ;;
  "pr view 2313 --repo rysweet/amplihack-rs --json mergedAt --jq .mergedAt") echo "2026-11-10T00:00:00Z" ;;
  *) echo "fake gh: unexpected: $*" >&2; exit 7 ;;
esac
"#;

fn run_06d(strict: bool) -> (Output, String) {
    let dir = TempDir::new().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, FAKE_GH_06D).unwrap();
    fs::set_permissions(
        &gh,
        <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();
    let log = dir.path().join("gh.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = bash(
        &step_command("workflow-design", "step-06d-goal-already-met-probe"),
        dir.path(),
        &[
            ("PATH", &path),
            ("GH_LOG", log.to_str().unwrap()),
            ("ISSUE_NUMBER", "2306"),
            (
                "RECIPE_VAR_strict_delivery",
                if strict { "true" } else { "false" },
            ),
            ("RECIPE_VAR_repository", "plan/tasks"),
        ],
    );
    (out, fs::read_to_string(log).unwrap_or_default())
}

#[test]
fn goal_probe_reads_a_strict_issue_in_its_own_repository() {
    // rysweet/amplihack-rs#2306 is closed by a merged PR; plan/tasks#2306 is
    // open. A strict run for plan/tasks#2306 must not skip implementation.
    let (out, log) = run_06d(true);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        text(&out.stdout).trim(),
        "false",
        "{}\n{log}",
        describe(&out)
    );
    assert_eq!(
        log.trim(),
        "issue view 2306 --repo github.com/plan/tasks --json state --jq .state",
        "only the owning repository's issue is read"
    );
}

#[test]
fn goal_probe_without_strict_delivery_still_reads_this_repository() {
    // Control: the same fixture, run as an ordinary caller, finds the local
    // closed issue, so the strict case above is not passing by accident.
    let (out, log) = run_06d(false);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        text(&out.stdout).trim(),
        "true",
        "{}\n{log}",
        describe(&out)
    );
    assert!(!log.contains("--repo github.com/plan/tasks"), "{log}");
}

// ---------------------------------------------------------------------------
// step-04: the base of a new branch
// ---------------------------------------------------------------------------

fn run_step04(repo: &Repo, env: &[(&str, &str)]) -> Output {
    let home = repo.root.path().join("home");
    let runtime = repo.root.path().join("runtime");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    let repo_path = repo.path();
    let mut all: Vec<(&str, &str)> = vec![
        ("REPO_PATH", repo_path.to_str().unwrap()),
        ("BRANCH_PREFIX", "feat"),
        ("ISSUE_NUMBER", "77"),
        ("TASK_DESCRIPTION", "deliver to the dev base"),
        ("EXISTING_BRANCH", ""),
        ("PR_NUMBER", ""),
        ("HOME", home.to_str().unwrap()),
        ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
    ];
    let root = workspace_root();
    all.push(("AMPLIHACK_HOME", root.to_str().unwrap()));
    for (k, v) in env {
        all.retain(|(key, _)| key != k);
        all.push((k, v));
    }
    bash(
        &step_command("workflow-worktree", "step-04-setup-worktree"),
        &repo_path,
        &all,
    )
}

fn receipt(out: &Output) -> serde_json::Value {
    let stdout = text(&out.stdout);
    let start = stdout.find('{').expect("receipt JSON");
    serde_json::from_str(stdout[start..].trim()).expect("receipt parses")
}

#[test]
fn new_branch_starts_from_base_branch_and_the_receipt_says_so() {
    let repo = Repo::new();
    let out = run_step04(&repo, &[("RECIPE_VAR_base_branch", "dev")]);
    assert!(out.status.success(), "{}", describe(&out));
    let r = receipt(&out);
    assert_eq!(r["base_ref"], "origin/dev", "{r}");
    assert_eq!(r["base_branch"], "dev", "{r}");
    let wt = PathBuf::from(r["worktree_path"].as_str().unwrap());
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), repo.rev("origin/dev"));
    assert!(
        wt.join("dev-only.txt").exists(),
        "the branch carries dev's work"
    );
}

#[test]
fn new_branch_without_base_branch_keeps_the_remote_default() {
    let repo = Repo::new();
    let out = run_step04(&repo, &[]);
    assert!(out.status.success(), "{}", describe(&out));
    let r = receipt(&out);
    assert_eq!(r["base_ref"], "origin/main", "{r}");
    let wt = PathBuf::from(r["worktree_path"].as_str().unwrap());
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), repo.rev("origin/main"));
}

#[test]
fn new_branch_never_falls_back_when_base_branch_is_missing_or_forbidden() {
    for env in [
        vec![("RECIPE_VAR_base_branch", "nope")],
        vec![
            ("RECIPE_VAR_base_branch", "dev"),
            ("RECIPE_VAR_forbidden_base_branches", "release/* dev"),
        ],
    ] {
        let repo = Repo::new();
        let out = run_step04(&repo, &env);
        assert_eq!(out.status.code(), Some(1), "{env:?}: {}", describe(&out));
        assert!(out.stdout.is_empty(), "{env:?}: no receipt");
        assert!(
            !repo.path().join("worktrees").exists(),
            "{env:?}: no worktree is created on another base"
        );
    }
}

#[test]
fn strict_delivery_never_derives_a_new_branch() {
    let repo = Repo::new();
    let out = run_step04(
        &repo,
        &[
            ("RECIPE_VAR_strict_delivery", "true"),
            ("RECIPE_VAR_base_branch", "dev"),
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(text(&out.stderr).contains("strict_delivery=true requires existing_branch"));
    assert!(!repo.path().join("worktrees").exists());
}

#[test]
fn existing_branch_receipt_reports_base_branch() {
    let repo = Repo::new();
    git(&repo.path(), &["branch", "feat/assigned", "origin/dev"]);
    git(&repo.path(), &["push", "origin", "feat/assigned"]);
    let out = run_step04(
        &repo,
        &[
            ("EXISTING_BRANCH", "feat/assigned"),
            ("RECIPE_VAR_base_branch", "dev"),
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let r = receipt(&out);
    assert_eq!(r["branch_name"], "feat/assigned", "{r}");
    assert_eq!(r["base_ref"], "origin/dev", "{r}");
    assert_eq!(r["base_branch"], "dev", "{r}");
}

#[test]
fn existing_branch_never_proceeds_on_a_bad_base() {
    // The base of an existing branch is checked at step-04, as for a new
    // branch, so a mistyped or forbidden base fails before any agent runs.
    for (base, forbidden, reason) in [
        ("nope", "", "has no remote-tracking ref origin/nope"),
        (
            "dev",
            "release/* dev",
            "matches forbidden_base_branches pattern 'dev'",
        ),
        ("feat/assigned", "", "is the branch being delivered"),
    ] {
        let repo = Repo::new();
        git(&repo.path(), &["branch", "feat/assigned", "origin/dev"]);
        git(&repo.path(), &["push", "origin", "feat/assigned"]);
        let out = run_step04(
            &repo,
            &[
                ("EXISTING_BRANCH", "feat/assigned"),
                ("RECIPE_VAR_base_branch", base),
                ("RECIPE_VAR_forbidden_base_branches", forbidden),
            ],
        );
        assert_eq!(out.status.code(), Some(1), "{base}: {}", describe(&out));
        assert!(out.stdout.is_empty(), "{base}: no receipt");
        assert!(
            text(&out.stderr).contains(reason),
            "{base}: {}",
            describe(&out)
        );
        let worktrees = git(&repo.path(), &["worktree", "list", "--porcelain"]);
        assert_eq!(
            worktrees
                .lines()
                .filter(|l| l.starts_with("worktree "))
                .count(),
            1,
            "{base}: no task worktree is set up on a bad base\n{worktrees}"
        );
    }
}

// ---------------------------------------------------------------------------
// workflow-worktree's other callers: quick-change and auto-drive-light
// ---------------------------------------------------------------------------

/// Every step in a bundled recipe that calls `workflow-worktree`, with the
/// name of the recipe it is in.
fn workflow_worktree_calls() -> Vec<(String, Value)> {
    let dir = workspace_root().join("amplifier-bundle/recipes");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
        .collect();
    paths.sort();
    let mut calls = Vec::new();
    for path in paths {
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let doc: Value = serde_yaml::from_str(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{name}.yaml does not parse: {e}"));
        for step in doc["steps"].as_sequence().into_iter().flatten() {
            if step["type"].as_str() == Some("recipe")
                && step["recipe"].as_str() == Some("workflow-worktree")
            {
                calls.push((name.clone(), step.clone()));
            }
        }
    }
    calls
}

#[test]
fn workflow_worktree_callers_outside_default_workflow_clear_base_branch() {
    // Step-04 honours an inherited base_branch, and the runner lays a
    // caller's whole context over workflow-worktree's defaults. Only
    // default-workflow opens its pull request on base_branch (step-16). Any
    // other caller opens it on origin/HEAD's branch, so it clears base_branch
    // and forbidden_base_branches in its call and the branch starts where the
    // pull request is aimed. strict_delivery stays inherited: step-04 then
    // refuses a strict run instead of delivering it on a new branch. See
    // "Where the inputs apply" in docs/reference/workflow-delivery-inputs.md.
    let calls = workflow_worktree_calls();
    let callers: Vec<&str> = calls.iter().map(|(r, _)| r.as_str()).collect();
    for expected in ["default-workflow", "quick-change", "auto-drive-light"] {
        assert!(
            callers.contains(&expected),
            "{expected} calls workflow-worktree; found {callers:?}"
        );
    }
    for (recipe, step) in &calls {
        let id = step["id"].as_str().unwrap_or("?");
        let ctx = &step["context"];
        if recipe == "default-workflow" {
            for key in ["base_branch", "forbidden_base_branches", "strict_delivery"] {
                assert!(
                    ctx[key].is_null(),
                    "default-workflow/{id} passes {key} through to step-04"
                );
            }
            continue;
        }
        for key in ["base_branch", "forbidden_base_branches"] {
            assert_eq!(
                ctx[key].as_str(),
                Some(""),
                "{recipe}/{id} must clear {key}: only default-workflow opens its pull request on base_branch"
            );
        }
        assert!(
            ctx["strict_delivery"].is_null(),
            "{recipe}/{id} must leave strict_delivery inherited, so step-04 refuses a strict run"
        );
    }
}

/// The variables step-04 sees when `recipe`'s call to `workflow-worktree`
/// runs under a parent context `inherited`. As in recipe-runner-rs: the
/// parent context, then the call's own `context:`, laid over
/// workflow-worktree's defaults, and each key exported as `RECIPE_VAR_<key>`
/// and as its uppercase alias.
fn worktree_call_env(recipe: &str, inherited: &[(&str, &str)]) -> Vec<(String, String)> {
    let call = workflow_worktree_calls()
        .into_iter()
        .find(|(r, _)| r == recipe)
        .map(|(_, step)| step)
        .unwrap_or_else(|| panic!("{recipe} calls workflow-worktree"));
    let path = workspace_root().join("amplifier-bundle/recipes/workflow-worktree.yaml");
    let worktree: Value = serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let mut ctx = std::collections::BTreeMap::new();
    for (k, v) in worktree["context"]
        .as_mapping()
        .expect("workflow-worktree context")
    {
        ctx.insert(
            k.as_str().unwrap().to_owned(),
            v.as_str().unwrap().to_owned(),
        );
    }
    for (k, v) in inherited {
        ctx.insert((*k).to_owned(), (*v).to_owned());
    }
    for (k, v) in call["context"].as_mapping().expect("call context") {
        ctx.insert(
            k.as_str().unwrap().to_owned(),
            v.as_str().unwrap().to_owned(),
        );
    }
    ctx.into_iter()
        .flat_map(|(k, v)| {
            [
                (format!("RECIPE_VAR_{k}"), v.clone()),
                (k.to_uppercase(), v),
            ]
        })
        .collect()
}

fn run_worktree_call(repo: &Repo, recipe: &str, inherited: &[(&str, &str)]) -> Output {
    let env = worktree_call_env(recipe, inherited);
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    run_step04(repo, &pairs)
}

/// The branch `recipe`'s pull request step passes to `gh pr create --base`,
/// worked out in `repo` by that step's own lines.
// The shell a step runs: its bash command, or for an agent step (#2329 made
// light-open-pr one) the command example its prompt gives the agent.
fn step_shell(recipe: &str, step_id: &str) -> String {
    let path = workspace_root().join(format!("amplifier-bundle/recipes/{recipe}.yaml"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let doc: Value = serde_yaml::from_str(&text).expect("recipe parses");
    let step = doc["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(step_id))
        .unwrap_or_else(|| panic!("{recipe}: no step {step_id}"));
    step["command"]
        .as_str()
        .or_else(|| step["prompt"].as_str())
        .unwrap_or_else(|| panic!("{recipe}/{step_id} has neither a command nor a prompt"))
        .to_owned()
}

fn pull_request_base(repo: &Repo, recipe: &str, step_id: &str) -> String {
    let command = step_shell(recipe, step_id);
    assert!(
        command.contains("--base \"$DEFAULT\""),
        "{recipe}/{step_id} opens the pull request on $DEFAULT"
    );
    let lines: Vec<&str> = command
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("DEFAULT="))
        .collect();
    assert_eq!(lines.len(), 2, "{recipe}/{step_id}: {lines:?}");
    let script = format!(
        "set -euo pipefail\n{}\nprintf '%s' \"$DEFAULT\"\n",
        lines.join("\n")
    );
    let out = bash(&script, &repo.path(), &[]);
    assert!(out.status.success(), "{}", describe(&out));
    text(&out.stdout)
}

#[test]
fn light_recipes_start_the_branch_on_the_base_their_pull_request_targets() {
    // A base_branch given to smart-orchestrator reaches quick-change and
    // auto-drive-light. Their branch must still start on the branch their
    // pull request targets, or the pull request carries every commit on dev
    // that main lacks.
    for (recipe, pr_step) in [
        ("quick-change", "quick-change-create-pr"),
        ("auto-drive-light", "light-open-pr"),
    ] {
        let repo = Repo::new();
        let out = run_worktree_call(
            &repo,
            recipe,
            &[
                ("base_branch", "dev"),
                ("forbidden_base_branches", "release/*"),
                ("issue_number", "77"),
                ("task_description", "fix a typo"),
            ],
        );
        assert!(out.status.success(), "{recipe}: {}", describe(&out));
        let r = receipt(&out);
        let base = pull_request_base(&repo, recipe, pr_step);
        assert_eq!(base, "main", "{recipe}");
        assert_eq!(r["base_ref"], format!("origin/{base}"), "{recipe}: {r}");
        let wt = PathBuf::from(r["worktree_path"].as_str().unwrap());
        assert_eq!(
            git(&wt, &["rev-parse", "HEAD"]),
            repo.rev("origin/main"),
            "{recipe}"
        );
        assert!(
            !wt.join("dev-only.txt").exists(),
            "{recipe}: dev's commits stay out of a pull request into main"
        );
    }
}

#[test]
fn light_recipes_refuse_an_inherited_strict_delivery() {
    for recipe in ["quick-change", "auto-drive-light"] {
        let repo = Repo::new();
        let out = run_worktree_call(
            &repo,
            recipe,
            &[("strict_delivery", "true"), ("base_branch", "dev")],
        );
        assert_eq!(out.status.code(), Some(1), "{recipe}: {}", describe(&out));
        assert!(
            text(&out.stderr).contains("strict_delivery=true requires existing_branch"),
            "{recipe}: {}",
            describe(&out)
        );
        assert!(out.stdout.is_empty(), "{recipe}: no receipt");
        assert!(
            !repo.path().join("worktrees").exists(),
            "{recipe}: no branch is created for a strict run"
        );
    }
}

// ---------------------------------------------------------------------------
// smart-orchestrator: concurrency controls preserve every decomposition leaf
// ---------------------------------------------------------------------------

/// smart-classify-route's activate-workflow with a decomposition of three
/// workstreams. Returns the step's output and stderr.
fn run_activate_workflow(strict: &str, force_single: &str) -> (String, String) {
    let dir = TempDir::new().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let fake = bin.join("amplihack");
    fs::write(
        &fake,
        "#!/usr/bin/env bash\n[ \"$*\" = \"orch helper count-workstreams\" ] || exit 97\ncat >/dev/null\nprintf 3\n",
    )
    .unwrap();
    fs::set_permissions(
        &fake,
        <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let project = dir.path().join("project");
    fs::create_dir(&project).unwrap();
    let out = bash(
        &step_command("smart-classify-route", "activate-workflow"),
        &project,
        &[
            ("PATH", &path),
            ("CLAUDE_PROJECT_DIR", project.to_str().unwrap()),
            (
                "RECIPE_VAR_decomposition_json",
                "{\"workstreams\":[{},{},{}]}",
            ),
            ("RECIPE_VAR_task_type", "Development"),
            ("RECIPE_VAR_force_single_workstream", force_single),
            ("RECIPE_VAR_strict_delivery", strict),
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));
    (text(&out.stdout), text(&out.stderr))
}

#[test]
fn smart_orchestrator_preserves_all_leaves_under_strict_and_capacity_settings() {
    // Strict delivery and the concurrency override do not discard leaves
    // from the decomposition.
    let doc: Value = serde_yaml::from_str(
        &fs::read_to_string(
            workspace_root().join("amplifier-bundle/recipes/smart-classify-route.yaml"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(doc["context"]["strict_delivery"].as_str(), Some("false"));
    for (strict, force_single) in [
        ("", ""),
        ("false", ""),
        ("false", "true"),
        ("true", ""),
        ("TRUE", ""),
    ] {
        let (count, stderr) = run_activate_workflow(strict, force_single);
        assert_eq!(
            count, "3",
            "strict={strict:?} force={force_single:?}\n{stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// step-15: the commit message's issue reference
// ---------------------------------------------------------------------------

/// Step-15's issue-reference block, from `ISSUE_TAG="#${ISSUE_NUMBER}"` to
/// the `fi` that closes it, exactly as the recipe has it.
fn step15_issue_tag_block() -> String {
    let command = step_command("workflow-publish", "step-15-commit-push");
    let lines: Vec<&str> = command.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == "ISSUE_TAG=\"#${ISSUE_NUMBER}\"")
        .expect("step-15 sets ISSUE_TAG");
    let end = start
        + lines[start..]
            .iter()
            .position(|l| l.trim() == "fi")
            .expect("the ISSUE_TAG block ends with fi");
    lines[start..=end].join("\n")
}

#[test]
fn step15_names_the_issue_with_the_shared_rule() {
    let dir = TempDir::new().unwrap();
    let tools = workspace_root().join("amplifier-bundle/tools");
    let script = format!(
        "set -euo pipefail\n{}\nprintf '%s\\n' \"$ISSUE_TAG\"\n",
        step15_issue_tag_block()
    );
    for (strict, repository, expected) in [
        // GitHub compares repository names case-insensitively, and so does
        // strict_issue_tag; step-15 used to write Owner/Repo#42 here.
        ("true", "Owner/Repo", "#42"),
        ("true", "owner/repo", "#42"),
        ("true", "plan/tasks", "plan/tasks#42"),
        ("false", "plan/tasks", "#42"),
    ] {
        let out = bash(
            &script,
            dir.path(),
            &[
                ("WF_TOOLS", tools.to_str().unwrap()),
                ("ISSUE_NUMBER", "42"),
                ("RECIPE_VAR_strict_delivery", strict),
                ("RECIPE_VAR_repository", repository),
                ("RECIPE_VAR_pr_repository", "owner/repo"),
            ],
        );
        assert!(
            out.status.success(),
            "{strict} {repository}: {}",
            describe(&out)
        );
        assert_eq!(text(&out.stdout).trim(), expected, "{strict} {repository}");
    }
    let out = bash(
        &script,
        dir.path(),
        &[
            ("WF_TOOLS", dir.path().to_str().unwrap()),
            ("ISSUE_NUMBER", "42"),
            ("RECIPE_VAR_strict_delivery", "true"),
            ("RECIPE_VAR_repository", "plan/tasks"),
            ("RECIPE_VAR_pr_repository", "owner/repo"),
        ],
    );
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    assert!(text(&out.stderr).contains("a strict_delivery commit cannot name its issue"));
}

// ---------------------------------------------------------------------------
// finalize: a pull request opened on base_branch
// ---------------------------------------------------------------------------

/// Fake gh for finalize. Pull request 7 is `feat/x` into `dev`, a draft at
/// `$HEAD_SHA`. Every call is logged; anything unexpected fails.
const FAKE_GH_FINALIZE: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
pr() {
  printf '{"number":7,"title":"Deliver to dev","body":"","state":"OPEN","createdAt":"2026-11-11T00:00:00Z","mergedAt":null,"url":"https://github.com/owner/repo/pull/7","headRefName":"feat/x","baseRefName":"dev","headRefOid":"%s","headRepositoryOwner":{"login":"owner"},"headRepository":{"name":"repo"},"isCrossRepository":false,"statusCheckRollup":[],"isDraft":true,"mergeable":"MERGEABLE","reviews":[]}' "$HEAD_SHA"
}
case "$1 $2" in
  "auth status") exit 0 ;;
  "pr view") pr; echo ;;
  "pr list") printf '['; pr; echo ']' ;;
  "pr ready") echo "marked ready: $3" ;;
  "pr comment") echo "$3#issuecomment-1" ;;
  *) echo "fake gh: unexpected: $*" >&2; exit 7 ;;
esac
"#;

const PR_URL: &str = "https://github.com/owner/repo/pull/7";

/// `Repo` with `feat/x` checked out: one commit on top of `dev`, while
/// `origin/HEAD` is `main`. The origin URL is github.com's, as finalize needs.
struct FinalizeFixture {
    repo: Repo,
    bin: PathBuf,
    log: PathBuf,
}

impl FinalizeFixture {
    fn new() -> Self {
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

    fn worktree(&self) -> PathBuf {
        fs::canonicalize(self.repo.path()).unwrap()
    }

    /// Runs `step` of `recipe` as the runner would, with `base_branch` set.
    fn run(&self, recipe: &str, step: &str, base_branch: &str) -> (Output, String) {
        let _ = fs::remove_file(&self.log);
        let worktree = self.worktree();
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let setup =
            serde_json::json!({ "worktree_path": worktree, "branch_name": "feat/x" }).to_string();
        let publish =
            serde_json::json!({ "pr_url": PR_URL, "pr_number": "7", "state": "FOLLOWUP_CREATED" })
                .to_string();
        let root = workspace_root();
        let out = bash(
            &step_command(recipe, step),
            &worktree,
            &[
                ("PATH", &path),
                ("GH_LOG", self.log.to_str().unwrap()),
                ("HEAD_SHA", &self.repo.rev("HEAD")),
                ("AMPLIHACK_HOME", root.to_str().unwrap()),
                ("REPO_PATH", worktree.to_str().unwrap()),
                ("REMOTE_HOST_TYPE", "github"),
                ("RECIPE_VAR_repo_path", worktree.to_str().unwrap()),
                ("RECIPE_VAR_worktree_setup", &setup),
                ("RECIPE_VAR_pr_publish_result", &publish),
                ("RECIPE_VAR_pr_url", PR_URL),
                ("RECIPE_VAR_terminal_fail_mode", "report"),
                ("RECIPE_VAR_base_branch", base_branch),
            ],
        );
        (out, fs::read_to_string(&self.log).unwrap_or_default())
    }
}

#[test]
fn every_finalize_recipe_declares_base_branch() {
    // The runner lays a parent's whole context over each sub-recipe's
    // defaults and exports every key as RECIPE_VAR_<key>, declared or not, so
    // the declaration is not what carries base_branch down. Each recipe on the
    // way declares it anyway: the empty default applies when the recipe runs
    // on its own, and it keeps a nested process from reading a stale inherited
    // RECIPE_VAR_base_branch (#1561). See "How the inputs reach each phase" in
    // docs/reference/workflow-delivery-inputs.md.
    for recipe in [
        "default-workflow",
        "workflow-publish",
        "workflow-finalize",
        "workflow-terminal-state",
        "workflow-terminal-adjudication",
    ] {
        let path = workspace_root().join(format!("amplifier-bundle/recipes/{recipe}.yaml"));
        let doc: Value = serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            doc["context"]["base_branch"].as_str(),
            Some(""),
            "{recipe} must declare base_branch: \"\""
        );
    }
}

#[test]
fn terminal_state_probe_checks_the_pull_request_against_base_branch() {
    let fx = FinalizeFixture::new();
    let (out, log) = fx.run("workflow-terminal-state", "probe-terminal-state", "dev");
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    let stdout = text(&out.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .expect("probe JSON");
    let state: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(state["terminal_state"], "FOLLOWUP_CREATED", "{state}");
    assert_eq!(state["should_run_ci_wait"], "true", "{state}");
    assert_eq!(state["pr_url"], PR_URL, "{state}");
    assert_eq!(
        state["commits_ahead"], "1",
        "counted from origin/dev, not origin/main: {state}"
    );

    // Control: the same pull request, with the base discovered from
    // origin/HEAD, is rejected; the fixture would catch a lost base_branch.
    let (out, _) = fx.run("workflow-terminal-state", "probe-terminal-state", "");
    let stdout = text(&out.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .expect("probe JSON");
    let state: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(state["terminal_state"], "FAILED_INVALID_INPUT", "{state}");
}

#[test]
fn step21_marks_a_pull_request_into_base_branch_ready() {
    let fx = FinalizeFixture::new();
    let (out, log) = fx.run("workflow-finalize", "step-21-pr-ready", "dev");
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    assert!(
        log.lines().any(|l| l == format!("pr ready {PR_URL}")),
        "the draft is marked ready\n{log}"
    );

    // Control: without base_branch the helper expects origin/HEAD (main), so
    // the pull request into dev does not match and nothing is mutated.
    let (out, log) = fx.run("workflow-finalize", "step-21-pr-ready", "");
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        text(&out.stderr).contains("no_scoped_pr"),
        "{}",
        describe(&out)
    );
    assert!(!log.contains("pr ready"), "{log}");
}

#[test]
fn step22b_matches_the_pull_request_on_base_branch() {
    let fx = FinalizeFixture::new();
    let (out, log) = fx.run("workflow-finalize", "step-22b-final-status", "dev");
    assert!(out.status.success(), "{}\n{log}", describe(&out));
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        !all.contains("scoped final PR match did not resolve"),
        "the scoped match finds the pull request into dev\n{}",
        describe(&out)
    );
    assert!(
        text(&out.stdout).contains("\"baseRefName\":\"dev\""),
        "the final status shows the matched pull request\n{}",
        describe(&out)
    );

    let (out, _) = fx.run("workflow-finalize", "step-22b-final-status", "");
    assert!(
        text(&out.stderr).contains("scoped final PR match did not resolve: no_scoped_pr"),
        "control: without base_branch the match misses\n{}",
        describe(&out)
    );
}

#[test]
fn finalization_evidence_measures_the_diff_from_base_branch() {
    let fx = FinalizeFixture::new();
    for (base, expected_ref, ahead) in [("dev", "origin/dev", "1"), ("", "origin/main", "2")] {
        let (out, log) = fx.run("workflow-finalize", "collect-finalization-evidence", base);
        assert!(out.status.success(), "{base:?}: {}\n{log}", describe(&out));
        let evidence: serde_json::Value =
            serde_json::from_str(&text(&out.stdout)).expect("evidence JSON");
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
