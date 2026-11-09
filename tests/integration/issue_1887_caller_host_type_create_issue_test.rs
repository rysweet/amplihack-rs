//! Issue #1887: `workflow-prep` honours two caller overrides.
//!
//! 1. step-02d keeps a caller-provided `remote_host_type` (`github`, `azdo`,
//!    `other`; `azure-devops` is an alias for `azdo`) and detects the host from
//!    `origin` only when the value is unset or empty. An unrecognised value
//!    prints a fixed WARN that never repeats the value and falls back to
//!    detection.
//! 2. `create_issue` set to `false`, `0`, `no` or `off` (trimmed, any case)
//!    stops step-03 from creating anything on every host. The order is:
//!    explicit `issue_number` reuse, then read-only task-text reuse, then
//!    `create_issue=false` (local tracking), then creation.
//!
//! See `docs/reference/workflow-tracking-issue-selection.md`, "Caller
//! overrides".
//!
//! Every child is `isolated("bash")` with a temporary HOME and TMPDIR,
//! `PATH=<fakebin>:/usr/bin:/bin` and fake `gh`, `az` and `git` binaries that
//! log each call and exit 7 on anything unexpected. Token variables are
//! removed. Nothing reaches GitHub, Azure DevOps, the real `~/.claude`,
//! `~/.amplihack`, the profile root or a tmux server. The step bodies are run
//! directly; step-02d's stdout is passed to step-03 as
//! `RECIPE_VAR_remote_host_type`, as the runner's `output:` does.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use serde_yaml::Value;

use crate::isolated_amplihack::isolated;

const GH_REPO_URL: &str = "https://github.com/example-org/example-repo";
const CREATED_URL: &str = "https://github.com/example-org/example-repo/issues/901";
const AZ_CREATED_URL: &str = "https://dev.azure.com/example-org/_apis/wit/workItems/501";
const AZ_SHOWN_URL: &str = "https://dev.azure.com/example-org/_apis/wit/workItems/77";

/// Fake credentials embedded in the origin URLs. Neither may appear in any
/// stdout or stderr (T3, #684).
const GH_CREDENTIAL: &str = "ghp_FAKEORIGINCREDENTIAL0123456789";
const AZ_CREDENTIAL: &str = "FAKEAZDOPATCREDENTIAL0123456789";

const TASK: &str = "Add widget support to the dashboard component";

const TOKEN_VARS: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GH_HOST",
    "GH_REPO",
    "AZURE_DEVOPS_EXT_PAT",
    "SYSTEM_ACCESSTOKEN",
    "AZDO_ASSIGNED_TO",
];

fn github_origin() -> String {
    format!("https://x-access-token:{GH_CREDENTIAL}@github.com/example-org/example-repo.git")
}

fn azdo_origin() -> String {
    format!("https://user:{AZ_CREDENTIAL}@dev.azure.com/example-org/example-proj/_git/example-repo")
}

// ---------------------------------------------------------------------------
// Recipe and helper access
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

fn recipe_path(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join(name)
}

fn helper_file() -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("tools")
        .join("workflow_issue_tracking.sh")
}

fn load_recipe(name: &str) -> Value {
    let text = fs::read_to_string(recipe_path(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn workflow_prep() -> &'static Value {
    static RECIPE: OnceLock<Value> = OnceLock::new();
    RECIPE.get_or_init(|| load_recipe("workflow-prep.yaml"))
}

fn extract_step_body(step_id: &str) -> String {
    let steps = workflow_prep()
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("workflow-prep must have a steps sequence");
    for step in steps {
        if step.get("id").and_then(Value::as_str) == Some(step_id) {
            return step
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("step {step_id} has no command"))
                .to_owned();
        }
    }
    panic!("step {step_id} not found in workflow-prep.yaml");
}

const STEP_02D: &str = "step-02d-detect-host-type";
const STEP_03: &str = "step-03-create-issue";

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// Fake `gh`. Logs every call to `$GH_LOG`.
/// - `issue view N`: prints the issue URL when N equals `GH_VIEW_ISSUE`.
/// - `api repos/.../issues/N`: an open issue when `GH_API_OPEN` equals N.
/// - `issue list`: prints `GH_LIST_URL` if set.
/// - `label create`: succeeds.
/// - `issue create`: the first `GH_CREATE_FAILS` calls print `GH_CREATE_ERR`
///   and exit 1; later calls succeed.
/// - `api rate_limit`: a GraphQL reset epoch of 0, so the wait takes its floor.
/// - anything else: exit 7.
const FAKE_GH: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
case "$1:$2" in
  issue:view)
    if [ -n "${GH_VIEW_ISSUE:-}" ] && [ "$3" = "$GH_VIEW_ISSUE" ]; then
      printf '%s/issues/%s\n' "$FAKE_REPO_URL" "$3"; exit 0
    fi
    echo "GraphQL: Could not resolve to an issue with the number of $3." >&2; exit 1 ;;
  api:repos/*)
    n="${2##*/}"
    if [ -n "${GH_API_OPEN:-}" ] && [ "$n" = "$GH_API_OPEN" ]; then
      printf 'open\tfalse\t%s/issues/%s\n' "$FAKE_REPO_URL" "$n"; exit 0
    fi
    echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
  issue:list) [ -n "${GH_LIST_URL:-}" ] && printf '%s\n' "$GH_LIST_URL"; exit 0 ;;
  label:create) exit 0 ;;
  issue:create)
    n=$(( $(cat "$GH_LOG.creates" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$GH_LOG.creates"
    if [ "$n" -le "${GH_CREATE_FAILS:-0}" ]; then echo "$GH_CREATE_ERR" >&2; exit 1; fi
    printf '%s/issues/901\n' "$FAKE_REPO_URL"; exit 0 ;;
  api:rate_limit) printf '{"resources":{"graphql":{"reset":0}}}\n'; exit 0 ;;
esac
echo "fake gh: unexpected invocation: $*" >&2
exit 7
"#;

/// Fake `az`. Logs every call to `$AZ_LOG`.
/// - `boards work-item show`: prints `AZ_SHOW_URL` if set, else fails.
/// - `account show`: prints a user name.
/// - `boards work-item create`: prints the created work item URL.
/// - anything else: exit 7.
const FAKE_AZ: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$AZ_LOG"
case "$1:$2:$3" in
  boards:work-item:show) [ -n "${AZ_SHOW_URL:-}" ] && { printf '%s\n' "$AZ_SHOW_URL"; exit 0; }; exit 1 ;;
  boards:work-item:create) printf '%s\n' "$AZ_CREATED_URL"; exit 0 ;;
  account:show:*) printf 'user@example.com\n'; exit 0 ;;
