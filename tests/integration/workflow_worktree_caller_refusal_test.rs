//! Integration tests for issue #858 — workflow-worktree caller-checkout reuse refusal.
//!
//! These tests are written FIRST (TDD red), against the current
//! `workflow-worktree.yaml`. They MUST fail until the #858 refusal gates land in
//! `step-04-setup-worktree` (design components C1/C3). Once implemented they
//! define the fail-closed safety contract.
//!
//! ## Contract under test (issue #858)
//!
//! The existing-branch path (EXISTING_BRANCH / PR_NUMBER) MUST NOT reuse the
//! caller's own checkout as the task worktree. Reusing the caller checkout leaks
//! the caller's dirty state (uncommitted edits, untracked files, unpushed
//! commits) into the task and lets the workflow mutate the human operator's
//! working tree. The recipe must therefore **refuse, fail-closed**:
//!
//!   * **Gate A — stale registration:** a worktree is registered for the branch
//!     but its path is inaccessible → `exit 1`, advise `git worktree prune`.
//!   * **Gate B — caller-reuse by canonical path:** the resolved worktree
//!     canonicalizes to the same real path as `REPO_PATH` → refuse, `exit 1`.
//!     Canonicalization (realpath) defeats symlink / `..` bypasses.
//!   * **Gate C — caller-reuse by HEAD:** the caller's `HEAD` symbolic-ref equals
//!     the target branch → refuse, `exit 1`.
//!
//! ## Refusal contract (SR-6 — the core #858 property)
//!
//!   * exit code == 1 (fail-closed; never 0, never fall-through),
//!   * **zero stdout** (no partial JSON emission before the refusal — no
//!     disclosure of a bogus worktree path to downstream steps),
//!   * stderr cites issue **#858** with a remediation hint,
//!   * no new worktree is created and the caller checkout is not mutated.
//!
//! ## Isolation contract (R3a)
//!
//! On the *new-branch* path (empty existing_branch/pr_number) a dirty caller
//! checkout MUST NOT leak into the freshly created task worktree: no untracked
//! files, no uncommitted edits, and no unpushed caller commits appear there.
//!
//! ## Non-regression (RK5)
//!
//! Reuse of a *separate* linked worktree (one that is NOT the caller checkout)
//! must still succeed (`exit 0`, `created=false`) — we must not over-refuse.
//!
//! Test strategy mirrors `existing_branch_context_test.rs`: parse the recipe
//! YAML, extract the `step-04-setup-worktree` `command:` block, and drive it as a
//! `bash -c` subprocess against a real tempdir git repo with a **bare local
//! origin** — no network, no git mocking.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_yaml::Value;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Repo / recipe paths
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

fn workflow_worktree_yaml() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/workflow-worktree.yaml")
}

// ---------------------------------------------------------------------------
// Recipe parsing helpers
// ---------------------------------------------------------------------------

fn load_recipe(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {} as YAML: {e}", path.display()))
}

/// Find a step by id under `steps:` and return its `command:` body.
fn extract_step_body(recipe: &Value, step_id: &str) -> String {
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have a top-level 'steps' sequence");

    for step in steps {
        let id = step.get("id").and_then(Value::as_str).unwrap_or("");
        if id == step_id {
            if let Some(cmd) = step.get("command").and_then(Value::as_str) {
                return cmd.to_owned();
            }
            panic!("step '{step_id}' has no 'command:' body");
        }
    }
    panic!("step '{step_id}' not found in recipe");
}

fn step04_body() -> String {
    extract_step_body(
        &load_recipe(&workflow_worktree_yaml()),
        "step-04-setup-worktree",
    )
}

// ---------------------------------------------------------------------------
// Git fixture (bare local origin — network-free)
// ---------------------------------------------------------------------------

struct GitFixture {
    _origin: TempDir,
    _repo: TempDir,
    repo_path: PathBuf,
}

