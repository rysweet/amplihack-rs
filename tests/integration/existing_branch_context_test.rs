//! Integration tests for issue #342 — targeting an existing PR branch via context.
//!
//! These tests are written FIRST (TDD red), against the current `default-workflow.yaml`
//! and `consensus-workflow.yaml`. They MUST fail until step 9 implementation lands:
//!
//!   * `default-workflow.yaml` declares `existing_branch` and `pr_number` context vars
//!     and `step-04-setup-worktree` skips branch creation when `existing_branch` is set.
//!   * `consensus-workflow.yaml` mirrors the same behaviour in `step3-setup-worktree`.
//!   * `smart-orchestrator.yaml` declares both vars so they propagate to sub-recipes.
//!
//! Test strategy (mirrors `amplifier-bundle/tools/test_default_workflow_fixes.py`):
//!   * Parse the recipe YAML with `serde_yaml` to extract the `command:` block of the
//!     worktree-setup step (or the prompt body for the consensus agent step).
//!   * Drive that block as a `bash -c` subprocess against a real tempdir git repo
//!     with a bare local origin — no network, no mocking of git.
//!   * Mock `gh` via a PATH shim for the `pr_number` resolution path.
//!   * Cover **6 functional cases** + **7 security cases** as enumerated in the
//!     design spec (D2/D4 in EXISTING_BRANCH_CONTEXT.md §7).
//!
//! Stdout contract preserved: `{"worktree_path","branch_name","created"}` JSON only;
//! all human-readable lines (INFO/WARNING/ERROR) go to stderr.

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

fn default_workflow_yaml() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/default-workflow.yaml")
}

fn consensus_workflow_yaml() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/consensus-workflow.yaml")
}

fn smart_orchestrator_yaml() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/smart-orchestrator.yaml")
}

// ---------------------------------------------------------------------------
// Recipe parsing helpers
// ---------------------------------------------------------------------------

/// Load a recipe YAML and return the parsed serde_yaml::Value.
fn load_recipe(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {} as YAML: {e}", path.display()))
}

/// Find a step by id under `steps:` and return its `command:` body.
/// For agent-typed steps without a `command:` field, returns the `prompt:` body.
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

/// Return the recipe's `context:` map.
fn context_keys(recipe: &Value) -> Vec<String> {
    let ctx = recipe
        .get("context")
        .and_then(Value::as_mapping)
        .expect("recipe must have a top-level 'context' mapping");
    ctx.keys()
        .filter_map(|k| k.as_str().map(str::to_owned))
        .collect()
}

// ---------------------------------------------------------------------------
// Git fixture
// ---------------------------------------------------------------------------

struct GitFixture {
    _origin: TempDir,
    _repo: TempDir,
    repo_path: PathBuf,
}

fn git(cwd: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
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
        Command::new("git")
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
        // Ensure local 'main' tracks origin/main
        let _ = Command::new("git")
            .args(["branch", "--set-upstream-to=origin/main", "main"])
            .current_dir(&rp)
            .output();

        Self {
            _origin: origin,
            _repo: repo,
            repo_path: rp,
        }
    }

    fn create_local_branch(&self, name: &str) {
        git(&self.repo_path, &["branch", name, "main"]);
    }

    fn create_remote_only_branch(&self, name: &str) {
        // Create on origin without leaving a local ref behind.
        git(&self.repo_path, &["branch", name, "main"]);
        git(&self.repo_path, &["push", "origin", name]);
        git(&self.repo_path, &["branch", "-D", name]);
        // Drop the local remote-tracking ref so the branch truly is "remote-only"
        // from the recipe's point of view until it fetches.
        let _ = Command::new("git")
            .args(["update-ref", "-d", &format!("refs/remotes/origin/{name}")])
            .current_dir(&self.repo_path)
            .output();
    }
}

// ---------------------------------------------------------------------------
// gh PATH shim (for pr_number tests)
// ---------------------------------------------------------------------------