esac
echo "fake az: unexpected invocation: $*" >&2
exit 7
"#;

/// Fake `sleep`. Logs the requested seconds to `$SLEEP_LOG` and returns at
/// once, so step-03's rate-limit wait costs no time.
const FAKE_SLEEP: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$SLEEP_LOG"
exit 0
"#;

/// Fake `git`. Logs every call to `$GIT_LOG`.
const FAKE_GIT: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GIT_LOG"
case "$1:$2" in
  rev-parse:--is-inside-work-tree) [ -n "${GIT_NOT_REPO:-}" ] && exit 128; exit 0 ;;
  remote:get-url) printf '%s\n' "$GIT_REMOTE_URL"; exit 0 ;;
esac
echo "fake git: unexpected invocation: $*" >&2
exit 7
"#;

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).expect("write fake");
    let mut perms = fs::metadata(path).expect("fake metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod fake");
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

struct Sandbox {
    temp: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        for dir in ["repo", "fakebin", "home", "scratch"] {
            fs::create_dir_all(temp.path().join(dir)).expect("mkdir");
        }
        let bin = temp.path().join("fakebin");
        write_executable(&bin.join("gh"), FAKE_GH);
        write_executable(&bin.join("az"), FAKE_AZ);
        write_executable(&bin.join("git"), FAKE_GIT);
        write_executable(&bin.join("sleep"), FAKE_SLEEP);
        let sandbox = Sandbox { temp };
        sandbox.assert_fakes_shadow_real_tools();
        sandbox
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    /// `isolated("bash")` with the fakes first on PATH, HOME and TMPDIR in the
    /// sandbox, token variables removed and GITHUB_OUTPUT in the sandbox.
    fn bash(&self, script: &str) -> Command {
        let mut cmd = isolated("bash");
        cmd.arg("-c").arg(script);
        for var in TOKEN_VARS {
            cmd.env_remove(var);
        }
        cmd.env(
            "PATH",
            format!("{}:/usr/bin:/bin", self.path("fakebin").display()),
        )
        .env("HOME", self.path("home"))
        .env("TMPDIR", self.path("scratch"))
        .env("AMPLIHACK_HOME", workspace_root())
        .env("REPO_PATH", self.path("repo"))
        .env("GH_LOG", self.path("gh.log"))
        .env("AZ_LOG", self.path("az.log"))
        .env("GIT_LOG", self.path("git.log"))
        .env("SLEEP_LOG", self.path("sleep.log"))
        .env("GITHUB_OUTPUT", self.path("github_output"))
        .env("FAKE_REPO_URL", GH_REPO_URL)
        .env("AZ_CREATED_URL", AZ_CREATED_URL)
        .env("GIT_REMOTE_URL", github_origin())
        .current_dir(self.path("repo"));
        cmd
    }

    /// T1: `gh`, `az` and `git` must resolve inside the fake bin directory.
    fn assert_fakes_shadow_real_tools(&self) {
        let out = self
            .bash("command -v gh; command -v az; command -v git")
            .output()
            .expect("run command -v");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let bin = self.path("fakebin");
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 3, "command -v output: {stdout}");
        for line in lines {
            assert!(
                Path::new(line).starts_with(&bin),
                "{line} must resolve inside {}",
                bin.display()
            );
        }
    }

    /// Files left in TMPDIR whose content holds `needle` (T5).
    fn scratch_files_containing(&self, needle: &str) -> Vec<PathBuf> {
        let mut hits = Vec::new();
        let mut stack = vec![self.path("scratch")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if fs::read_to_string(&p).is_ok_and(|s| s.contains(needle)) {
                    hits.push(p);
                }
            }
        }
        hits
    }
}

struct Run {
    output: Output,
    sandbox: Sandbox,
}

impl Run {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    fn gh_log(&self) -> String {
        self.sandbox.read("gh.log")
    }

