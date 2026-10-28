//! Integration tests for issue #1595: how `workflow-prep` step-03 chooses the
//! tracking issue on a GitHub repository.
//!
//! Before the fix, step-03 ignored `issue_number` on GitHub and reused the
//! first `#N` anywhere in the task description through `gh issue view`, which
//! also succeeds for pull requests and closed issues.
//!
//! The contract under test (see
//! `docs/reference/workflow-tracking-issue-selection.md`):
//!
//! 1. An explicit `issue_number` wins on every host. On GitHub it is resolved
//!    with `gh issue view N`; if that fails, step-03 emits local metadata for N
//!    rather than searching or creating.
//! 2. Otherwise a number from the task text is reused only when
//!    `gh api repos/{owner}/{repo}/issues/N` reports an open issue (not a pull
//!    request) whose URL ends in `/issues/N`. Marked references
//!    (`Issue #N`, `Fixes #N`, ...) are tried before bare `#N`; `PR #N`,
//!    `owner/repo#N` and `AB#N` are never looked up; at most five lookups.
//! 3. Otherwise the existing title search and `gh issue create` path runs.
//!
//! Every test runs the shipped step bodies (or sources the shipped helper)
//! against a fake `gh` and a fake `git`. The PATH puts the fakes first, the
//! environment is cleared, and HOME points at a temporary directory, so no
//! test can reach the real GitHub.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use serde_yaml::Value;

const REPO_URL: &str = "https://github.com/example-org/example-repo";
const CREATED_URL: &str = "https://github.com/example-org/example-repo/issues/901";

// ---------------------------------------------------------------------------
// Helpers (minimal copies of the issue-684 harness)
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

fn recipe_file() -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-prep.yaml")
}

fn helper_file() -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("tools")
        .join("workflow_issue_tracking.sh")
}

fn workflow_prep() -> &'static Value {
    static RECIPE: OnceLock<Value> = OnceLock::new();
    RECIPE.get_or_init(|| {
        let text = fs::read_to_string(recipe_file()).expect("read workflow-prep.yaml");
        serde_yaml::from_str(&text).expect("parse workflow-prep.yaml")
    })
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

fn write_executable(path: &Path, content: &str) {
    let mut file = fs::File::create(path).expect("create shim");
    file.write_all(content.as_bytes()).expect("write shim");
    drop(file);
    let mut perms = fs::metadata(path).expect("shim metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod shim");
}

/// Fake `gh`. Every call is logged to `$GH_LOG`, one line per call.
///
/// - `api repos/{owner}/{repo}/issues/N`: answered from `GH_API_<N>`:
///   `open`, `closed`, `pr` (an open pull request), `transferred` (open, but
///   its URL points at another repository), `nohttps` (open, but its URL has
///   no `https://` scheme), `garbage` or `timeout` (prints a token-like error
///   and exits 124, as `timeout 30` does). Unset gives a 404.
/// - `issue view N`: succeeds, as the real `gh` does, for any N whose
///   `GH_API_<N>` is set, including closed issues and pull requests, or when N
///   equals `GH_VIEW_ISSUE`. `GH_VIEW_URL` overrides the URL printed.
///   `GH_VIEW_EXIT` makes it print a token-like error and exit with that code
///   (a timeout, rate limit or auth failure).
/// - `issue list`: prints `GH_LIST_URL` if set.
/// - `label create`: succeeds.
/// - `issue create`: prints the issue #901 URL.
/// - anything else: exit 7.
const FAKE_GH: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
lookup_state() {
  case "$1" in ''|*[!0-9]*) state=""; return ;; esac
  eval "state=\${GH_API_$1:-}"
}
case "$1:$2" in
  api:repos/*)
    n="${2##*/}"
    case "$2" in */issues/"$n") ;; *) echo "fake gh: unexpected api path $2" >&2; exit 7 ;; esac
    lookup_state "$n"
    case "$state" in
      open) printf 'open\tfalse\t%s/issues/%s\n' "$FAKE_REPO_URL" "$n"; exit 0 ;;
      closed) printf 'closed\tfalse\t%s/issues/%s\n' "$FAKE_REPO_URL" "$n"; exit 0 ;;
      pr) printf 'open\ttrue\t%s/pull/%s\n' "$FAKE_REPO_URL" "$n"; exit 0 ;;
      nohttps) printf 'open\tfalse\tgithub.com/example-org/example-repo/issues/%s\n' "$n"; exit 0 ;;
      transferred) printf 'open\tfalse\thttps://github.com/other-org/other-repo/issues/%s\n' "$((n + 1000))"; exit 0 ;;
      garbage) printf 'not-a-tsv-row\n'; exit 0 ;;
      timeout) echo "gh: request timed out for token ghp_FAKEFAKEFAKEFAKE" >&2; exit 124 ;;
      *) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
    esac
    ;;
  issue:view)
    if [ -n "${GH_VIEW_EXIT:-}" ]; then echo "gh: auth failed for token ghp_FAKEFAKEFAKEFAKE" >&2; exit "$GH_VIEW_EXIT"; fi
    lookup_state "$3"
    if [ -n "$state" ] || { [ -n "${GH_VIEW_ISSUE:-}" ] && [ "$GH_VIEW_ISSUE" = "$3" ]; }; then
      if [ -n "${GH_VIEW_URL:-}" ]; then printf '%s\n' "$GH_VIEW_URL"
      elif [ "$state" = "pr" ]; then printf '%s/pull/%s\n' "$FAKE_REPO_URL" "$3"
      else printf '%s/issues/%s\n' "$FAKE_REPO_URL" "$3"; fi
      exit 0
    fi
    echo "GraphQL: Could not resolve to an issue or pull request with the number of $3." >&2
    exit 1
    ;;
  issue:list) [ -n "${GH_LIST_URL:-}" ] && printf '%s\n' "$GH_LIST_URL"; exit 0 ;;
  label:create) exit 0 ;;
  issue:create) printf '%s/issues/901\n' "$FAKE_REPO_URL"; exit 0 ;;