/// Build a temp dir containing an executable `gh` shim that emits the given
/// branch name when invoked as `gh pr view <N> --json headRefName -q .headRefName`.
fn gh_shim(branch_name: &str) -> TempDir {
    let dir = TempDir::new().expect("gh shim tempdir");
    let script = format!(
        "#!/usr/bin/env bash\n\
         # Test shim — only the headRefName lookup is supported.\n\
         if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then\n\
           printf '%s' '{branch_name}'\n\
           exit 0\n\
         fi\n\
         echo 'gh shim: unsupported invocation' >&2\n\
         exit 2\n",
    );
    let p = dir.path().join("gh");
    fs::write(&p, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&p, perm).unwrap();
    }
    dir
}

/// Build a malicious gh shim that emits an invalid/dangerous branch name.
fn gh_shim_malicious(payload: &str) -> TempDir {
    gh_shim(payload)
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
    // Start from a clean env so PATH shims are deterministic.
    cmd.env_clear();
    // Preserve a minimal baseline.
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
    // The recipe heredoc may emit leading whitespace per YAML block-scalar
    // indentation; locate the first '{'.
    let start = stdout.find('{').unwrap_or_else(|| {
        panic!("no JSON object in stdout:\n{stdout}");
    });
    let slice = &stdout[start..];
    let end = slice.rfind('}').unwrap_or_else(|| {
        panic!("no closing brace in stdout:\n{stdout}");
    });
    let candidate = &slice[..=end];
    serde_json::from_str(candidate).unwrap_or_else(|e| panic!("invalid JSON {candidate:?}: {e}"))
}

// ---------------------------------------------------------------------------
// Default-workflow context-var declaration tests
// ---------------------------------------------------------------------------

#[test]
fn default_workflow_declares_existing_branch_context_var() {
    let recipe = load_recipe(&default_workflow_yaml());
    let keys = context_keys(&recipe);
    assert!(
        keys.iter().any(|k| k == "existing_branch"),
        "default-workflow.yaml must declare 'existing_branch' in its context block. \
         Found keys: {keys:?}"
    );
}

#[test]
fn default_workflow_declares_pr_number_context_var() {
    let recipe = load_recipe(&default_workflow_yaml());
    let keys = context_keys(&recipe);
    assert!(
        keys.iter().any(|k| k == "pr_number"),
        "default-workflow.yaml must declare 'pr_number' in its context block. \
         Found keys: {keys:?}"
    );
}

#[test]
fn smart_orchestrator_declares_existing_branch_context_var() {
    let recipe = load_recipe(&smart_orchestrator_yaml());
    let keys = context_keys(&recipe);
    assert!(
        keys.iter().any(|k| k == "existing_branch"),
        "smart-orchestrator.yaml must declare 'existing_branch' so it propagates to sub-recipes. \
         Found keys: {keys:?}"
    );
}

#[test]
fn smart_orchestrator_declares_pr_number_context_var() {
    let recipe = load_recipe(&smart_orchestrator_yaml());
    let keys = context_keys(&recipe);
    assert!(
        keys.iter().any(|k| k == "pr_number"),
        "smart-orchestrator.yaml must declare 'pr_number' so it propagates to sub-recipes. \
         Found keys: {keys:?}"
    );
}

#[test]
fn consensus_workflow_declares_existing_branch_context_var() {
    let recipe = load_recipe(&consensus_workflow_yaml());
    let keys = context_keys(&recipe);
    assert!(
        keys.iter().any(|k| k == "existing_branch"),
        "consensus-workflow.yaml must declare 'existing_branch' in its context block. \
         Found keys: {keys:?}"
    );
}

#[test]
fn consensus_workflow_declares_pr_number_context_var() {
    let recipe = load_recipe(&consensus_workflow_yaml());
    let keys = context_keys(&recipe);
    assert!(
        keys.iter().any(|k| k == "pr_number"),
        "consensus-workflow.yaml must declare 'pr_number' in its context block. \
         Found keys: {keys:?}"
    );
}

// ---------------------------------------------------------------------------
// Functional cases — drive step-04-setup-worktree directly
// ---------------------------------------------------------------------------

fn step04_body() -> String {
    extract_step_body(
        &load_recipe(&default_workflow_yaml()),
        "step-04-setup-worktree",
    )
}