    fn az_log(&self) -> String {
        self.sandbox.read("az.log")
    }

    fn git_log(&self) -> String {
        self.sandbox.read("git.log")
    }

    fn diag(&self) -> String {
        format!(
            "exit: {:?}\nstdout:\n{}\nstderr:\n{}\ngh log:\n{}\naz log:\n{}\ngit log:\n{}",
            self.output.status.code(),
            self.stdout(),
            self.stderr(),
            self.gh_log(),
            self.az_log(),
            self.git_log()
        )
    }

    fn gh_called(&self, prefix: &str) -> bool {
        self.gh_log().lines().any(|l| l.starts_with(prefix))
    }

    fn az_called(&self, needle: &str) -> bool {
        self.az_log().lines().any(|l| l.contains(needle))
    }

    fn assert_success(&self) {
        assert!(self.output.status.success(), "must exit 0\n{}", self.diag());
    }

    /// T3: neither embedded origin credential appears in any output.
    fn assert_no_credential_leak(&self) {
        for secret in [GH_CREDENTIAL, AZ_CREDENTIAL] {
            assert!(
                !self.stdout().contains(secret) && !self.stderr().contains(secret),
                "origin credential leaked\n{}",
                self.diag()
            );
        }
    }

    fn assert_local_metadata(&self) {
        self.assert_success();
        let stdout = self.stdout();
        assert!(
            stdout
                .lines()
                .any(|l| l.starts_with("tracking_reference=local-")),
            "expected a tracking_reference=local-… line\n{}",
            self.diag()
        );
        assert!(
            stdout.contains("tracking_system=local"),
            "expected tracking_system=local\n{}",
            self.diag()
        );
    }

    /// No GitHub call that creates or changes anything, and no title search.
    fn assert_nothing_created_on_github(&self) {
        for prefix in ["issue create", "label create", "issue list"] {
            assert!(
                !self.gh_called(prefix),
                "gh {prefix} must not run\n{}",
                self.diag()
            );
        }
    }
}

fn run_step(sandbox: Sandbox, step: &str, env: &[(&str, &str)]) -> Run {
    let mut cmd = sandbox.bash(&extract_step_body(step));
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().unwrap_or_else(|e| panic!("run {step}: {e}"));
    Run { output, sandbox }
}

/// Run step-02d. `host` of `None` leaves `RECIPE_VAR_remote_host_type` unset.
fn run_02d(host: Option<&str>, origin: &str, env: &[(&str, &str)]) -> Run {
    let sandbox = Sandbox::new();
    let mut all: Vec<(&str, &str)> = vec![("GIT_REMOTE_URL", origin)];
    if let Some(h) = host {
        all.push(("RECIPE_VAR_remote_host_type", h));
    }
    all.extend_from_slice(env);
    run_step(sandbox, STEP_02D, &all)
}

fn step_03_env<'a>(
    host: &'a str,
    issue_number: &'a str,
    create_issue: Option<&'a str>,
    task: &'a str,
) -> Vec<(&'a str, &'a str)> {
    let mut env = vec![
        ("RECIPE_VAR_remote_host_type", host),
        ("RECIPE_VAR_issue_number", issue_number),
        ("TASK_DESCRIPTION", task),
        (
            "RECIPE_VAR_final_requirements",
            "Issue #1887 test requirements",
        ),
    ];
    if let Some(v) = create_issue {
        env.push(("RECIPE_VAR_create_issue", v));
    }
    env
}

fn run_03(
    host: &str,
    issue_number: &str,
    create_issue: Option<&str>,
    task: &str,
    extra: &[(&str, &str)],
) -> Run {
    let mut env = step_03_env(host, issue_number, create_issue, task);
    env.extend_from_slice(extra);
    run_step(Sandbox::new(), STEP_03, &env)
}

/// Run step-02d, then step-03 in the same sandbox with step-02d's stdout as
/// `RECIPE_VAR_remote_host_type`, as the runner does with `output:`.
struct Chain {
    detected: String,
    step_02d_stderr: String,
    run: Run,
}

fn run_chain(
    caller_host: Option<&str>,
    origin: &str,
    issue_number: &str,
    create_issue: Option<&str>,
    extra: &[(&str, &str)],
) -> Chain {
    let first = run_02d(caller_host, origin, extra);
    assert!(
        first.output.status.success(),
        "step-02d must succeed\n{}",
        first.diag()
    );
    let detected = first.stdout();
    let step_02d_stderr = first.stderr();
    first.assert_no_credential_leak();
    let sandbox = first.sandbox;
    let mut env = step_03_env(&detected, issue_number, create_issue, TASK);
    env.push(("GIT_REMOTE_URL", origin));
    env.extend_from_slice(extra);
    let run = run_step(sandbox, STEP_03, &env);
    Chain {
        detected,
        step_02d_stderr,
        run,
    }
}

/// Source the helper and run `script` under `set -euo pipefail`.
fn run_helper(script: &str, env: &[(&str, &str)]) -> Run {
    let sandbox = Sandbox::new();
    let full = format!("set -euo pipefail\n. \"$HELPER\"\n{script}\n");
    let mut cmd = sandbox.bash(&full);
    cmd.env("HELPER", helper_file());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run helper");
    Run { output, sandbox }
}

// ===========================================================================
// step-02d: caller-provided remote_host_type
// ===========================================================================