esac
echo "fake gh: unexpected invocation: $*" >&2
exit 7
"#;

const FAKE_GIT: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GIT_LOG"
case "$1:$2" in
  rev-parse:--is-inside-work-tree) [ -n "${GIT_NOT_REPO:-}" ] && exit 128; exit 0 ;;
  remote:get-url) printf '%s\n' "${GIT_REMOTE_URL:-https://github.com/example-org/example-repo.git}"; exit 0 ;;
esac
echo "fake git: unexpected invocation: $*" >&2
exit 2
"#;

const FAKE_AZ: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$AZ_LOG"
echo "fake az: unexpected invocation: $*" >&2
exit 7
"#;

struct Sandbox {
    temp: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        for dir in ["repo", "bin", "home"] {
            fs::create_dir_all(temp.path().join(dir)).expect("mkdir");
        }
        let bin = temp.path().join("bin");
        write_executable(&bin.join("gh"), FAKE_GH);
        write_executable(&bin.join("git"), FAKE_GIT);
        write_executable(&bin.join("az"), FAKE_AZ);
        Sandbox { temp }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    /// A bash command with a cleared environment and the fakes first on PATH.
    /// The system directories supply timeout, sed, grep, awk, tr and sha256sum;
    /// the fake `gh` always shadows any real one.
    fn bash(&self, script: &str) -> Command {
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(script)
            .env_clear()
            .env(
                "PATH",
                format!(
                    "{}:/usr/local/bin:/usr/bin:/bin",
                    self.path("bin").display()
                ),
            )
            .env("HOME", self.path("home"))
            .env("AMPLIHACK_HOME", workspace_root())
            .env("REPO_PATH", self.path("repo"))
            .env("GH_LOG", self.path("gh.log"))
            .env("GIT_LOG", self.path("git.log"))
            .env("AZ_LOG", self.path("az.log"))
            .env("FAKE_REPO_URL", REPO_URL)
            .current_dir(self.path("repo"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
}

struct Step03Run {
    output: Output,
    gh_log: String,
    az_log: String,
    sandbox: Sandbox,
}

impl Step03Run {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    fn diag(&self) -> String {
        format!(
            "exit: {:?}\nstdout:\n{}\nstderr:\n{}\ngh log:\n{}",
            self.output.status.code(),
            self.stdout(),
            self.stderr(),
            self.gh_log
        )
    }

    /// The `gh api` issue lookups, in call order.
    fn api_lookups(&self) -> Vec<&str> {
        self.gh_log
            .lines()
            .filter(|l| l.starts_with("api ") && l.contains("/issues/"))
            .collect()
    }

    fn looked_up(&self, n: u32) -> bool {
        let suffix = format!("/issues/{n}");
        self.api_lookups().iter().any(|l| {
            l.split_whitespace()
                .nth(1)
                .is_some_and(|p| p.ends_with(&suffix))
        })
    }

    fn called(&self, prefix: &str) -> bool {
        self.gh_log.lines().any(|l| l.starts_with(prefix))
    }

    fn assert_success(&self) {
        assert!(
            self.output.status.success(),
            "step-03 must succeed\n{}",
            self.diag()
        );
    }

    fn assert_selected(&self, n: u32) {
        self.assert_success();
        assert_eq!(
            self.stdout().trim(),
            format!("{REPO_URL}/issues/{n}"),
            "step-03 must select issue #{n}\n{}",
            self.diag()
        );
        assert!(
            !self.called("issue create"),
            "selecting #{n} must not create an issue\n{}",
            self.diag()
        );
    }

    fn assert_created(&self) {
        self.assert_success();
        assert_eq!(
            self.stdout().trim(),
            CREATED_URL,
            "step-03 must fall through to a new issue\n{}",
            self.diag()
        );
        assert!(
            self.called("issue create"),
            "gh issue create must run\n{}",
            self.diag()
        );
    }
}

fn run_step_03(host: &str, issue_number: &str, task: &str, env: &[(&str, &str)]) -> Step03Run {
    let sandbox = Sandbox::new();
    let mut cmd = sandbox.bash(&extract_step_body("step-03-create-issue"));
    cmd.env("RECIPE_VAR_remote_host_type", host)
        .env("RECIPE_VAR_issue_number", issue_number)
        .env("TASK_DESCRIPTION", task)
        .env(
            "RECIPE_VAR_final_requirements",
            "Issue #1595 test requirements",
        );
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run step-03-create-issue");
    Step03Run {
        output,
        gh_log: sandbox.read("gh.log"),
        az_log: sandbox.read("az.log"),
        sandbox,
    }
}

fn run_github(issue_number: &str, task: &str, env: &[(&str, &str)]) -> Step03Run {
    run_step_03("github", issue_number, task, env)
}

/// Run step-03b on step-03's stdout and return the `issue_number` it prints.
fn step_03b_issue_number(run: &Step03Run, task: &str) -> String {
    let sandbox = Sandbox::new();
    let output = sandbox
        .bash(&extract_step_body("step-03b-extract-issue-number"))
        .env("RECIPE_VAR_issue_creation", run.stdout())
        .env("TASK_DESCRIPTION", task)
        .output()
        .expect("run step-03b-extract-issue-number");
    assert!(
        output.status.success(),
        "step-03b must succeed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Source the helper and run `call` under `set -euo pipefail`, as step-03 does.
fn run_helper(call: &str, issue_number: &str, task: &str, env: &[(&str, &str)]) -> Step03Run {
    let sandbox = Sandbox::new();
    let script = format!(
        "set -euo pipefail\n. \"$HELPER\"\nEXISTING_ISSUE_NUMBER=\"$ISSUE_NUMBER\"\nTASK_DESC=\"$TASK_DESCRIPTION\"\n{call}\n"
    );
    let mut cmd = sandbox.bash(&script);
    cmd.env("HELPER", helper_file())
        .env("ISSUE_NUMBER", issue_number)
        .env("TASK_DESCRIPTION", task);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run helper");
    Step03Run {
        output,
        gh_log: sandbox.read("gh.log"),
        az_log: sandbox.read("az.log"),
        sandbox,
    }
}

/// Candidate numbers only (the first field of each `N kind` line).
fn candidates(task: &str) -> Vec<String> {
    candidate_lines(task)
        .iter()
        .map(|l| l.split_whitespace().next().unwrap_or_default().to_owned())
        .collect()
}

/// Raw `N kind` candidate lines.
fn candidate_lines(task: &str) -> Vec<String> {
    let run = run_helper("_tracking_issue_candidates", "", task, &[]);
    assert!(
        run.output.status.success(),
        "_tracking_issue_candidates must return 0 for {task:?}\n{}",
        run.diag()
    );
    assert!(
        run.gh_log.is_empty(),
        "_tracking_issue_candidates must not call gh\n{}",
        run.diag()
    );
    run.stdout().lines().map(str::to_owned).collect()
}

// ===========================================================================
// R1: an explicit issue_number wins on GitHub
// ===========================================================================

/// (a) Scenario 2 from the issue: `-c issue_number=1592` with background
/// references to a merged PR and a closed issue.
#[test]
fn explicit_issue_number_wins_over_task_text_on_github() {
    let task = "Fix step-03 issue selection. See PR #1585 and #1578 for background.";
    let run = run_github(
        "1592",
        task,
        &[
            ("GH_API_1592", "open"),
            ("GH_API_1585", "pr"),
            ("GH_API_1578", "closed"),
        ],
    );
    run.assert_selected(1592);
    assert!(
        run.called("issue view 1592"),
        "explicit number must use gh issue view\n{}",
        run.diag()
    );
    assert!(
        run.api_lookups().is_empty(),
        "an explicit number must not trigger task-text lookups\n{}",
        run.diag()
    );
    assert!(
        !run.called("issue list"),
        "explicit number must not search titles\n{}",
        run.diag()
    );
    assert!(
        run.az_log.is_empty(),
        "github host must not call az\n{}",
        run.az_log
    );
}

/// (a′) One leading `#` is stripped from the explicit value.
#[test]
fn explicit_issue_number_with_leading_hash_is_accepted() {
    let run = run_github(
        "#1592",
        "Background: #1578.",
        &[("GH_API_1592", "open"), ("GH_API_1578", "closed")],
    );
    run.assert_selected(1592);
    assert!(run.called("issue view 1592"), "{}", run.diag());
    assert!(run.api_lookups().is_empty(), "{}", run.diag());
}

/// (a″, D1) The explicit lookup fails: keep the number as local metadata, do
/// not search titles and do not create an unrelated issue.
#[test]
fn explicit_issue_number_that_cannot_be_resolved_falls_back_to_local_metadata() {
    let run = run_github("1592", "Background: #1578.", &[("GH_API_1578", "closed")]);
    run.assert_success();
    let stdout = run.stdout();
    assert!(
        stdout.contains("tracking_reference=local-issue-1592")
            && stdout.lines().any(|l| l == "issue_number=1592"),
        "an unresolvable explicit number must be kept as local metadata\n{}",
        run.diag()
    );
    assert!(
        run.stderr().contains("WARN"),
        "the fallback must be visible on stderr\n{}",
        run.diag()
    );
    assert!(!run.called("issue list"), "{}", run.diag());
    assert!(!run.called("issue create"), "{}", run.diag());
    assert!(run.api_lookups().is_empty(), "{}", run.diag());
    assert_eq!(
        step_03b_issue_number(&run, "Background: #1578."),
        "local-issue-1592"
    );
}

/// (S1) When `gh issue view` itself fails (auth or rate limit: exit 1;
/// timeout: exit 124), the warning reports gh's exit code instead of blaming
/// the number, gh's stderr stays out of the log, and nothing is created.
#[test]
fn explicit_issue_number_gh_failure_reports_the_exit_code() {
    for code in ["1", "124"] {
        let run = run_github(
            "1592",
            "Add a flag.",
            &[("GH_API_1592", "open"), ("GH_VIEW_EXIT", code)],
        );
        run.assert_success();
        let stderr = run.stderr();
        assert!(
            stderr.contains(&format!(
                "WARN: could not verify issue #1592 (gh exit {code}); falling back to local tracking"
            )),
            "gh exit {code}\n{}",
            run.diag()
        );
        assert!(
            !stderr.contains("could not be resolved as an issue"),
            "a gh failure must not blame the number\n{}",
            run.diag()
        );
        assert!(
            !stderr.contains("ghp_FAKE"),
            "gh's stderr must stay discarded\n{}",
            run.diag()
        );
        let stdout = run.stdout();
        assert!(
            stdout.contains("tracking_reference=local-issue-1592")
                && stdout.lines().any(|l| l == "issue_number=1592"),
            "gh exit {code}\n{}",
            run.diag()
        );
        assert!(!run.called("issue list"), "{}", run.diag());
        assert!(!run.called("issue create"), "{}", run.diag());
        assert!(run.api_lookups().is_empty(), "{}", run.diag());
    }
}

/// (S1) gh succeeds but returns a non-issue URL: the original wording stays.
#[test]
fn explicit_issue_number_bad_url_keeps_the_resolution_warning() {
    let run = run_github(
        "1592",
        "Add a flag.",
        &[
            ("GH_VIEW_ISSUE", "1592"),
            (
                "GH_VIEW_URL",
                "github.com/example-org/example-repo/issues/1592",
            ),
        ],
    );
    run.assert_success();
    assert!(
        run.stderr()
            .contains("could not be resolved as an issue in this repository"),
        "{}",
        run.diag()
    );
    assert!(!run.stderr().contains("gh exit"), "{}", run.diag());
    assert!(
        run.stdout().contains("tracking_reference=local-issue-1592"),
        "{}",
        run.diag()
    );
}

/// (a‴, SR-5) A `/pull/N` URL from the explicit view is never passed on as
/// the tracking issue.
#[test]
fn explicit_issue_number_resolving_to_a_pull_request_url_is_not_used_as_an_issue() {
    let run = run_github(
        "1592",
        "Add a flag.",
        &[
            ("GH_VIEW_ISSUE", "1592"),
            (
                "GH_VIEW_URL",
                "https://github.com/example-org/example-repo/pull/1592",
            ),
        ],
    );
    run.assert_success();
    let stdout = run.stdout();
    assert!(
        !stdout.contains("/pull/"),
        "a pull request URL must not be printed\n{}",
        run.diag()
    );
    assert!(
        stdout.contains("tracking_reference=local-issue-1592"),
        "{}",
        run.diag()
    );
    assert!(!run.called("issue create"), "{}", run.diag());
}

/// The explicit number is the user's choice and is not filtered for state;
/// step-03c remains the gate for closed or claimed issues.
#[test]
fn explicit_closed_issue_number_is_not_filtered() {
    let run = run_github(
        "1578",
        "Reopen work on the selection bug.",
        &[("GH_API_1578", "closed")],
    );
    run.assert_selected(1578);
    assert!(run.api_lookups().is_empty(), "{}", run.diag());
}

/// (D2, SR-11) A non-numeric explicit value is ignored for selection, with a
/// fixed warning that does not echo the value.
#[test]
fn non_numeric_explicit_issue_number_is_ignored_without_echoing_it() {
    let run = run_github(
        "abc-secret-xyz",
        "Issue #1595: fix selection.",
        &[("GH_API_1595", "open")],
    );
    run.assert_selected(1595);
    assert!(
        run.looked_up(1595),
        "selection must fall back to task-text candidates\n{}",
        run.diag()
    );
    let stderr = run.stderr();
    assert!(
        stderr.contains("WARN"),
        "a non-numeric value must be warned about\n{}",
        run.diag()
    );
    assert!(
        !stderr.contains("abc-secret-xyz"),
        "the warning must not echo the value\n{}",
        run.diag()
    );
}

/// Only one leading `#` is stripped, so `##1592` is rejected.
#[test]
fn double_hash_explicit_issue_number_is_rejected() {
    let run = run_github(
        "##1592",
        "Issue #1595: fix selection.",
        &[("GH_API_1595", "open"), ("GH_API_1592", "open")],
    );
    run.assert_selected(1595);
    assert!(
        !run.called("issue view 1592"),
        "##1592 must not be read as 1592\n{}",
        run.diag()
    );
}

/// More than nine digits is rejected (SR-4).
#[test]
fn overlong_explicit_issue_number_is_rejected() {
    let run = run_github("12345678901", "Add a flag.", &[]);
    run.assert_created();
    assert!(!run.called("issue view"), "{}", run.diag());
}

// ===========================================================================
// R3: task-text candidates, validated through gh api
// ===========================================================================

/// (b) A marked open issue is chosen; the closed background issue is not.
#[test]
fn marked_open_issue_is_selected_over_closed_background_issue() {
    let run = run_github(
        "",
        "Issue #1595: fix selection. See #1578.",
        &[("GH_API_1595", "open"), ("GH_API_1578", "closed")],
    );
    run.assert_selected(1595);
    assert!(
        run.looked_up(1595),
        "candidates must be validated with gh api\n{}",
        run.diag()
    );
    assert!(
        !run.called("issue view"),
        "task-text candidates must not use gh issue view, which accepts PRs\n{}",
        run.diag()
    );
}

/// (SR-1) The candidate lookup is read-only and does not override the
/// repository or host.
#[test]
fn candidate_lookup_is_read_only_and_uses_the_current_repository() {
    let run = run_github(
        "",
        "Issue #1595: fix selection.",
        &[("GH_API_1595", "open")],
    );
    run.assert_selected(1595);
    let lookups = run.api_lookups();
    assert_eq!(lookups.len(), 1, "{}", run.diag());
    let line = lookups[0];
    assert!(
        line.starts_with("api repos/{owner}/{repo}/issues/1595"),
        "lookup must use the {{owner}}/{{repo}} placeholders\n{}",
        run.diag()
    );
    for flag in [
        " -X",
        " --method",
        " -f ",
        " -F ",
        " --field",
        " --raw-field",
        " --input",
        " --hostname",
        " --repo",
        " -R ",
    ] {
        assert!(
            !line.contains(flag),
            "lookup must not pass {flag:?}\n{}",
            run.diag()
        );
    }
}

/// (c) `Fixes #42` where #42 is a closed issue.
#[test]
fn marked_closed_issue_is_rejected_and_a_new_issue_is_created() {
    let run = run_github("", "Fixes #42", &[("GH_API_42", "closed")]);
    run.assert_created();
    assert!(run.looked_up(42), "{}", run.diag());
    assert!(!run.stdout().contains("/issues/42"), "{}", run.diag());
}

/// (d) `Fixes #42` where #42 is a pull request.
#[test]
fn marked_pull_request_is_rejected_and_a_new_issue_is_created() {
    let run = run_github("", "Fixes #42", &[("GH_API_42", "pr")]);
    run.assert_created();
    assert!(!run.stdout().contains("/pull/42"), "{}", run.diag());
}

/// A task-text candidate whose `gh api` lookup fails (here a timeout, exit
/// 124) is reported with gh's exit code, gh's stderr stays out of the log,
/// and selection falls through to creating an issue.
#[test]
fn candidate_gh_failure_reports_the_exit_code() {
    let run = run_github("", "Fixes #42", &[("GH_API_42", "timeout")]);
    run.assert_created();
    assert!(run.looked_up(42), "{}", run.diag());
    let stderr = run.stderr();
    assert!(
        stderr.contains("WARN: could not verify candidate #42 (gh exit 124)"),
        "{}",
        run.diag()
    );
    assert!(
        !stderr.contains("ghp_FAKE"),
        "gh's stderr must stay discarded\n{}",
        run.diag()
    );
}

/// (e) An incidental mention of a merged PR.
#[test]
fn incidental_pull_request_reference_leads_to_a_new_issue() {
    let run = run_github("", "Follows #1585.", &[("GH_API_1585", "pr")]);
    run.assert_created();
}

/// (f) An incidental mention of a closed issue (scenario 2 without an
/// explicit number).
#[test]
fn incidental_closed_issue_reference_leads_to_a_new_issue() {
    let run = run_github("", "Related to #1578.", &[("GH_API_1578", "closed")]);
    run.assert_created();
    assert!(!run.stdout().contains("/issues/1578"), "{}", run.diag());
}

/// (g) A marked open issue is preferred over an earlier incidental open issue.
#[test]
fn marked_open_issue_is_preferred_over_earlier_incidental_open_issue() {
    let run = run_github(
        "",
        "See #1592 first. Fixes #1595.",
        &[("GH_API_1592", "open"), ("GH_API_1595", "open")],
    );
    run.assert_selected(1595);
    let lookups = run.api_lookups();
    assert!(
        lookups.first().is_some_and(|l| l.contains("/issues/1595")),
        "the marked reference must be looked up first\n{}",
        run.diag()
    );
    assert!(
        !run.looked_up(1592),
        "lookups stop at the first accepted candidate\n{}",
        run.diag()
    );
}

/// (D8) With no marked reference, an incidental open issue is still reused.
#[test]
fn incidental_open_issue_is_reused_when_no_marked_reference_passes() {
    let run = run_github(
        "",
        "Fixes #42. Related to #1592.",
        &[("GH_API_42", "pr"), ("GH_API_1592", "open")],
    );
    run.assert_selected(1592);
    assert!(
        run.stderr().contains("selected (bare)"),
        "a bare selection must be logged\n{}",
        run.diag()
    );
}

/// The selection source is logged on stderr (SR-12).
#[test]
fn selection_source_is_logged_on_stderr() {
    let explicit = run_github("1592", "Add a flag.", &[("GH_API_1592", "open")]);
    assert!(
        explicit
            .stderr()
            .contains("tracking issue #1592 selected (explicit)"),
        "{}",
        explicit.diag()
    );
    let marked = run_github("", "Issue #1595: fix.", &[("GH_API_1595", "open")]);
    assert!(
        marked
            .stderr()
            .contains("tracking issue #1595 selected (marked)"),
        "{}",
        marked.diag()
    );
}

/// (S2) A number written both bare and marked is labelled by the branch that
/// picked it, which is the marked one because marked references come first.
#[test]
fn log_label_matches_the_kind_that_picked_the_number() {
    let run = run_github("", "See #20. Fixes #20.", &[("GH_API_20", "open")]);
    run.assert_selected(20);
    assert!(
        run.stderr()
            .contains("tracking issue #20 selected (marked)"),
        "{}",
        run.diag()
    );
    assert_eq!(run.api_lookups().len(), 1, "{}", run.diag());
    let run = run_github(
        "",
        "See #21. Fixes #20.",
        &[("GH_API_20", "closed"), ("GH_API_21", "open")],
    );
    run.assert_selected(21);
    assert!(
        run.stderr().contains("tracking issue #21 selected (bare)"),
        "{}",
        run.diag()
    );
}

/// (h) `PR #N`, `pull request #N`, `owner/repo#N` and `AB#N` are never looked up.
#[test]
fn masked_reference_forms_are_never_looked_up() {
    let task = "Follow-up to PR #1592, PR#1593, pull request #1594, pull #1596, rysweet/amplihack-rs#1597 and AB#1598.";
    let run = run_github(
        "",
        task,
        &[
            ("GH_API_1592", "open"),
            ("GH_API_1593", "open"),
            ("GH_API_1594", "open"),
            ("GH_API_1596", "open"),
            ("GH_API_1597", "open"),
            ("GH_API_1598", "open"),
        ],
    );
    run.assert_created();
    assert!(
        run.api_lookups().is_empty(),
        "masked forms must not be looked up\n{}",
        run.diag()
    );
    assert!(!run.called("issue view"), "{}", run.diag());
}

/// Hex colours and `word#N` tokens are not issue references.
#[test]
fn hex_colours_and_word_prefixed_hashes_are_not_looked_up() {
    let run = run_github(
        "",
        "Set colour #123abc and fix abc#12.",
        &[("GH_API_123", "open"), ("GH_API_12", "open")],
    );
    run.assert_created();
    assert!(run.api_lookups().is_empty(), "{}", run.diag());
}

/// (i) Scenario 1 from the issue: "runner fixes #161 and #162", where #161 is
/// a merged PR here and #162 does not exist.
#[test]
fn runner_fixes_scenario_creates_a_new_issue_and_never_uses_161() {
    let task = "Upgrade the recipe runner; runner fixes #161 and #162.";
    let run = run_github("", task, &[("GH_API_161", "pr")]);
    run.assert_created();
    assert!(run.looked_up(161) && run.looked_up(162), "{}", run.diag());
    let stdout = run.stdout();
    assert!(
        !stdout.contains("/161") && !stdout.contains("/162"),
        "{}",
        run.diag()
    );
    let number = step_03b_issue_number(&run, task);
    assert_eq!(
        number, "901",
        "the branch must be named for the new issue, not feat/issue-161"
    );
}

/// An issue transferred to another repository is rejected by the URL check.
#[test]
fn transferred_issue_is_rejected() {
    let run = run_github("", "Issue #50: tidy up.", &[("GH_API_50", "transferred")]);
    run.assert_created();
}

/// Output in an unexpected shape rejects the candidate.
#[test]
fn unexpected_api_output_rejects_the_candidate() {
    let run = run_github("", "Issue #50: tidy up.", &[("GH_API_50", "garbage")]);
    run.assert_created();
}

/// (S3) A task-text candidate whose URL lacks `https://` is rejected, the
/// same rule the explicit-number path uses.
#[test]
fn candidate_url_without_https_is_rejected() {
    let run = run_github("", "Fixes #42.", &[("GH_API_42", "nohttps")]);
    run.assert_created();
    assert!(run.looked_up(42), "{}", run.diag());
}

/// (j) No reference: the title search and create path are unchanged.
#[test]
fn task_without_references_uses_the_unchanged_search_and_create_path() {
    let run = run_github("", "Add a --json flag to recipe list.", &[]);
    run.assert_created();
    assert!(run.api_lookups().is_empty(), "{}", run.diag());
    assert!(
        run.called("issue list"),
        "the title search must still run\n{}",
        run.diag()
    );
    assert!(run.called("label create"), "{}", run.diag());
    assert!(
        !run.stderr().contains("no task-text candidate accepted"),
        "no fall-through line without candidates\n{}",
        run.diag()
    );
}

/// The title search still reuses a matching open issue when no candidate
/// passes.
#[test]
fn rejected_candidates_fall_through_to_the_title_search() {
    let found = "https://github.com/example-org/example-repo/issues/777";
    let run = run_github(
        "",
        "Follows #1585.",
        &[("GH_API_1585", "pr"), ("GH_LIST_URL", found)],
    );
    run.assert_success();
    assert_eq!(run.stdout().trim(), found, "{}", run.diag());
    assert!(!run.called("issue create"), "{}", run.diag());
    let stderr = run.stderr();
    assert!(
        stderr.contains("no task-text candidate accepted") && stderr.contains("checked: 1585)"),
        "the rejected candidates must be logged\n{}",
        run.diag()
    );
}

/// (m) At most five candidates are looked up.
#[test]
fn at_most_five_candidates_are_looked_up() {
    let run = run_github("", "See #11 #12 #13 #14 #15 #16 #17.", &[]);
    run.assert_created();
    assert_eq!(run.api_lookups().len(), 5, "{}", run.diag());
    assert!(!run.looked_up(16) && !run.looked_up(17), "{}", run.diag());
}

/// A number repeated in several forms is looked up once.
#[test]
fn duplicate_candidates_are_looked_up_once() {
    let run = run_github(
        "",
        "#20, then Fixes #20, and #20 again.",
        &[("GH_API_20", "closed")],
    );
    run.assert_created();
    assert_eq!(run.api_lookups().len(), 1, "{}", run.diag());
}

/// (s3, SR-8) Large input with many references: no SIGPIPE under pipefail
/// and still no more than five lookups. The text is 120 KiB, well over the
/// helper's 64 KiB cut but under Linux's 128 KiB limit on a single
/// environment string, which is how the recipe passes TASK_DESCRIPTION.
#[test]
fn large_task_text_does_not_trigger_sigpipe_or_extra_lookups() {
    let mut task: String = (1..=10_000).map(|i| format!("see #{i} ")).collect();
    while task.len() < 120 * 1024 {
        task.push_str("padding text ");
    }
    let run = run_github("", &task, &[]);
    assert_ne!(
        run.output.status.code(),
        Some(141),
        "SIGPIPE under pipefail\n{}",
        run.stderr()
    );
    run.assert_created();
    assert!(
        run.api_lookups().len() <= 5,
        "{} lookups",
        run.api_lookups().len()
    );
}

/// (s5) Shell syntax in the task text is never evaluated.
#[test]
fn task_text_is_never_evaluated() {
    let task = "Issue #1595 $(touch PWNED1) `touch PWNED2` ; touch PWNED3";
    let run = run_github("", task, &[("GH_API_1595", "open")]);
    run.assert_selected(1595);
    for name in ["PWNED1", "PWNED2", "PWNED3"] {
        assert!(
            !run.sandbox.path("repo").join(name).exists()
                && !run.sandbox.path("home").join(name).exists(),
            "task text must not be executed ({name} was created)"
        );
    }
}

// ===========================================================================
// R2 / R4: AzDO and local tracking keep honouring the explicit number
// ===========================================================================

/// (k1) AzDO still prints `AB#N` and calls neither gh nor az.
#[test]
fn explicit_issue_number_is_honoured_on_azdo() {
    let run = run_step_03("azdo", "763", "See #1578.", &[]);
    run.assert_success();
    assert_eq!(run.stdout().trim(), "AB#763", "{}", run.diag());
    assert!(
        run.gh_log.is_empty() && run.az_log.is_empty(),
        "{}",
        run.diag()
    );
}

/// (k2) The `other` host still gives `local-issue-N`.
#[test]
fn explicit_issue_number_is_honoured_on_other_hosts() {
    let run = run_step_03("other", "763", "See #1578.", &[]);
    run.assert_success();
    assert!(
        run.stdout().contains("tracking_reference=local-issue-763"),
        "{}",
        run.diag()
    );
    assert!(run.gh_log.is_empty(), "{}", run.diag());
}

/// (k3) A GitHub host outside a git repository still gives `local-issue-N`.
#[test]
fn explicit_issue_number_is_honoured_outside_a_git_repository() {
    let run = run_github("763", "See #1578.", &[("GIT_NOT_REPO", "1")]);
    run.assert_success();
    assert!(
        run.stdout().contains("tracking_reference=local-issue-763"),
        "{}",
        run.diag()
    );
    assert!(run.gh_log.is_empty(), "{}", run.diag());
}

/// (s2, F-1) A multi-line `issue_number` cannot add key=value lines to the
/// local metadata that step-03b parses.
#[test]
fn multiline_issue_number_cannot_inject_local_metadata_lines() {
    let run = run_step_03(
        "other",
        "x\nissue_number=9\nlocal-issue-5",
        "Add a flag.",
        &[],
    );
    run.assert_success();
    assert!(
        !run.stdout().lines().any(|l| l == "issue_number=9"),
        "issue_number must not be injected through ISSUE_NUMBER\n{}",
        run.diag()
    );
    assert!(
        !run.stdout()
            .lines()
            .any(|l| l.starts_with("tracking_reference=x")),
        "the raw value must not become the tracking reference\n{}",
        run.diag()
    );
}

// ===========================================================================
// (l) step-03b reads back the selected number
// ===========================================================================

/// Explicit number, task text, fake `gh` environment, expected read-back.
type ReadBackCase<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], &'a str);

#[test]
fn step_03b_reads_back_the_selected_issue_number() {
    let cases: [ReadBackCase; 3] = [
        (
            "1592",
            "Fix selection. See PR #1585 and #1578 for background.",
            &[("GH_API_1592", "open"), ("GH_API_1578", "closed")],
            "1592",
        ),
        (
            "",
            "Issue #1595: fix selection. See #1578.",
            &[("GH_API_1595", "open"), ("GH_API_1578", "closed")],
            "1595",
        ),
        (
            "",
            "See #1592 first. Fixes #1595.",
            &[("GH_API_1592", "open"), ("GH_API_1595", "open")],
            "1595",
        ),
    ];
    for (explicit, task, env, expected) in cases {
        let run = run_github(explicit, task, env);
        run.assert_success();
        assert_eq!(
            step_03b_issue_number(&run, task),
            expected,
            "task {task:?}\n{}",
            run.diag()
        );
    }
}

// ===========================================================================
// (u) Unit tests: the helper functions sourced directly
// ===========================================================================

#[test]
fn candidates_put_marked_references_before_bare_ones() {
    assert_eq!(candidates("Issue #1595 ... see #1578"), ["1595", "1578"]);
    assert_eq!(candidates("See #20 first. Fixes #30."), ["30", "20"]);
}

#[test]
fn candidates_recognise_every_marker_keyword_and_spacing() {
    for task in [
        "issue #5",
        "Issue#5",
        "ISSUE: #5",
        "fix #5",
        "fixes #5",
        "Fixed #5",
        "close #5",
        "closes : #5",
        "closed #5",
        "resolve #5",
        "Resolves: #5",
        "RESOLVED #5",
    ] {
        assert_eq!(
            candidates(&format!("See #3. {task}")),
            ["5", "3"],
            "marker form {task:?}"
        );
    }
}

#[test]
fn candidates_exclude_masked_forms() {
    assert!(
        candidates("PR #12, PR#13, pull request #14, pull #15, rysweet/other#16, AB#17").is_empty()
    );
    assert!(
        candidates("pr #12 and Pull Request #14").is_empty(),
        "masking is case-insensitive"
    );
}

#[test]
fn candidates_exclude_non_reference_hashes() {
    assert!(candidates("colour #123abc").is_empty());
    assert!(candidates("abc#12").is_empty(), "abc#12 must not yield 12");
    assert!(candidates("#007 #0 #12345678901").is_empty());
}

#[test]
fn candidates_accept_bare_references_at_boundaries() {
    assert_eq!(candidates("#42"), ["42"]);
    assert_eq!(candidates("(#42)."), ["42"]);
    assert_eq!(candidates("see #42, #43"), ["42", "43"]);
}

#[test]
fn candidates_are_deduplicated_and_capped_at_five() {
    assert_eq!(candidates("#20 Fixes #20 #20"), ["20"]);
    assert_eq!(
        candidates("#1 #2 #3 #4 #5 #6 #7"),
        ["1", "2", "3", "4", "5"]
    );
}

#[test]
fn candidates_carry_their_kind_and_the_first_kind_wins() {
    assert_eq!(
        candidate_lines("See #20. Fixes #20. Also #21."),
        ["20 marked", "21 bare"]
    );
}

#[test]
fn candidates_for_empty_or_reference_free_text_are_empty() {
    assert!(candidates("").is_empty());
    assert!(candidates("Add a --json flag to recipe list.").is_empty());
}

#[test]
fn candidates_treat_a_marker_split_across_lines_as_bare() {
    assert_eq!(candidates("See #8.\nFixes\n#9"), ["8", "9"]);
}

#[test]
fn open_issue_url_rejects_non_numeric_input_without_calling_gh() {
    for bad in ["", "12a", "1;id", "$(id)", "-1"] {
        let run = run_helper(
            &format!("_github_open_issue_url '{}'", bad.replace('\'', "")),
            "",
            "",
            &[],
        );
        assert_eq!(
            run.output.status.code(),
            Some(1),
            "input {bad:?}\n{}",
            run.diag()
        );
        assert!(
            run.stdout().is_empty() && run.gh_log.is_empty(),
            "input {bad:?}\n{}",
            run.diag()
        );
    }
}

#[test]
fn open_issue_url_accepts_only_open_non_pr_issues_in_this_repository() {
    let cases = [
        ("open", true),
        ("closed", false),
        ("pr", false),
        ("transferred", false),
        ("garbage", false),
    ];
    for (state, accepted) in cases {
        let run = run_helper("_github_open_issue_url 42", "", "", &[("GH_API_42", state)]);
        if accepted {
            assert_eq!(run.output.status.code(), Some(0), "{state}\n{}", run.diag());
            assert_eq!(
                run.stdout().trim(),
                format!("{REPO_URL}/issues/42"),
                "{state}"
            );
        } else {
            assert_eq!(run.output.status.code(), Some(1), "{state}\n{}", run.diag());
            assert!(run.stdout().is_empty(), "{state}\n{}", run.diag());
        }
    }
    let missing = run_helper("_github_open_issue_url 42", "", "", &[]);
    assert_eq!(
        missing.output.status.code(),
        Some(1),
        "a 404 rejects\n{}",
        missing.diag()
    );
}

#[test]
fn select_returns_1_with_empty_stdout_when_nothing_is_selected() {
    let run = run_helper(
        "rc=0; out=$(select_github_tracking_issue) || rc=$?; printf 'rc=%s out=[%s]\\n' \"$rc\" \"$out\"",
        "",
        "Follows #1585.",
        &[("GH_API_1585", "pr")],
    );
    assert!(run.output.status.success(), "{}", run.diag());
    assert_eq!(run.stdout().trim(), "rc=1 out=[]", "{}", run.diag());
}

#[test]
fn select_does_not_exit_the_calling_shell() {
    let run = run_helper(
        "select_github_tracking_issue || true; echo still-running",
        "",
        "Add a flag.",
        &[],
    );
    assert!(
        run.stdout().contains("still-running"),
        "the helper must never call exit\n{}",
        run.diag()
    );
}

// ===========================================================================
// Structure: step-03 wiring, line budget and helper header
// ===========================================================================

#[test]
fn step_03_delegates_github_selection_to_the_helper() {
    let body = extract_step_body("step-03-create-issue");
    assert!(
        body.contains("select_github_tracking_issue"),
        "step-03 must call the helper"
    );
    assert!(
        !body.contains("gh issue view \"$REF_ISSUE_NUM\""),
        "step-03 must not reuse the first task-text #N through gh issue view"
    );
}

#[test]
fn workflow_prep_does_not_grow() {
    let text = fs::read_to_string(recipe_file()).expect("read workflow-prep.yaml");
    assert!(
        text.lines().count() <= 391,
        "workflow-prep.yaml must stay at or below 391 lines (step-03 is at its brick budget); now {}",
        text.lines().count()
    );
}

#[test]
fn helper_header_names_the_file_and_lists_the_new_functions() {
    let text = fs::read_to_string(helper_file()).expect("read helper");
    let header: String = text
        .lines()
        .take_while(|l| l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        header.contains("workflow_issue_tracking.sh"),
        "header must name the file\n{header}"
    );
    assert!(
        !header.contains("workflow_local_tracking.sh"),
        "stale file name in header\n{header}"
    );
    for f in [
        "select_github_tracking_issue",
        "_tracking_issue_masked_text",
        "_tracking_issue_refs",
        "_tracking_issue_candidates",
        "_github_open_issue_url",
        "_is_github_issue_url",
    ] {
        assert!(header.contains(f), "header must list {f}\n{header}");
        assert!(text.contains(&format!("{f}()")), "helper must define {f}");
    }
}