fn base_env(fix: &GitFixture) -> HashMap<&'static str, String> {
    let mut env = HashMap::new();
    env.insert("REPO_PATH", fix.repo_path.to_string_lossy().into_owned());
    env.insert("BRANCH_PREFIX", "feat".to_owned());
    env.insert("ISSUE_NUMBER", "342".to_owned());
    env.insert(
        "TASK_DESCRIPTION",
        "issue 342 existing branch context".to_owned(),
    );
    env.insert("EXISTING_BRANCH", String::new());
    env.insert("PR_NUMBER", String::new());
    env
}

/// Case 1 (functional): empty existing_branch — current behaviour preserved (created=true).
#[test]
fn case1_empty_existing_branch_preserves_legacy_behaviour() {
    let fix = GitFixture::new();
    let body = step04_body();
    let env = base_env(&fix);

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_eq!(
        r.status, 0,
        "step exited {} stderr=\n{}",
        r.status, r.stderr
    );
    let json = parse_json(&r.stdout);
    assert_eq!(json["created"], serde_json::Value::Bool(true));
    let bn = json["branch_name"].as_str().unwrap();
    assert!(
        bn.starts_with("feat/issue-342-"),
        "legacy slug derivation must run when existing_branch is empty; got {bn}"
    );
}

/// Case 2 (functional): existing_branch points at a local branch — reuse, created=false,
/// no `git branch -b` invocation.
#[test]
fn case2_existing_local_branch_reused_without_creating() {
    let fix = GitFixture::new();
    fix.create_local_branch("feat/pre-existing");
    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "feat/pre-existing".to_owned());
    // Surface git invocations so we can assert no `branch -b` happens.
    env.insert("GIT_TRACE", "1".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_eq!(
        r.status, 0,
        "step exited {} stderr=\n{}",
        r.status, r.stderr
    );
    let json = parse_json(&r.stdout);
    assert_eq!(
        json["branch_name"].as_str().unwrap(),
        "feat/pre-existing",
        "branch_name must equal existing_branch"
    );
    assert_eq!(json["created"], serde_json::Value::Bool(false));
    assert!(
        !r.stderr.contains("branch -b") && !r.stderr.contains("worktree add -b"),
        "must not invoke `git branch -b` or `git worktree add -b` on reuse path. \
         GIT_TRACE stderr:\n{}",
        r.stderr
    );
}

/// Case 3 (functional): existing_branch is remote-only — fetched, worktree created,
/// created=true, still no `git branch -b` (worktree resolves remote ref).
#[test]
fn case3_remote_only_branch_fetched_and_attached() {
    let fix = GitFixture::new();
    fix.create_remote_only_branch("feat/remote-only");
    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "feat/remote-only".to_owned());
    env.insert("GIT_TRACE", "1".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_eq!(
        r.status, 0,
        "step exited {} stderr=\n{}",
        r.status, r.stderr
    );
    let json = parse_json(&r.stdout);
    assert_eq!(
        json["branch_name"].as_str().unwrap(),
        "feat/remote-only",
        "branch_name must equal existing_branch"
    );
    let wp = json["worktree_path"].as_str().unwrap();
    assert!(
        Path::new(wp).is_dir(),
        "worktree path must exist on disk: {wp}"
    );
    assert!(
        !r.stderr.contains("branch -b"),
        "remote-only attach must not use `git branch -b`. stderr=\n{}",
        r.stderr
    );
}