const WARN_02D: &str = "WARN: unrecognised remote_host_type ignored; detecting from origin (accepted: github|azdo|azure-devops|other)";

#[test]
fn step_02d_preserves_caller_value() {
    for (value, origin) in [
        ("github", azdo_origin()),
        ("azdo", github_origin()),
        ("other", github_origin()),
    ] {
        let run = run_02d(Some(value), &origin, &[]);
        run.assert_success();
        assert_eq!(
            run.stdout(),
            value,
            "caller value {value} must be kept\n{}",
            run.diag()
        );
        assert!(
            run.stderr().contains(&format!(
                "INFO: Remote host type: {value} (caller-provided)"
            )),
            "expected the (caller-provided) INFO line for {value}\n{}",
            run.diag()
        );
        assert!(
            run.git_log().is_empty(),
            "a valid caller value must not call git\n{}",
            run.diag()
        );
        run.assert_no_credential_leak();
    }
}

#[test]
fn step_02d_normalises_azure_devops() {
    for value in ["azure-devops", " Azure-DevOps ", "AZURE-DEVOPS"] {
        let run = run_02d(Some(value), &github_origin(), &[]);
        run.assert_success();
        assert_eq!(
            run.stdout(),
            "azdo",
            "{value:?} must map to azdo\n{}",
            run.diag()
        );
        assert!(run.git_log().is_empty(), "no git call\n{}", run.diag());
    }
}

#[test]
fn step_02d_trims_and_folds_case_of_valid_values() {
    for (value, expected) in [
        (" GitHub ", "github"),
        ("AZDO", "azdo"),
        ("\tOther\n", "other"),
    ] {
        let run = run_02d(Some(value), &azdo_origin(), &[]);
        run.assert_success();
        assert_eq!(run.stdout(), expected, "{value:?}\n{}", run.diag());
        assert!(run.git_log().is_empty(), "no git call\n{}", run.diag());
    }
}

#[test]
fn step_02d_unknown_value_warns_and_detects() {
    let run = run_02d(Some("gitlab;rm -rf"), &github_origin(), &[]);
    run.assert_success();
    assert_eq!(
        run.stdout(),
        "github",
        "must fall back to detection\n{}",
        run.diag()
    );
    let stderr = run.stderr();
    assert!(
        stderr.contains(WARN_02D),
        "expected the fixed WARN\n{}",
        run.diag()
    );
    assert!(
        !stderr.contains("gitlab;rm"),
        "raw value echoed\n{}",
        run.diag()
    );
    assert!(!stderr.contains("gitlab"), "value echoed\n{}", run.diag());
    assert!(
        run.git_log().contains("remote get-url"),
        "detection must read origin\n{}",
        run.diag()
    );
    run.assert_no_credential_leak();
}

#[test]
fn step_02d_newline_value_warns() {
    // V5: a newline cannot smuggle a valid value past the allow-list.
    let run = run_02d(Some("github\nINFO: forged"), &azdo_origin(), &[]);
    run.assert_success();
    assert_eq!(
        run.stdout(),
        "azdo",
        "must detect from origin\n{}",
        run.diag()
    );
    let stderr = run.stderr();
    assert!(stderr.contains(WARN_02D), "{}", run.diag());
    assert!(
        !stderr.lines().any(|l| l.starts_with("INFO: forged")),
        "a newline in the value must not start a new log line\n{}",
        run.diag()
    );
}

#[test]
fn step_02d_unicode_lookalike_is_not_accepted() {
    // Cyrillic 'һ' in place of 'h'.
    let run = run_02d(Some("gitһub"), &azdo_origin(), &[]);
    run.assert_success();
    assert_eq!(run.stdout(), "azdo", "{}", run.diag());
    assert!(run.stderr().contains(WARN_02D), "{}", run.diag());
}

#[test]
fn step_02d_unknown_value_never_echoed() {
    // T4: token-shaped values never reach the log.
    for value in [
        "ghp_FAKEFAKEFAKEFAKE1234567890",
        "github_pat_FAKE_1234567890abcdef",
        "abcdefghijABCDEFGHIJ0123456789",
    ] {
        let run = run_02d(Some(value), &github_origin(), &[]);
        run.assert_success();
        assert_eq!(run.stdout(), "github", "{}", run.diag());
        let stderr = run.stderr();
        assert!(
            stderr.contains(WARN_02D),
            "expected the fixed WARN for {value:?}\n{}",
            run.diag()
        );
        assert!(
            !stderr.contains(&value[..8]),
            "value leaked\n{}",
            run.diag()
        );
    }
}

#[test]
fn step_02d_not_a_repo_keeps_caller_value() {
    let run = run_02d(Some("azdo"), &github_origin(), &[("GIT_NOT_REPO", "1")]);
    run.assert_success();
    assert_eq!(run.stdout(), "azdo", "{}", run.diag());

    let fallback = run_02d(None, &github_origin(), &[("GIT_NOT_REPO", "1")]);
    fallback.assert_success();
    assert_eq!(
        fallback.stdout(),
        "other",
        "unset still falls back to other\n{}",
        fallback.diag()
    );
}

#[test]
fn step_02d_unset_detects_github() {
    let run = run_02d(None, &github_origin(), &[]);
    run.assert_success();
    assert_eq!(run.stdout(), "github", "{}", run.diag());
    run.assert_no_credential_leak();
}

