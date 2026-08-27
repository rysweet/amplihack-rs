//! Integration tests for issue #858 — recipe task worktrees must ALWAYS be
//! created from a freshly-fetched remote base ref on a fresh dedicated branch,
//! never inheriting the caller checkout's current branch, local commits, or
//! uncommitted working tree.
//!
//! Written FIRST (TDD red) against the current recipes:
//!   * `amplifier-bundle/recipes/workflow-worktree.yaml` — `step-04-setup-worktree`
//!     (deterministic `type: bash` step; base-ref resolution is already
//!     remote-only via `resolve_base_ref()`).
//!   * `amplifier-bundle/recipes/consensus-issue-worktree.yaml` —
//!     `step3-setup-worktree` (an *agent* step whose shell logic lives inside a
//!     fenced ```bash block in the agent's instruction prompt; still carries the
//!     `BASE_REF="HEAD"` fallbacks that constitute Vector A).
//!
//! Strategy (mirrors `existing_branch_context_test.rs`):
//!   * Parse the recipe YAML with `serde_yaml`.
//!   * For the bash step, extract its `command:` body. For the consensus agent
//!     step, extract the fenced ```bash block from its `prompt:`.
//!   * Drive that block as a `bash -c` subprocess against a real tempdir git
//!     repo with a bare *local* origin — no network, no git mocking.
//!   * Inspect the worktree the step actually created and assert the
//!     clean-baseline invariants.
//!
//! Clean-baseline invariants (design §Quick Start):
//!   I1. `<base_ref>..HEAD` in the created worktree contains **zero** commits
//!       that were not already reachable from the base ref.
//!   I2. `git status --porcelain` in the created worktree is **empty**.
//!   I3. the created `worktree_path` is a **dedicated** directory, never the
//!       caller's `repo_path`.
//!
//! Expected result on the CURRENT (pre-fix) recipes:
//!   * `consensus_master_only_origin_does_not_inherit_caller_commits` — FAILS
//!     (Vector A: `BASE_REF="HEAD"` bakes the caller's local commit in).
//!   * `consensus_no_origin_fails_closed_instead_of_head_fallback` — FAILS
//!     (silently branches off caller HEAD instead of failing visibly).
//!   * the workflow-worktree tests and the consensus main-origin test PASS —
//!     they are regression guards proving the fix does not regress the already
//!     remote-only paths.

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

fn consensus_issue_worktree_yaml() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/consensus-issue-worktree.yaml")
}

// ---------------------------------------------------------------------------
// Recipe parsing helpers
// ---------------------------------------------------------------------------

fn load_recipe(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {} as YAML: {e}", path.display()))
}

/// Find a step by id and return its `command:` body (bash steps) or its
/// `prompt:` body (agent steps).
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
            if let Some(prompt) = step.get("prompt").and_then(Value::as_str) {
                return prompt.to_owned();
            }
            panic!("step '{step_id}' has neither 'command:' nor 'prompt:' body");
        }
    }
    panic!("step '{step_id}' not found in recipe");
}

/// Extract the first fenced ```bash ... ``` block from an agent prompt body.
/// The consensus worktree step embeds its executable logic this way.
fn extract_fenced_bash(prompt: &str) -> String {
    let open = prompt
        .find("```bash")
        .unwrap_or_else(|| panic!("no ```bash fence found in prompt:\n{prompt}"));
    // Skip past the "```bash" marker and its trailing newline.
    let after_marker = &prompt[open + "```bash".len()..];
    let body_start = after_marker
        .find('\n')
        .map(|nl| nl + 1)
        .unwrap_or_else(|| panic!("malformed ```bash fence (no newline after marker)"));
    let rest = &after_marker[body_start..];
    let close = rest
        .find("```")
        .unwrap_or_else(|| panic!("unterminated ```bash fence in prompt"));
    rest[..close].to_owned()
}

fn workflow_step04_body() -> String {
    extract_step_body(
        &load_recipe(&workflow_worktree_yaml()),
        "step-04-setup-worktree",
    )
}

fn consensus_step3_body() -> String {
    let prompt = extract_step_body(
        &load_recipe(&consensus_issue_worktree_yaml()),
        "step3-setup-worktree",
    );
    extract_fenced_bash(&prompt)
}

// ---------------------------------------------------------------------------
// Git helpers
// ---------------------------------------------------------------------------