/// Case 5 (functional): pr_number resolves to a branch via the gh shim.
#[test]
fn case5_pr_number_resolves_via_gh_shim() {
    let fix = GitFixture::new();
    fix.create_local_branch("feat/pr-resolved");
    let shim = gh_shim("feat/pr-resolved");

    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("PR_NUMBER", "342".to_owned());
    let path = format!(
        "{}:{}",
        shim.path().display(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
    );
    env.insert("PATH", path);

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_eq!(
        r.status, 0,
        "step exited {} stderr=\n{}",
        r.status, r.stderr
    );
    let json = parse_json(&r.stdout);
    assert_eq!(json["branch_name"].as_str().unwrap(), "feat/pr-resolved");
    assert_eq!(json["created"], serde_json::Value::Bool(false));
}

/// Case 6 (functional): when both vars are set, existing_branch wins and a warning
/// is emitted to stderr (precedence rule per design D1).
#[test]
fn case6_both_set_existing_branch_wins_with_warning() {
    let fix = GitFixture::new();
    fix.create_local_branch("feat/wins");
    // gh shim returns a *different* branch — must NOT be selected.
    let shim = gh_shim("feat/loses");

    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "feat/wins".to_owned());
    env.insert("PR_NUMBER", "342".to_owned());
    let path = format!(
        "{}:{}",
        shim.path().display(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
    );
    env.insert("PATH", path);

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_eq!(
        r.status, 0,
        "step exited {} stderr=\n{}",
        r.status, r.stderr
    );
    let json = parse_json(&r.stdout);
    assert_eq!(json["branch_name"].as_str().unwrap(), "feat/wins");
    let warn_lc = r.stderr.to_lowercase();
    assert!(
        warn_lc.contains("warning") && warn_lc.contains("existing_branch"),
        "must emit a precedence WARNING on stderr; got:\n{}",
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// Security cases
// ---------------------------------------------------------------------------

/// Case 4 (security): invalid branch name (`..`) must be rejected via check-ref-format.
#[test]
fn sec_case_invalid_ref_name_rejected() {
    let fix = GitFixture::new();
    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "invalid..name".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_ne!(
        r.status, 0,
        "must reject invalid ref names; stdout=\n{}",
        r.stdout
    );
    let s = r.stderr.to_lowercase();
    assert!(
        s.contains("check-ref-format") || s.contains("invalid"),
        "must reference check-ref-format or 'invalid' in error; got:\n{}",
        r.stderr
    );
}

/// Security: leading-dash branch name (flag injection) rejected.
#[test]
fn sec_case_leading_dash_rejected() {
    let fix = GitFixture::new();
    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "-rf".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_ne!(r.status, 0, "leading-dash branch name must be rejected");
}

/// Security: shell metachar in branch name rejected.
#[test]
fn sec_case_shell_metachar_rejected() {
    let fix = GitFixture::new();
    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert(
        "EXISTING_BRANCH",
        "feat/$(touch /tmp/pwn-issue-342)".to_owned(),
    );

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_ne!(r.status, 0, "shell metachars must be rejected");
    assert!(
        !Path::new("/tmp/pwn-issue-342").exists(),
        "command substitution payload must not execute"
    );
}

/// Security: path-traversal branch name rejected.
#[test]
fn sec_case_path_traversal_rejected() {
    let fix = GitFixture::new();
    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("EXISTING_BRANCH", "../../etc/passwd".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_ne!(r.status, 0, "path-traversal branch name must be rejected");
}

/// Security: PR_NUMBER non-numeric (arg injection) rejected before reaching gh.
#[test]
fn sec_case_pr_number_non_numeric_rejected() {
    let fix = GitFixture::new();
    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("PR_NUMBER", "342 --repo evil/x".to_owned());
    // Intentionally do not install a gh shim — the input must be rejected
    // BEFORE any gh invocation.

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_ne!(r.status, 0, "non-numeric PR_NUMBER must be rejected");
}

/// Security: PR_NUMBER leading-dash arg-injection rejected.
#[test]
fn sec_case_pr_number_leading_dash_rejected() {
    let fix = GitFixture::new();
    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("PR_NUMBER", "-1".to_owned());

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_ne!(
        r.status, 0,
        "negative/leading-dash PR_NUMBER must be rejected"
    );
}

/// Security: malicious `gh` response (invalid ref) is re-validated and rejected.
/// The recipe must NOT trust the upstream API response — it must run the same
/// `check-ref-format` gate on whatever `gh` emits.
#[test]
fn sec_case_malicious_gh_response_rejected() {
    let fix = GitFixture::new();
    let shim = gh_shim_malicious("../../etc/passwd");

    let body = step04_body();
    let mut env = base_env(&fix);
    env.insert("PR_NUMBER", "342".to_owned());
    let path = format!(
        "{}:{}",
        shim.path().display(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
    );
    env.insert("PATH", path);

    let r = run_bash(&body, &env, &fix.repo_path);
    assert_ne!(
        r.status, 0,
        "malicious gh response must be re-validated and rejected; stdout=\n{}",
        r.stdout
    );
}