#[test]
fn step_02d_unset_or_empty_output_is_unchanged() {
    // R1: with no caller value, stdout and stderr are exactly today's.
    let origins = [
        (github_origin(), "github"),
        (azdo_origin(), "azdo"),
        (
            "https://example-org.visualstudio.com/proj/_git/repo".to_owned(),
            "azdo",
        ),
        ("git@gitlab.com:example-org/repo.git".to_owned(), "other"),
    ];
    for (origin, expected) in origins {
        for host in [None, Some("")] {
            let run = run_02d(host, &origin, &[]);
            run.assert_success();
            assert_eq!(run.stdout(), expected, "{origin} {host:?}\n{}", run.diag());
            assert_eq!(
                run.stderr(),
                format!("INFO: Remote host type: {expected}\n"),
                "stderr must be unchanged for {host:?}\n{}",
                run.diag()
            );
        }
    }
}

// ===========================================================================
// step-02d -> step-03 chain
// ===========================================================================

#[test]
fn chain_other_on_github_origin_creates_nothing() {
    let chain = run_chain(Some("other"), &github_origin(), "", None, &[]);
    assert_eq!(
        chain.detected, "other",
        "step-03 must receive other\n{}",
        chain.step_02d_stderr
    );
    chain.run.assert_local_metadata();
    assert!(
        chain.run.gh_log().is_empty(),
        "remote_host_type=other must not call gh\n{}",
        chain.run.diag()
    );
    assert!(chain.run.az_log().is_empty(), "{}", chain.run.diag());
    chain.run.assert_no_credential_leak();
}

#[test]
fn chain_azure_devops_alias_with_issue_number_reuses_ab_reference() {
    let chain = run_chain(Some(" azure-devops "), &github_origin(), "55", None, &[]);
    assert_eq!(chain.detected, "azdo");
    chain.run.assert_success();
    assert_eq!(chain.run.stdout().trim(), "AB#55", "{}", chain.run.diag());
    assert!(chain.run.az_log().is_empty(), "{}", chain.run.diag());
    assert!(chain.run.gh_log().is_empty(), "{}", chain.run.diag());
}

// ===========================================================================
// create_issue=false: GitHub
// ===========================================================================

#[test]
fn create_issue_unset_on_github_still_creates() {
    // Control: the fakes reach `gh issue create` when nothing suppresses it,
    // so the negative tests below are not vacuous.
    let chain = run_chain(None, &github_origin(), "", None, &[]);
    assert_eq!(chain.detected, "github");
    chain.run.assert_success();
    assert_eq!(
        chain.run.stdout().trim(),
        CREATED_URL,
        "{}",
        chain.run.diag()
    );
    assert!(chain.run.gh_called("issue create"), "{}", chain.run.diag());
}

#[test]
fn github_create_retries_after_rate_limit_with_moved_helpers() {
    // The rate-limit helpers moved from step-03 into workflow_issue_tracking.sh
    // (#1887). Both attempts of the first try_gh_create (with and without the
    // label) hit a rate limit; step-03 must wait once and then create.
    let chain = run_chain(
        None,
        &github_origin(),
        "",
        None,
        &[
            ("GH_CREATE_FAILS", "2"),
            (
                "GH_CREATE_ERR",
                "GraphQL: API rate limit exceeded for user ID 1.",
            ),
        ],
    );
    let run = &chain.run;
    run.assert_success();
    assert_eq!(run.stdout().trim(), CREATED_URL, "{}", run.diag());
    assert_eq!(
        run.gh_log()
            .lines()
            .filter(|l| l.starts_with("issue create"))
            .count(),
        3,
        "{}",
        run.diag()
    );
    assert!(run.gh_called("api rate_limit"), "{}", run.diag());
    assert_eq!(run.sandbox.read("sleep.log").trim(), "5", "{}", run.diag());
    assert!(
        run.stderr().contains("reset window 1"),
        "expected one reset window\n{}",
        run.diag()
    );
    run.assert_no_credential_leak();
}

#[test]
fn github_create_auth_failure_is_not_retried() {
    let chain = run_chain(
        None,
        &github_origin(),
        "",
        None,
        &[
            ("GH_CREATE_FAILS", "99"),
            (
                "GH_CREATE_ERR",
                "HTTP 401: Bad credentials (rate limit headers present)",
            ),
        ],
    );
    let run = &chain.run;
    assert_eq!(run.output.status.code(), Some(1), "{}", run.diag());
    assert!(
        run.stderr()
            .contains("ERROR: GitHub issue creation failed."),
        "{}",
        run.diag()
    );
    assert_eq!(
        run.gh_log()
            .lines()
            .filter(|l| l.starts_with("issue create"))
            .count(),
        2,
        "an auth failure must not be retried\n{}",
        run.diag()
    );
    assert!(run.sandbox.read("sleep.log").is_empty(), "{}", run.diag());
}