fn git(cwd: &Path, args: &[&str]) -> Output {
    let out = amplihack_git::command()
        .args(["-c", "user.email=test@test", "-c", "user.name=test"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed:\n{}\n{}",
        cwd.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

fn git_stdout(cwd: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git(cwd, args).stdout)
        .trim()
        .to_owned()
}

/// Count of commits in `<range>` (e.g. "origin/main..HEAD"). Panics if the range
/// cannot be evaluated (missing ref) — the caller must ensure the base ref is
/// available locally.
fn rev_count(cwd: &Path, range: &str) -> i64 {
    let out = amplihack_git::command()
        .args(["rev-list", "--count", range])
        .current_dir(cwd)
        .output()
        .expect("spawn git rev-list");
    assert!(
        out.status.success(),
        "git rev-list --count {range} in {} failed:\n{}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("rev-list count is an integer")
}

/// Return the worktree path (from `git worktree list --porcelain`) whose path is
/// under `under` and is not `repo` itself. `None` if no such worktree exists.
fn worktree_under(repo: &Path, under: &Path) -> Option<PathBuf> {
    let out = git(repo, &["worktree", "list", "--porcelain"]);
    let text = String::from_utf8_lossy(&out.stdout);
    let repo_canon = fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let under_canon = fs::canonicalize(under).unwrap_or_else(|_| under.to_path_buf());
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            let path = PathBuf::from(p.trim());
            let canon = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if canon == repo_canon {
                continue;
            }
            if canon.starts_with(&under_canon) {
                return Some(path);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Git fixture: bare local origin + a dirty, non-main caller checkout
// ---------------------------------------------------------------------------

struct GitFixture {
    _origin: Option<TempDir>,
    _repo: TempDir,
    repo_path: PathBuf,
    /// e.g. "main" or "master" — the origin default branch name (None if no origin).
    default_branch: Option<String>,
    /// The non-main feature branch the caller checkout is left sitting on.
    caller_branch: String,
    /// Relative path of the uncommitted file left in the caller working tree.
    dirty_file: String,
}

impl GitFixture {
    /// Build a repo whose bare origin's default branch is `default_branch`
    /// (e.g. "main" or "master"), then leave the caller checkout on a dirty,
    /// non-main feature branch carrying one *unrelated* local commit AND one
    /// uncommitted file — exactly the #858 failure setup.
    fn with_origin(default_branch: &str) -> Self {
        let origin = TempDir::new().expect("origin tempdir");
        let ok = amplihack_git::command()
            .args(["init", "--bare", "-b", default_branch])
            .arg(origin.path())
            .output()
            .expect("git init --bare");
        assert!(ok.status.success(), "git init --bare failed");

        let repo = TempDir::new().expect("repo tempdir");
        let rp = repo.path().to_path_buf();
        git(&rp, &["init", "-b", default_branch]);
        git(
            &rp,
            &["remote", "add", "origin", origin.path().to_str().unwrap()],
        );
        fs::write(rp.join("README.md"), "init\n").unwrap();
        git(&rp, &["add", "README.md"]);
        git(&rp, &["commit", "-m", "init"]);
        git(
            &rp,
            &["push", "-u", "origin", &format!("HEAD:{default_branch}")],
        );
        // Make the remote-tracking ref for the base branch available locally so
        // assertions can reference it. This mirrors what a real fetch produces.
        git(&rp, &["fetch", "origin"]);

        let mut me = Self {
            _origin: Some(origin),
            _repo: repo,
            repo_path: rp,
            default_branch: Some(default_branch.to_owned()),
            caller_branch: "feat/overseer-goal-board-health".to_owned(),
            dirty_file: "src/disk_health.rs".to_owned(),
        };
        me.dirty_non_main_caller();
        me
    }

    /// Build a repo with NO origin remote at all, on a dirty feature branch.
    fn without_origin() -> Self {
        let repo = TempDir::new().expect("repo tempdir");
        let rp = repo.path().to_path_buf();
        git(&rp, &["init", "-b", "main"]);
        fs::write(rp.join("README.md"), "init\n").unwrap();
        git(&rp, &["add", "README.md"]);
        git(&rp, &["commit", "-m", "init"]);

        let mut me = Self {
            _origin: None,
            _repo: repo,
            repo_path: rp,
            default_branch: None,
            caller_branch: "feat/overseer-goal-board-health".to_owned(),
            dirty_file: "src/disk_health.rs".to_owned(),
        };
        me.dirty_non_main_caller();
        me
    }

    /// Check out a non-main feature branch, add one unrelated local (unpushed)
    /// commit, and leave one uncommitted file in the working tree.
    fn dirty_non_main_caller(&mut self) {
        let rp = &self.repo_path;
        git(rp, &["checkout", "-b", &self.caller_branch]);
        // Unrelated local commit — simulates the "overseer" commit from #858.
        fs::create_dir_all(rp.join("src")).unwrap();
        fs::write(rp.join("src/overseer.rs"), "// unrelated overseer work\n").unwrap();
        git(rp, &["add", "src/overseer.rs"]);
        git(rp, &["commit", "-m", "unrelated overseer local commit"]);
        // Uncommitted file — simulates the leaked src/disk_health.rs.
        fs::write(
            rp.join(&self.dirty_file),
            "// uncommitted work in caller checkout\n",
        )
        .unwrap();
    }

    fn base_ref(&self) -> String {
        format!(
            "origin/{}",
            self.default_branch
                .as_deref()
                .expect("base_ref requires an origin")
        )
    }

    /// SHA of the caller's unrelated local commit (the thing that must NOT leak).
    fn caller_local_commit(&self) -> String {
        git_stdout(&self.repo_path, &["rev-parse", "HEAD"])
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

/// Common env for a NEW-task (no EXISTING_BRANCH / PR_NUMBER) run. `iso_home`
/// and `xdg` are throwaway temp dirs so the recipe never touches the real HOME
/// or `/tmp`.
fn new_task_env(fix: &GitFixture, home: &Path, xdg: &Path) -> HashMap<&'static str, String> {
    let mut env = HashMap::new();
    env.insert("REPO_PATH", fix.repo_path.to_string_lossy().into_owned());
    env.insert("BRANCH_PREFIX", "feat".to_owned());
    env.insert("ISSUE_NUMBER", "858".to_owned());
    env.insert(
        "TASK_DESCRIPTION",
        "resolve issue 858 clean task worktree baseline".to_owned(),
    );
    env.insert("HOME", home.to_string_lossy().into_owned());
    env.insert("XDG_RUNTIME_DIR", xdg.to_string_lossy().into_owned());
    env
}

fn existing_branch_env(fix: &GitFixture, home: &Path, xdg: &Path) -> HashMap<&'static str, String> {
    let mut env = new_task_env(fix, home, xdg);
    env.insert("EXISTING_BRANCH", fix.caller_branch.clone());
    env
}

// ===========================================================================
// workflow-worktree.yaml — regression guards (already remote-only)
// ===========================================================================

/// GUARD: with origin/main present, a dirty non-main caller yields a clean,
/// dedicated worktree branched off origin/main.
#[test]
fn workflow_worktree_clean_baseline_from_dirty_caller() {
    let fix = GitFixture::with_origin("main");
    let home = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let env = new_task_env(&fix, home.path(), xdg.path());

    let r = run_bash(&workflow_step04_body(), &env, &fix.repo_path);
    assert_eq!(
        r.status, 0,
        "step-04 exited {}\nstderr:\n{}",
        r.status, r.stderr
    );

    let json = parse_json(&r.stdout);
    let wp = json["worktree_path"].as_str().expect("worktree_path");
    let worktree = PathBuf::from(wp);

    // I3: dedicated directory, never the caller checkout.
    assert_ne!(
        fs::canonicalize(&worktree).unwrap(),
        fs::canonicalize(&fix.repo_path).unwrap(),
        "worktree must be dedicated, not the caller repo_path"
    );
    assert_eq!(json["base_ref"].as_str().unwrap(), "origin/main");

    // I1: no caller commits leaked past the base ref.
    assert_eq!(
        rev_count(&worktree, "origin/main..HEAD"),
        0,
        "created worktree must contain zero commits ahead of origin/main"
    );
    // I2: clean working tree (no leaked uncommitted files).
    assert!(
        git_stdout(&worktree, &["status", "--porcelain"]).is_empty(),
        "created worktree working tree must be clean"
    );
    // The caller's unrelated commit must not be reachable from the worktree.
    let leaked = amplihack_git::command()
        .args([
            "merge-base",
            "--is-ancestor",
            &fix.caller_local_commit(),
            "HEAD",
        ])
        .current_dir(&worktree)
        .status()
        .expect("spawn git merge-base");
    assert!(
        !leaked.success(),
        "caller's unrelated local commit must NOT be an ancestor of the worktree HEAD"
    );
}

/// GUARD: when the origin default branch is `master` (no `main`),
/// workflow-worktree resolves the remote base ref (origin/master) — it does not
/// fall back to the caller's HEAD.
#[test]
fn workflow_worktree_master_only_origin_uses_remote_base() {
    let fix = GitFixture::with_origin("master");
    let home = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let env = new_task_env(&fix, home.path(), xdg.path());

    let r = run_bash(&workflow_step04_body(), &env, &fix.repo_path);
    assert_eq!(
        r.status, 0,
        "step-04 exited {}\nstderr:\n{}",
        r.status, r.stderr
    );

    let json = parse_json(&r.stdout);
    assert_eq!(json["base_ref"].as_str().unwrap(), "origin/master");
    let worktree = PathBuf::from(json["worktree_path"].as_str().unwrap());
    assert_eq!(
        rev_count(&worktree, "origin/master..HEAD"),
        0,
        "worktree must be branched cleanly off origin/master"
    );
}

/// RED (Vector B): when an existing target branch is already checked out in
/// the caller repository, workflow-worktree must fail closed instead of using
/// REPO_PATH as the task worktree and inheriting caller-local state.
#[test]
fn workflow_existing_branch_in_caller_checkout_fails_closed() {
    let fix = GitFixture::with_origin("main");
    let home = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let env = existing_branch_env(&fix, home.path(), xdg.path());

    let r = run_bash(&workflow_step04_body(), &env, &fix.repo_path);

    assert_ne!(
        r.status, 0,
        "workflow existing-branch path must not succeed by reusing caller repo_path; stdout:\n{}",
        r.stdout
    );
    assert!(
        r.stderr.contains("refusing to use the caller checkout") && r.stderr.contains("issue #858"),
        "failure must explain the #858 caller-checkout refusal; stderr:\n{}",
        r.stderr
    );
    assert!(
        r.stdout.trim().is_empty(),
        "failing path must not emit a worktree_setup JSON object downstream could trust; stdout:\n{}",
        r.stdout
    );
    assert!(
        worktree_under(&fix.repo_path, &fix.repo_path.join("worktrees")).is_none(),
        "refusal path must not create a replacement worktree while the caller checkout owns the branch"
    );
}

// ===========================================================================
// consensus-issue-worktree.yaml — clean-baseline invariant
// ===========================================================================

/// GUARD: with origin/main present, the consensus worktree step branches off
/// origin/main and does not inherit caller commits.
#[test]
fn consensus_clean_baseline_from_dirty_caller() {
    let fix = GitFixture::with_origin("main");
    let home = TempDir::new().unwrap();
    let worktree_dir = TempDir::new().unwrap();

    let mut env = HashMap::new();
    env.insert(
        "TASK_DESCRIPTION",
        "resolve issue 858 clean baseline consensus".to_owned(),
    );
    env.insert(
        "WORKTREE_DIR",
        worktree_dir.path().to_string_lossy().into_owned(),
    );
    env.insert("HOME", home.path().to_string_lossy().into_owned());

    let r = run_bash(&consensus_step3_body(), &env, &fix.repo_path);
    // The consensus main path emits no stdout JSON (the agent does); locate the
    // worktree it created under WORKTREE_DIR.
    let worktree = worktree_under(&fix.repo_path, worktree_dir.path()).unwrap_or_else(|| {
        panic!(
            "consensus step created no dedicated worktree under WORKTREE_DIR\nstatus={}\nstderr:\n{}",
            r.status, r.stderr
        )
    });

    // I3: dedicated directory.
    assert_ne!(
        fs::canonicalize(&worktree).unwrap(),
        fs::canonicalize(&fix.repo_path).unwrap(),
    );
    // I1: no caller commits ahead of origin/main.
    assert_eq!(
        rev_count(&worktree, "origin/main..HEAD"),
        0,
        "consensus worktree must be branched cleanly off origin/main"
    );
}

/// RED (Vector A): with an origin whose default branch is `master` (no `main`),
/// the CURRENT consensus recipe falls back to `BASE_REF="HEAD"` and branches the
/// worktree off the caller's dirty feature-branch tip — leaking the caller's
/// unrelated local commit. After the fix (remote-only `resolve_base_ref`) the
/// worktree must be branched off origin/master with an empty `<base>..HEAD`.
#[test]
fn consensus_master_only_origin_does_not_inherit_caller_commits() {
    let fix = GitFixture::with_origin("master");
    let home = TempDir::new().unwrap();
    let worktree_dir = TempDir::new().unwrap();

    let mut env = HashMap::new();
    env.insert(
        "TASK_DESCRIPTION",
        "resolve issue 858 master only consensus".to_owned(),
    );
    env.insert(
        "WORKTREE_DIR",
        worktree_dir.path().to_string_lossy().into_owned(),
    );
    env.insert("HOME", home.path().to_string_lossy().into_owned());

    let r = run_bash(&consensus_step3_body(), &env, &fix.repo_path);
    let worktree = worktree_under(&fix.repo_path, worktree_dir.path()).unwrap_or_else(|| {
        panic!(
            "consensus step created no worktree under WORKTREE_DIR\nstatus={}\nstderr:\n{}",
            r.status, r.stderr
        )
    });

    // The heart of #858 Vector A: the caller's unrelated local commit must not
    // be baked into the recipe's worktree branch.
    assert_eq!(
        rev_count(&worktree, &format!("{}..HEAD", fix.base_ref())),
        0,
        "consensus worktree inherited caller commits ahead of {} \
         (Vector A: BASE_REF=\"HEAD\" fallback). stderr:\n{}",
        fix.base_ref(),
        r.stderr
    );
    let leaked = amplihack_git::command()
        .args([
            "merge-base",
            "--is-ancestor",
            &fix.caller_local_commit(),
            "HEAD",
        ])
        .current_dir(&worktree)
        .status()
        .expect("spawn git merge-base");
    assert!(
        !leaked.success(),
        "caller's unrelated local commit leaked into the consensus worktree HEAD"
    );
}

/// RED (Vector A / fail-closed): with NO origin remote, the CURRENT consensus
/// recipe silently uses `BASE_REF="HEAD"` and creates the worktree off the
/// caller's dirty tip. Per the design (§Behavior 1, §Security Notes) setup must
/// instead **fail closed and visibly** when no remote base ref resolves, rather
/// than baking in caller state.
#[test]
fn consensus_no_origin_fails_closed_instead_of_head_fallback() {
    let fix = GitFixture::without_origin();
    let home = TempDir::new().unwrap();
    let worktree_dir = TempDir::new().unwrap();

    let mut env = HashMap::new();
    env.insert(
        "TASK_DESCRIPTION",
        "resolve issue 858 no origin consensus".to_owned(),
    );
    env.insert(
        "WORKTREE_DIR",
        worktree_dir.path().to_string_lossy().into_owned(),
    );
    env.insert("HOME", home.path().to_string_lossy().into_owned());

    let r = run_bash(&consensus_step3_body(), &env, &fix.repo_path);

    let failed_visibly = r.status != 0
        && (r.stderr.to_lowercase().contains("base ref")
            || r.stderr.to_lowercase().contains("no supported remote"));
    let created = worktree_under(&fix.repo_path, worktree_dir.path()).is_some();

    assert!(
        failed_visibly && !created,
        "with no remote base ref, setup must fail closed and create no worktree \
         off caller HEAD (Vector A). Got status={}, worktree_created={}\nstderr:\n{}",
        r.status,
        created,
        r.stderr
    );
}

// ===========================================================================
// Cross-recipe invariant
// ===========================================================================

/// GUARD: for a plain new-task run (no EXISTING_BRANCH / PR_NUMBER), neither
/// recipe may operate directly in the caller's repo_path (Vector B). Both must
/// produce a dedicated worktree directory.
#[test]
fn new_task_never_uses_caller_checkout_as_worktree() {
    // workflow-worktree
    {
        let fix = GitFixture::with_origin("main");
        let home = TempDir::new().unwrap();
        let xdg = TempDir::new().unwrap();
        let env = new_task_env(&fix, home.path(), xdg.path());
        let r = run_bash(&workflow_step04_body(), &env, &fix.repo_path);
        assert_eq!(r.status, 0, "workflow step-04 stderr:\n{}", r.stderr);
        let wp = parse_json(&r.stdout)["worktree_path"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(
            fs::canonicalize(&wp).unwrap(),
            fs::canonicalize(&fix.repo_path).unwrap(),
            "workflow-worktree must not reuse the caller repo_path as the worktree"
        );
    }
    // consensus
    {
        let fix = GitFixture::with_origin("main");
        let home = TempDir::new().unwrap();
        let worktree_dir = TempDir::new().unwrap();
        let mut env = HashMap::new();
        env.insert("TASK_DESCRIPTION", "resolve issue 858 vector b".to_owned());
        env.insert(
            "WORKTREE_DIR",
            worktree_dir.path().to_string_lossy().into_owned(),
        );
        env.insert("HOME", home.path().to_string_lossy().into_owned());
        run_bash(&consensus_step3_body(), &env, &fix.repo_path);
        let worktree = worktree_under(&fix.repo_path, worktree_dir.path())
            .expect("consensus must create a dedicated worktree under WORKTREE_DIR");
        assert_ne!(
            fs::canonicalize(&worktree).unwrap(),
            fs::canonicalize(&fix.repo_path).unwrap(),
            "consensus must not reuse the caller checkout as the worktree"
        );
    }
}