fn git(cwd: &Path, args: &[&str]) -> Output {
    let out = amplihack_git::command()
        .args(["-c", "user.email=test@test", "-c", "user.name=test"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

impl GitFixture {
    fn new() -> Self {
        let origin = TempDir::new().expect("origin tempdir");
        amplihack_git::command()
            .args(["init", "--bare", "-b", "main"])
            .arg(origin.path())
            .output()
            .expect("git init --bare");

        let repo = TempDir::new().expect("repo tempdir");
        let rp = repo.path().to_path_buf();
        git(&rp, &["init", "-b", "main"]);
        git(
            &rp,
            &["remote", "add", "origin", origin.path().to_str().unwrap()],
        );
        fs::write(rp.join("README.md"), "init\n").unwrap();
        git(&rp, &["add", "README.md"]);
        git(&rp, &["commit", "-m", "init"]);
        git(&rp, &["push", "-u", "origin", "HEAD:main"]);
        // Establish origin/HEAD so resolve_base_ref() finds a base without network.
        let _ = amplihack_git::command()
            .args(["remote", "set-head", "origin", "-a"])
            .current_dir(&rp)
            .output();
        let _ = amplihack_git::command()
            .args(["branch", "--set-upstream-to=origin/main", "main"])
            .current_dir(&rp)
            .output();

        Self {
            _origin: origin,
            _repo: repo,
            repo_path: rp,
        }
    }

    /// Create a local branch pointing at main (no checkout switch).
    fn create_local_branch(&self, name: &str) {
        git(&self.repo_path, &["branch", name, "main"]);
    }

    /// Switch the caller's own checkout (REPO_PATH) onto `name`.
    fn checkout(&self, name: &str) {
        git(&self.repo_path, &["checkout", name]);
    }

    /// Create a *separate* linked worktree for `name` (NOT the caller checkout).
    /// Returns its path. Creates the branch if absent.
    fn add_linked_worktree(&self, name: &str, at: &Path) {
        if git(&self.repo_path, &["branch", "--list", name])
            .stdout
            .is_empty()
        {
            self.create_local_branch(name);
        }
        git(
            &self.repo_path,
            &["worktree", "add", at.to_str().unwrap(), name],
        );
    }
}

// ---------------------------------------------------------------------------
// Bash runner
// ---------------------------------------------------------------------------

struct RunResult {
    status: i32,
    stdout: String,
    stderr: String,
}

fn run_bash(script: &str, env: &HashMap<&str, String>, cwd: &Path) -> RunResult {
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(script).current_dir(cwd);
    cmd.env_clear();
    if let Some(home) = std::env::var_os("HOME") {
        cmd.env("HOME", home);
    }
    let path = env
        .get("PATH")
        .cloned()
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()));
    cmd.env("PATH", path);
    for (k, v) in env {
        if *k == "PATH" {
            continue;
        }
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn bash");
    RunResult {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn parse_json(stdout: &str) -> serde_json::Value {
    let start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("no JSON object in stdout:\n{stdout}"));
    let slice = &stdout[start..];
    let end = slice
        .rfind('}')
        .unwrap_or_else(|| panic!("no closing brace in stdout:\n{stdout}"));
    let candidate = &slice[..=end];
    serde_json::from_str(candidate).unwrap_or_else(|e| panic!("invalid JSON {candidate:?}: {e}"))
}

fn base_env(fix: &GitFixture) -> HashMap<&'static str, String> {
    let mut env = HashMap::new();
    env.insert("REPO_PATH", fix.repo_path.to_string_lossy().into_owned());
    env.insert("BRANCH_PREFIX", "feat".to_owned());
    env.insert("ISSUE_NUMBER", "858".to_owned());
    env.insert(
        "TASK_DESCRIPTION",
        "issue 858 caller checkout refusal".to_owned(),
    );
    env.insert("EXISTING_BRANCH", String::new());
    env.insert("PR_NUMBER", String::new());
    // Point AMPLIHACK_HOME at the tempdir repo so the best-effort sweep helper
    // is not found and cannot touch anything outside the fixture.
    env.insert(
        "AMPLIHACK_HOME",
        fix.repo_path.to_string_lossy().into_owned(),
    );
    // Pin HOME to the fixture too: step-04's issue-#840 self-healing sweep also
    // probes $HOME/.copilot and $HOME/.amplihack for the sweep helper. Leaking
    // the real HOME would let that helper run `git worktree prune` on the
    // fixture and silently clear the stale registration Gate A must observe.
    env.insert("HOME", fix.repo_path.to_string_lossy().into_owned());
    env
}

/// Assert the fail-closed refusal contract (SR-6):
/// exit 1, empty stdout, stderr cites #858.
fn assert_refusal(r: &RunResult) {
    assert_eq!(
        r.status, 1,
        "refusal must exit 1 (fail-closed); got exit {}\nstdout=\n{}\nstderr=\n{}",
        r.status, r.stdout, r.stderr
    );
    assert!(
        r.stdout.trim().is_empty(),
        "refusal must emit ZERO stdout (no partial JSON disclosure); got stdout=\n{}",
        r.stdout
    );
    assert!(
        r.stderr.contains("#858"),
        "refusal must cite issue #858 on stderr; got stderr=\n{}",
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// Gate C — caller HEAD on the target branch → refuse (HEAD-match vector)
// ---------------------------------------------------------------------------

/// The caller's own checkout is on the target branch. Reusing it as the task
/// worktree would let the workflow mutate the operator's tree. Refuse.
#[test]
fn gate_c_caller_head_on_target_branch_refused() {
    let fix = GitFixture::new();
    fix.create_local_branch("feat/caller-on-branch");
    fix.checkout("feat/caller-on-branch");

    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "feat/caller-on-branch".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_refusal(&r);

    // The caller checkout must not have been adopted as a task worktree.
    assert!(
        !fix.repo_path.join("worktrees").exists(),
        "no task worktree should be created under the caller repo on refusal"
    );
    // stderr should steer the operator away from running inside their own checkout.
    let lc = r.stderr.to_lowercase();
    assert!(
        lc.contains("caller") || lc.contains("checkout") || lc.contains("worktree"),
        "refusal must explain the caller-checkout reuse hazard; stderr=\n{}",
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// Gate B — resolved worktree == caller REPO_PATH (canonical path-match vector)
// ---------------------------------------------------------------------------

/// When the caller is on the branch, `git worktree list` resolves the branch's
/// worktree to REPO_PATH itself. The path-match gate must refuse regardless of
/// the HEAD gate.
#[test]
fn gate_b_resolved_worktree_equals_repo_path_refused() {
    let fix = GitFixture::new();
    fix.create_local_branch("feat/path-match");
    fix.checkout("feat/path-match");

    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "feat/path-match".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_refusal(&r);
}

/// SR-4: a symlinked REPO_PATH must not bypass the path-match gate. Both sides
/// must be canonicalized (realpath) before comparison.
#[test]
fn gate_b_symlinked_repo_path_still_refused() {
    let fix = GitFixture::new();
    fix.create_local_branch("feat/symlink-bypass");
    fix.checkout("feat/symlink-bypass");

    // Present REPO_PATH via a symlink pointing at the real on-branch checkout.
    let link_home = TempDir::new().expect("symlink home");
    let link = link_home.path().join("repo-link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&fix.repo_path, &link).expect("create symlink");

    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("REPO_PATH", link.to_string_lossy().into_owned());
    env.insert("EXISTING_BRANCH", "feat/symlink-bypass".to_owned());

    let r = run_bash(&body, &env, &link);
    assert_refusal(&r);
}

// ---------------------------------------------------------------------------
// Gate A — stale worktree registration → refuse and advise prune
// ---------------------------------------------------------------------------

/// A worktree is registered for the branch but its directory is gone (a prior
/// run leaked it without pruning). Refuse and tell the operator to prune —
/// do NOT silently emit a JSON path pointing at a nonexistent worktree.
#[test]
fn gate_a_stale_worktree_registration_refused() {
    let fix = GitFixture::new();

    // Register a linked worktree for the branch, then delete its directory to
    // leave a stale registration (no `git worktree prune`).
    let wt_home = TempDir::new().expect("wt home");
    let stale = wt_home.path().join("stale-wt");
    fix.add_linked_worktree("feat/stale-reg", &stale);
    fs::remove_dir_all(&stale).expect("delete worktree dir to make it stale");

    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "feat/stale-reg".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_refusal(&r);
    assert!(
        r.stderr.to_lowercase().contains("prune"),
        "stale-registration refusal must advise `git worktree prune`; stderr=\n{}",
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// R3a — dirty caller state must not leak into a NEW task worktree
// ---------------------------------------------------------------------------

/// On the new-branch path (no existing_branch/pr_number), a dirty caller
/// checkout — unpushed commit, uncommitted edit, and untracked file — must NOT
/// leak into the freshly created task worktree.
#[test]
fn dirty_caller_state_does_not_leak_into_new_worktree() {
    let fix = GitFixture::new();

    // Caller checkout goes dirty in three distinct ways:
    // 1. an unpushed commit on main (ahead of origin/main),
    fs::write(fix.repo_path.join("caller_only.txt"), "caller commit\n").unwrap();
    git(&fix.repo_path, &["add", "caller_only.txt"]);
    git(&fix.repo_path, &["commit", "-m", "unpushed caller commit"]);
    // 2. an uncommitted edit to a tracked file,
    fs::write(fix.repo_path.join("README.md"), "DIRTY-UNCOMMITTED\n").unwrap();
    // 3. an untracked file.
    fs::write(fix.repo_path.join("leaked.txt"), "untracked\n").unwrap();

    let body = step04_body();
    let env = base_env(&fix); // EXISTING_BRANCH / PR_NUMBER empty → new-branch path

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_eq!(
        r.status, 0,
        "new-branch path must succeed; stderr=\n{}",
        r.stderr
    );
    let json = parse_json(&r.stdout);
    let wp = json["worktree_path"].as_str().expect("worktree_path");
    let wt = Path::new(wp);
    assert!(wt.is_dir(), "new worktree must exist on disk: {wp}");

    // Isolation assertions: none of the caller's dirty artifacts leaked.
    assert!(
        !wt.join("leaked.txt").exists(),
        "untracked caller file leaked into new worktree"
    );
    assert!(
        !wt.join("caller_only.txt").exists(),
        "unpushed caller commit leaked into new worktree"
    );
    let readme = fs::read_to_string(wt.join("README.md")).unwrap_or_default();
    assert_eq!(
        readme, "init\n",
        "uncommitted caller edit leaked into new worktree README"
    );

    // The new branch is based on the resolved remote base, not the caller's HEAD.
    let ahead = amplihack_git::command()
        .args(["rev-list", "--count", "origin/main..HEAD"])
        .current_dir(wt)
        .output()
        .expect("git rev-list");
    assert_eq!(
        String::from_utf8_lossy(&ahead.stdout).trim(),
        "0",
        "new worktree must start at origin/main with zero caller commits ahead"
    );
}

// ---------------------------------------------------------------------------
// RK5 — do not over-refuse legitimate reuse of a SEPARATE linked worktree
// ---------------------------------------------------------------------------

/// A pre-existing *separate* linked worktree (not the caller checkout) for the
/// branch must still be reused (exit 0, created=false). The refusal gates must
/// target only the caller's own checkout.
#[test]
fn separate_linked_worktree_still_reused_not_refused() {
    let fix = GitFixture::new();

    let wt_home = TempDir::new().expect("wt home");
    let sep = wt_home.path().join("sep-wt");
    fix.add_linked_worktree("feat/separate-wt", &sep);
    // Caller stays on main (NOT on the target branch).

    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "feat/separate-wt".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_eq!(
        r.status, 0,
        "legitimate separate-worktree reuse must NOT be refused; stderr=\n{}",
        r.stderr
    );
    let json = parse_json(&r.stdout);
    assert_eq!(
        json["branch_name"].as_str().unwrap(),
        "feat/separate-wt",
        "branch_name must equal the requested existing branch"
    );
    assert_eq!(
        json["created"],
        serde_json::Value::Bool(false),
        "reusing an existing branch/worktree must report created=false"
    );
    let wp = json["worktree_path"].as_str().unwrap();
    assert_eq!(
        fs::canonicalize(wp).unwrap(),
        fs::canonicalize(&sep).unwrap(),
        "must reuse the separate linked worktree, not the caller checkout"
    );
}