#[test]
fn create_issue_false_on_github_creates_nothing() {
    let chain = run_chain(None, &github_origin(), "", Some("false"), &[]);
    assert_eq!(chain.detected, "github");
    let run = &chain.run;
    run.assert_local_metadata();
    run.assert_nothing_created_on_github();
    assert!(
        run.stderr()
            .contains("INFO: create_issue=false; issue creation disabled, using local tracking"),
        "expected the fixed INFO line\n{}",
        run.diag()
    );
    run.assert_no_credential_leak();
    // T5: the gate sits before the body mktemp.
    let residue = run.sandbox.scratch_files_containing(TASK);
    assert!(
        residue.is_empty(),
        "task text written to TMPDIR: {residue:?}\n{}",
        run.diag()
    );
}

#[test]
fn create_issue_false_github_with_list_match_still_skips_title_search() {
    // D4: the title search is skipped even when it would have found a match.
    let run = run_03(
        "github",
        "",
        Some("false"),
        TASK,
        &[(
            "GH_LIST_URL",
            "https://github.com/example-org/example-repo/issues/300",
        )],
    );
    run.assert_local_metadata();
    run.assert_nothing_created_on_github();
}

#[test]
fn task_text_reuse_precedes_create_issue_false() {
    // Read-only reuse of an open issue named in the task text still runs.
    let task = "Fix the widget crash, see Issue #77";
    let run = run_03("github", "", Some("false"), task, &[("GH_API_OPEN", "77")]);
    run.assert_success();
    assert_eq!(
        run.stdout().trim(),
        format!("{GH_REPO_URL}/issues/77"),
        "{}",
        run.diag()
    );
    run.assert_nothing_created_on_github();
}

#[test]
fn issue_number_wins_over_create_issue_false() {
    let run = run_03(
        "github",
        "42",
        Some("false"),
        TASK,
        &[("GH_VIEW_ISSUE", "42")],
    );
    run.assert_success();
    assert_eq!(
        run.stdout().trim(),
        format!("{GH_REPO_URL}/issues/42"),
        "{}",
        run.diag()
    );
    run.assert_nothing_created_on_github();
    assert!(
        !run.stdout().contains("tracking_system=local"),
        "a resolved issue_number emits no local metadata\n{}",
        run.diag()
    );
}

#[test]
fn issue_number_unresolvable_with_create_issue_false_falls_back_to_local_42() {
    let run = run_03("github", "42", Some("false"), TASK, &[]);
    run.assert_local_metadata();
    assert!(
        run.stdout().contains("tracking_reference=local-issue-42"),
        "{}",
        run.diag()
    );
    run.assert_nothing_created_on_github();
}

#[test]
fn create_issue_spellings_suppress() {
    for value in ["False", "0", "NO", " off ", "OFF", "no\n"] {
        let run = run_03("github", "", Some(value), TASK, &[]);
        run.assert_local_metadata();
        run.assert_nothing_created_on_github();
    }
}

#[test]
fn create_issue_other_values_keep_creating_on_github() {
    for value in ["", "true", "yes", "1", "TRUE", "falsey", "o ff", "nope"] {
        let run = run_03("github", "", Some(value), TASK, &[]);
        run.assert_success();
        assert_eq!(
            run.stdout().trim(),
            CREATED_URL,
            "create_issue={value:?} must not suppress creation\n{}",
            run.diag()
        );
    }
}

#[test]
fn create_issue_raw_value_never_echoed() {
    // D2/V7: the gate message and any warning are fixed strings.
    let secret = "ghp_FAKECREATEISSUE0123456789";
    let run = run_03("github", "", Some(secret), TASK, &[]);
    run.assert_success();
    assert!(
        !run.stdout().contains(secret) && !run.stderr().contains(secret),
        "raw create_issue value echoed\n{}",
        run.diag()
    );
}

// ===========================================================================
// create_issue=false: Azure DevOps
// ===========================================================================

#[test]
fn create_issue_unset_on_azdo_still_creates() {
    // Control for the AzDO gate.
    let run = run_03(
        "azdo",
        "",
        None,
        TASK,
        &[("GIT_REMOTE_URL", &azdo_origin())],
    );
    run.assert_success();
    assert_eq!(run.stdout().trim(), AZ_CREATED_URL, "{}", run.diag());
    assert!(run.az_called("work-item create"), "{}", run.diag());
}

#[test]
fn create_issue_false_on_azdo_creates_nothing() {
    let chain = run_chain(Some("azdo"), &azdo_origin(), "", Some("false"), &[]);
    assert_eq!(chain.detected, "azdo");
    let run = &chain.run;
    run.assert_local_metadata();
    assert!(!run.az_called("work-item create"), "{}", run.diag());
    assert!(
        !run.az_called("account show"),
        "identity lookup must be skipped\n{}",
        run.diag()
    );
    assert!(!run.az_called("signed-in-user"), "{}", run.diag());
    assert!(run.gh_log().is_empty(), "{}", run.diag());
    run.assert_no_credential_leak();
    let residue = run.sandbox.scratch_files_containing(TASK);
    assert!(
        residue.is_empty(),
        "TMPDIR residue: {residue:?}\n{}",
        run.diag()
    );
}

#[test]
fn create_issue_false_on_azdo_still_reuses_referenced_work_item() {
    let task = "Fix widget crash AB#77";
    let run = run_03(
        "azdo",
        "",
        Some("false"),
        task,
        &[
            ("GIT_REMOTE_URL", &azdo_origin()),
            ("AZ_SHOW_URL", AZ_SHOWN_URL),
        ],
    );
    run.assert_success();
    assert_eq!(run.stdout().trim(), AZ_SHOWN_URL, "{}", run.diag());
    assert!(!run.az_called("work-item create"), "{}", run.diag());
}

#[test]
fn create_issue_false_on_azdo_with_issue_number_prints_ab_reference() {
    let run = run_03(
        "azdo",
        "12",
        Some("false"),
        TASK,
        &[("GIT_REMOTE_URL", &azdo_origin())],
    );
    run.assert_success();
    assert_eq!(run.stdout().trim(), "AB#12", "{}", run.diag());
    assert!(run.az_log().is_empty(), "{}", run.diag());
}

// ===========================================================================
// Helper unit tests: issue_creation_disabled, warn_unrecognised_create_issue
// ===========================================================================

fn predicate(value: &str) -> (bool, Run) {
    let run = run_helper(
        "if issue_creation_disabled \"$V\"; then r=0; else r=1; fi\nprintf 'rc=%s\\n' \"$r\" >&2\necho after >&2",
        &[("V", value)],
    );
    let disabled = run.stderr().contains("rc=0");
    (disabled, run)
}

#[test]
fn issue_creation_disabled_accepts_false_set() {
    for value in [
        "false", "False", "FALSE", "0", "no", "NO", "off", " off ", "\tOff\n",
    ] {
        let (disabled, run) = predicate(value);
        run.assert_success();
        assert!(disabled, "{value:?} must disable creation\n{}", run.diag());
    }
}

#[test]
fn issue_creation_disabled_rejects_other_values() {
    for value in [
        "", " ", "true", "yes", "1", "falsey", "o ff", "f", "nö", "false\nx",
    ] {
        let (disabled, run) = predicate(value);
        run.assert_success();
        assert!(
            !disabled,
            "{value:?} must not disable creation\n{}",
            run.diag()
        );
    }
}

#[test]
fn issue_creation_disabled_is_pure() {
    // Never exits, never prints, never touches the network.
    for value in ["false", "true", ""] {
        let (_, run) = predicate(value);
        run.assert_success();
        assert!(
            run.stdout().is_empty(),
            "predicate printed to stdout\n{}",
            run.diag()
        );
        assert!(
            run.stderr().contains("after"),
            "predicate must not exit\n{}",
            run.diag()
        );
        let stderr = run.stderr();
        let noise: Vec<&str> = stderr
            .lines()
            .filter(|l| !l.starts_with("rc=") && *l != "after")
            .collect();
        assert!(noise.is_empty(), "predicate printed: {noise:?}");
        assert!(run.gh_log().is_empty() && run.az_log().is_empty() && run.git_log().is_empty());
    }
}

#[test]
fn unrecognised_create_issue_warns_without_echo() {
    let secret = "maybe_ghp_FAKEVALUE0123456789";
    let run = run_helper(
        "warn_unrecognised_create_issue \"$V\"\necho after >&2",
        &[("V", secret)],
    );
    run.assert_success();
    assert!(run.stdout().is_empty(), "{}", run.diag());
    assert!(
        run.stderr().contains("WARN"),
        "expected a WARN\n{}",
        run.diag()
    );
    assert!(
        !run.stderr().contains(secret),
        "value echoed\n{}",
        run.diag()
    );
    assert!(
        !run.stderr().contains("maybe"),
        "value echoed\n{}",
        run.diag()
    );
    assert!(run.stderr().contains("after"), "{}", run.diag());

    for quiet in [
        "", "true", "1", "yes", "false", "0", "no", "off", " OFF ", "True",
    ] {
        let run = run_helper("warn_unrecognised_create_issue \"$V\"", &[("V", quiet)]);
        run.assert_success();
        assert!(
            run.stdout().is_empty() && run.stderr().is_empty(),
            "{quiet:?} must not warn\n{}",
            run.diag()
        );
    }
}

// ===========================================================================
// Static checks: contexts, budget, bash 3.2
// ===========================================================================

fn context_value<'a>(recipe: &'a Value, key: &str) -> Option<&'a Value> {
    recipe.get("context").and_then(|c| c.get(key))
}

#[test]
fn recipe_contexts_declare_create_issue_empty() {
    for name in [
        "workflow-prep.yaml",
        "default-workflow.yaml",
        "smart-orchestrator.yaml",
    ] {
        let recipe = load_recipe(name);
        assert_eq!(
            context_value(&recipe, "create_issue").and_then(Value::as_str),
            Some(""),
            "{name} must declare create_issue: \"\""
        );
    }
}

#[test]
fn smart_orchestrator_declares_remote_host_type() {
    let recipe = load_recipe("smart-orchestrator.yaml");
    assert_eq!(
        context_value(&recipe, "remote_host_type").and_then(Value::as_str),
        Some(""),
        "smart-orchestrator.yaml must declare remote_host_type: \"\""
    );
}

#[test]
fn new_code_avoids_bash4_case_expansions() {
    // #1423: macOS /bin/bash 3.2 rejects ${v,,} and ${v^^}.
    let helper = fs::read_to_string(helper_file()).expect("read helper");
    for (what, text) in [
        ("step-02d", extract_step_body(STEP_02D)),
        ("step-03", extract_step_body(STEP_03)),
        ("workflow_issue_tracking.sh", helper),
    ] {
        for bad in [",,}", "^^}", "declare -l", "declare -u"] {
            assert!(!text.contains(bad), "{what} uses bash 4 syntax {bad}");
        }
    }
}

#[test]
fn step_02d_sources_no_helper() {
    // E1: the caller check stays inline.
    let body = extract_step_body(STEP_02D);
    assert!(
        body.contains("RECIPE_VAR_remote_host_type"),
        "step-02d must read the caller value"
    );
    assert!(
        !body.contains("workflow_issue_tracking.sh")
            && !body.lines().any(|l| l.trim_start().starts_with(". ")),
        "step-02d must not source a helper"
    );
}

#[test]
fn docs_describe_caller_overrides() {
    let doc = fs::read_to_string(
        workspace_root().join("docs/reference/workflow-tracking-issue-selection.md"),
    )
    .expect("read docs page");
    for needle in [
        "Caller overrides",
        "remote_host_type",
        "create_issue",
        "azure-devops",
        "#1885",
    ] {
        assert!(doc.contains(needle), "docs page must mention {needle}");
    }
}

#[test]
fn stale_helper_stops_step_03_with_a_named_cause() {
    // A helper copy that predates #1887 must stop step-03 with a message
    // naming the cause, not a bare exit 127, and must create nothing.
    for missing in [
        "_create_issue_value",
        "issue_creation_disabled",
        "warn_unrecognised_create_issue",
        "_issue_is_rate_limit",
        "_issue_is_auth",
        "_issue_wait_for_reset",
    ] {
        let sandbox = Sandbox::new();
        let tools = sandbox.path("stale/amplifier-bundle/tools");
        fs::create_dir_all(&tools).expect("mkdir stale tools");
        let current = fs::read_to_string(helper_file()).expect("read helper");
        fs::write(
            tools.join("workflow_issue_tracking.sh"),
            format!("{current}\nunset -f {missing}\n"),
        )
        .expect("write stale helper");
        fs::copy(
            helper_file().with_file_name("workflow_body_file.sh"),
            tools.join("workflow_body_file.sh"),
        )
        .expect("copy body helper");
        let stale_home = sandbox.path("stale");
        let mut env = step_03_env("github", "", Some("false"), TASK);
        env.push(("GIT_REMOTE_URL", GH_REPO_URL));
        let stale = stale_home.to_str().expect("utf-8 path").to_owned();
        env.push(("AMPLIHACK_HOME", &stale));
        let run = run_step(sandbox, STEP_03, &env);
        assert_eq!(run.output.status.code(), Some(2), "{}", run.diag());
        assert!(
            run.stderr().contains("predates #1887")
                && run.stderr().contains(&format!("missing {missing}")),
            "expected the named-cause error\n{}",
            run.diag()
        );
        run.assert_nothing_created_on_github();
    }
}

// ---------------------------------------------------------------------------
// #1885 merge-order guard
// ---------------------------------------------------------------------------

/// Calls that decide whether step-03 may create an issue: this PR's
/// `issue_creation_disabled` and #1885's `issue_creation_opted_in`. A rebase
/// of either PR onto the other must leave exactly one per host arm.
const CREATION_GATES: &[&str] = &["issue_creation_disabled \"", "issue_creation_opted_in \""];

/// The lines of step-03's second `case "$HOST_TYPE"` arm that starts with
/// `arm` (`github)` or `azdo|azure-devops)`), up to the arm's `;;`.
fn step_03_arm(arm: &str) -> Vec<String> {
    let body = extract_step_body(STEP_03);
    let lines: Vec<&str> = body.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == arm)
        .unwrap_or_else(|| panic!("step-03 has no `{arm}` arm on a line of its own"));
    lines[start + 1..]
        .iter()
        .take_while(|l| l.trim() != ";;")
        .map(|l| (*l).to_owned())
        .collect()
}

/// Lines of `arm` that call a creation gate (the version guard's `for f in`
/// list names the function without calling it and is outside the arm).
fn gate_lines(arm: &[String]) -> Vec<usize> {
    arm.iter()
        .enumerate()
        .filter(|(_, l)| CREATION_GATES.iter().any(|g| l.contains(g)))
        .map(|(i, _)| i)
        .collect()
}

/// Exactly one creation gate on the GitHub arm and one on the Azure DevOps
/// arm, each before that arm's first network write. This fails if a merge
/// of #1885 and #1896 keeps both gates or drops both, whichever merges
/// second (owner of that rebase: see the PR body and
/// `docs/reference/workflow-tracking-issue-selection.md`).
#[test]
fn step_03_has_exactly_one_creation_gate_per_host() {
    for (arm, first_write) in [
        ("github)", "gh issue list"),
        ("azdo|azure-devops)", "az boards work-item create"),
    ] {
        let lines = step_03_arm(arm);
        let gates = gate_lines(&lines);
        assert_eq!(
            gates.len(),
            1,
            "step-03 `{arm}` must have exactly one creation gate ({CREATION_GATES:?}); found {}",
            gates.len()
        );
        let write = lines
            .iter()
            .position(|l| l.contains(first_write))
            .unwrap_or_else(|| panic!("`{arm}` arm has no `{first_write}`"));
        assert!(
            gates[0] < write,
            "the `{arm}` gate must come before `{first_write}`"
        );
    }
}
