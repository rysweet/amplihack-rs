//! Integration tests for issue #1881: step-03 uses local tracking unless the
//! run opts in to GitHub issue creation, reuses an open issue with exactly the
//! same title, and creates issues with a bounded, jittered backoff.
//!
//! The contract under test is `docs/reference/workflow-tracking-issue-selection.md`
//! (sections "Local tracking by default", "Exact-title reuse", "Bounded issue
//! creation" and "Issue creation helper contract").
//!
//! Every test runs the shipped `step-03-create-issue` body, or sources the
//! shipped helpers, with a cleared environment, a temporary HOME and fake `gh`,
//! `git` and `sleep` first on PATH. No test reaches GitHub, the real
//! `~/.claude`, `~/.amplihack`, the profile root or the tmux server (#1699).
//! No test spawns the amplihack binary, so `isolated(..)` is not needed here.
//!
//! The fake `sleep` logs its argument and returns at once, so the backoff
//! budget is exercised in milliseconds. No test depends on exact `$RANDOM`
//! values: delays are checked against their bounds only.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use regex::Regex;
use serde_yaml::Value;

const REPO_URL: &str = "https://github.com/example-org/example-repo";
const CREATED_URL: &str = "https://github.com/example-org/example-repo/issues/901";
const TITLE: &str = "Add a --json flag to recipe list";
const BODY_MARKER: &str = "BODY-MARKER-7f3a9c";

// ---------------------------------------------------------------------------
// Paths and recipe access
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

fn recipe_file(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join(format!("{name}.yaml"))
}

fn tools_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle").join("tools")
}

fn tracking_helper() -> PathBuf {
    tools_dir().join("workflow_issue_tracking.sh")
}

fn create_helper() -> PathBuf {
    tools_dir().join("workflow_issue_create.sh")
}

fn create_helper_text() -> String {
    fs::read_to_string(create_helper()).unwrap_or_else(|e| {
        panic!(
            "{} must exist (#1881 moves bounded creation into it): {e}",
            create_helper().display()
        )
    })
}

fn load_recipe(name: &str) -> Value {
    let text = fs::read_to_string(recipe_file(name)).expect("read recipe");
    serde_yaml::from_str(&text).expect("parse recipe")
}

fn step_body(step_id: &str) -> String {
    let recipe = load_recipe("workflow-prep");
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("workflow-prep must have steps");
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

/// Shell source with comment lines and trailing ` # ...` comments removed and
/// backslash continuations joined.
fn code_only(text: &str) -> String {
    let joined = text.replace("\\\n", " ");
    joined
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .map(|l| match l.find(" # ") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_executable(path: &Path, content: &str) {
    let mut file = fs::File::create(path).expect("create shim");
    file.write_all(content.as_bytes()).expect("write shim");
    drop(file);
    let mut perms = fs::metadata(path).expect("shim metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod shim");
}

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// Fake `gh`.
///
/// Logs: `$GH_LOG` gets `"$*"` per call; `$GH_ARGV_LOG` gets each argument
/// on its own line followed by `---`; `$GH_PROMPT_LOG` gets every write or
/// lookup call (`issue list`, `issue create`, `label create`, `repo view`,
/// `api rate_limit`, `api user`, `api --hostname`) made without
/// `GH_PROMPT_DISABLED=1`.
///
/// Scripted behaviour (a `*_SEQ` variable is a space-separated list of
/// tokens, one per call; after the end the last token repeats):
///
/// - `issue view N`: prints `$FAKE_REPO_URL/issues/N` when N equals
///   `GH_VIEW_ISSUE`; otherwise fails.
/// - `api user`: prints `GH_USER_LOGIN` (default `example-user`); step 03 must
///   never call it.
///   `GH_API_USER_EXIT` makes it fail with a fake token on stderr.
/// - `api repos/...`: 404.
/// - `api --hostname H repos/O/R/issues/N --jq F` (strict_delivery): answers
///   only that exact shape, and only for `H` = `STRICT_HOST` (default
///   `github.com`), `O/R` = `STRICT_REPO` and `N` = `STRICT_NUMBER`; any other
///   api call with `--hostname` fails, so a wrong endpoint cannot pass.
///   `STRICT_ISSUE` picks the REST document, and the real `jq` applies `F` to
///   it, as `gh api --jq` does: `open` (default), `closed` (closed as not
///   planned), `pull` (a pull request: `pull_request` set, a `/pull/` URL),
///   `moved` (an open issue whose URL is in another repository) or `fail`
///   (HTTP 502).
/// - `api rate_limit`: reset epoch `GH_RESET_EPOCH`, or now + `GH_RESET_IN`
///   (default 10). Honours `--jq` by printing the bare epoch.
/// - `repo view`: `GH_REPO_NWO` (default `example-org/example-repo`), as
///   JSON or, with `--jq`, bare. `GH_REPO_VIEW_EXIT` makes it fail.
/// - `issue list`: `GH_LIST_SEQ` tokens `default` (`GH_LIST_JSON`, default
///   `[]`), `found` (`GH_LIST_FOUND_JSON`), `fail`, `garbage`, `object`.
/// - `label create`: exits `GH_LABEL_EXIT` (default 0), with a fake token on
///   stderr when it fails.
/// - `issue create`: `GH_CREATE_SEQ` tokens `ok`, `noisy`, `secondary`,
///   `secondary429`, `abuse`, `graphql_secondary`, `too_quickly`, `retry:N`,
///   `primary`, `auth`, `forbidden`, `label`, `notfound`, `other`, and
///   `timeout` (exits 124, as `timeout` does when it kills gh). Records the body file's mode and path.
const FAKE_GH: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
{ for a in "$@"; do printf '%s\n' "$a"; done; printf '%s\n' '---'; } >> "$GH_ARGV_LOG"
case "$1:$2" in
  issue:list|issue:create|label:create|repo:view|api:rate_limit|api:user|api:--hostname)
    [ "${GH_PROMPT_DISABLED:-}" = "1" ] || printf '%s\n' "$*" >> "$GH_PROMPT_LOG" ;;
esac
next_token() {
  _seq="$1"; _file="$2"
  _n=$(cat "$_file" 2>/dev/null || echo 0); _n=$((_n + 1)); printf '%s' "$_n" > "$_file"
  tok=""; _i=0
  for _t in $_seq; do _i=$((_i + 1)); tok="$_t"; [ "$_i" -eq "$_n" ] && break; done
}
has_jq() { case " $* " in *" --jq"*|*" -q "*) return 0 ;; esac; return 1; }
case "$1:$2" in
  issue:view)
    if [ -n "${GH_VIEW_ISSUE:-}" ] && [ "$GH_VIEW_ISSUE" = "$3" ]; then printf '%s/issues/%s\n' "$FAKE_REPO_URL" "$3"; exit 0; fi
    echo "GraphQL: Could not resolve to an issue or pull request with the number of $3." >&2; exit 1 ;;
  api:rate_limit)
    if [ -n "${GH_RESET_EPOCH:-}" ]; then e="$GH_RESET_EPOCH"; else e=$(( $(date +%s) + ${GH_RESET_IN:-10} )); fi
    if has_jq "$@"; then printf '%s\n' "$e"; else printf '{"resources":{"graphql":{"limit":5000,"remaining":0,"reset":%s}}}\n' "$e"; fi
    exit 0 ;;
  api:user)
    if [ -n "${GH_API_USER_EXIT:-}" ]; then echo "HTTP 401: Bad credentials token ghp_USERFAKEUSERFAKE" >&2; exit "$GH_API_USER_EXIT"; fi
    printf '%s\n' "${GH_USER_LOGIN:-example-user}"
    exit 0 ;;
  api:--hostname)
    if [ "$#" -ne 6 ] || [ "$3" != "${STRICT_HOST:-github.com}" ] || [ "$4" != "repos/${STRICT_REPO:-}/issues/${STRICT_NUMBER:-}" ] || [ "$5" != "--jq" ]; then
      echo "fake gh: unexpected strict api call: $*" >&2; exit 7
    fi
    issue="https://$3/$STRICT_REPO/issues/$STRICT_NUMBER"
    case "${STRICT_ISSUE:-open}" in
      open) doc="{\"state\":\"open\",\"html_url\":\"$issue\"}" ;;
      closed) doc="{\"state\":\"closed\",\"state_reason\":\"not_planned\",\"html_url\":\"$issue\"}" ;;
      pull) doc="{\"state\":\"open\",\"html_url\":\"https://$3/$STRICT_REPO/pull/$STRICT_NUMBER\",\"pull_request\":{\"url\":\"x\"}}" ;;
      moved) doc="{\"state\":\"open\",\"html_url\":\"https://$3/elsewhere/moved/issues/$STRICT_NUMBER\"}" ;;
      fail) echo "HTTP 502: Bad Gateway token ghp_STRICTFAKESTRICTFAKE" >&2; exit 1 ;;
    esac
    printf '%s\n' "$doc" | jq -r "$6"
    exit $? ;;
  api:*) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
  repo:view)
    if [ -n "${GH_REPO_VIEW_EXIT:-}" ]; then echo "gh: could not determine repository" >&2; exit "$GH_REPO_VIEW_EXIT"; fi
    nwo="${GH_REPO_NWO:-example-org/example-repo}"
    if has_jq "$@"; then printf '%s\n' "$nwo"; else printf '{"nameWithOwner":"%s"}\n' "$nwo"; fi
    exit 0 ;;
  issue:list)
    next_token "${GH_LIST_SEQ:-default}" "$STATE_DIR/list.count"
    case "$tok" in
      found) printf '%s\n' "$GH_LIST_FOUND_JSON" ;;
      fail) echo "HTTP 502: Bad Gateway (https://api.github.com/graphql) token ghp_FAKEFAKEFAKEFAKE" >&2; exit 1 ;;
      garbage) echo 'not json at all' ;;
      object) printf '{"url":"%s/issues/5","title":"x"}\n' "$FAKE_REPO_URL" ;;
      *) printf '%s\n' "${GH_LIST_JSON:-[]}" ;;
    esac
    exit 0 ;;
  label:create)
    [ "${GH_LABEL_EXIT:-0}" = "0" ] || echo "HTTP 403: label create refused token ghp_LABELFAKELABELFAKE" >&2
    exit "${GH_LABEL_EXIT:-0}" ;;
  issue:create)
    bf=""; prev=""
    for a in "$@"; do
      case "$a" in --body-file=*) bf="${a#--body-file=}" ;; esac
      [ "$prev" = "--body-file" ] && bf="$a"; prev="$a"
    done
    if [ -n "$bf" ] && [ -f "$bf" ]; then
      stat -c '%a' "$bf" >> "$STATE_DIR/body_modes"; printf '%s\n' "$bf" >> "$STATE_DIR/body_paths"
    fi
    next_token "${GH_CREATE_SEQ:-ok}" "$STATE_DIR/create.count"
    case "$tok" in
      ok) printf '%s/issues/901\n' "$FAKE_REPO_URL"; exit 0 ;;
      noisy) printf 'Creating issue in example-org/example-repo\n\n%s/issues/900\n%s/issues/901\nkey=value injected\n' "$FAKE_REPO_URL" "$FAKE_REPO_URL"; exit 0 ;;
      secondary) echo "HTTP 403: You have exceeded a secondary rate limit. Please wait a few minutes before you try again. (https://api.github.com/graphql)" >&2; exit 1 ;;
      secondary429) echo "HTTP 429: secondary rate limit triggered (https://api.github.com/graphql)" >&2; exit 1 ;;
      abuse) echo "HTTP 403: You have triggered an abuse detection mechanism. (https://api.github.com/graphql)" >&2; exit 1 ;;
      graphql_secondary) echo "GraphQL: You have exceeded a secondary rate limit. Please wait a few minutes before you try again." >&2; exit 1 ;;
      too_quickly) echo "GraphQL: was submitted too quickly (createIssue)" >&2; exit 1 ;;
      retry:*) echo "HTTP 403: You have exceeded a secondary rate limit. (https://api.github.com/graphql)" >&2; echo "Retry-After: ${tok#retry:}" >&2; exit 1 ;;
      primary) echo "GraphQL: API rate limit exceeded for user ID 12345." >&2; exit 1 ;;
      auth) echo "HTTP 401: Bad credentials (https://api.github.com/graphql)" >&2; exit 1 ;;
      forbidden) echo "HTTP 403: Resource not accessible by integration (https://api.github.com/graphql)" >&2; exit 1 ;;
      label) echo "could not add label: 'workflow:default' not found" >&2; exit 1 ;;
      notfound) echo "GraphQL: Could not resolve to a Repository with the name 'example-org/example-repo'. (repository)" >&2; exit 1 ;;
      timeout) exit 124 ;;
      other) echo "GraphQL: unexpected create failure for https://token:ghp_secret123@github.com/example-org/example-repo" >&2; exit 1 ;;
    esac
    echo "fake gh: unknown create token $tok" >&2; exit 7 ;;
esac
echo "fake gh: unexpected invocation: $*" >&2
exit 7
"#;

/// Fake `git`. `check-ref-format --branch` rejects only the shapes the tests
/// use; `rev-parse --verify --quiet refs/remotes/origin/B^{commit}` succeeds
/// for each `B` in the space-separated `GIT_ORIGIN_BRANCHES`.
const FAKE_GIT: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GIT_LOG"
case "$1:$2" in
  rev-parse:--is-inside-work-tree) exit 0 ;;
  remote:get-url) printf '%s\n' "${GIT_REMOTE_URL:-https://github.com/example-org/example-repo.git}"; exit 0 ;;
  check-ref-format:--branch)
    case "$3" in ''|-*|*..*|*' '*|HEAD) exit 128 ;; esac
    printf '%s\n' "$3"; exit 0 ;;
  rev-parse:--verify)
    b="${4#refs/remotes/origin/}"; b="${b%^\{commit\}}"
    case " ${GIT_ORIGIN_BRANCHES:-} " in *" $b "*) echo 0123456789abcdef0123456789abcdef01234567; exit 0 ;; esac
    exit 1 ;;
esac
echo "fake git: unexpected invocation: $*" >&2
exit 2
"#;

/// Fake `sleep`: logs the requested delay and returns at once.
const FAKE_SLEEP: &str = r#"#!/bin/sh
printf '%s\n' "$1" >> "$SLEEP_LOG"
exit 0
"#;

const FAKE_AZ: &str = r#"#!/bin/sh
echo "fake az: unexpected invocation: $*" >&2
exit 7
"#;

struct Sandbox {
    temp: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        for dir in ["repo", "bin", "home", "state"] {
            fs::create_dir_all(temp.path().join(dir)).expect("mkdir");
        }
        let bin = temp.path().join("bin");
        write_executable(&bin.join("gh"), FAKE_GH);
        write_executable(&bin.join("git"), FAKE_GIT);
        write_executable(&bin.join("sleep"), FAKE_SLEEP);
        write_executable(&bin.join("az"), FAKE_AZ);
        // Keep the sandbox PATH private while finding the installed prerequisite
        // on macOS, where Homebrew tools are outside /usr/local/bin.
        for (name, lookup) in [
            ("timeout", "command -v timeout"),
            ("jq", "command -v jq"),
            ("sha256sum", "command -v sha256sum || command -v gsha256sum"),
            ("stat", "command -v gstat || command -v stat"),
        ] {
            let output = Command::new("bash").args(["-c", lookup]).output().unwrap();
            assert!(
                output.status.success(),
                "{name} must be installed for process fixtures"
            );
            let tool = String::from_utf8(output.stdout).unwrap();
            std::os::unix::fs::symlink(tool.trim(), bin.join(name)).unwrap();
        }
        Sandbox { temp }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }

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
            .env("STATE_DIR", self.path("state"))
            .env("GH_LOG", self.path("gh.log"))
            .env("GH_ARGV_LOG", self.path("gh.argv"))
            .env("GH_PROMPT_LOG", self.path("gh.prompt"))
            .env("GIT_LOG", self.path("git.log"))
            .env("SLEEP_LOG", self.path("sleep.log"))
            .env("FAKE_REPO_URL", REPO_URL)
            .current_dir(self.path("repo"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
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

    fn code(&self) -> Option<i32> {
        self.output.status.code()
    }

    fn diag(&self) -> String {
        format!(
            "exit: {:?}\nstdout:\n{}\nstderr:\n{}\ngh log:\n{}\nsleeps: {:?}",
            self.code(),
            self.stdout(),
            self.stderr(),
            self.gh_log(),
            self.sleeps()
        )
    }

    /// Every gh call as its argv (without the leading `gh`).
    fn gh_calls(&self) -> Vec<Vec<String>> {
        let text = self.sandbox.read("gh.argv");
        let mut calls = Vec::new();
        let mut cur = Vec::new();
        for line in text.lines() {
            if line == "---" {
                calls.push(std::mem::take(&mut cur));
            } else {
                cur.push(line.to_owned());
            }
        }
        calls
    }

    fn calls_of(&self, a: &str, b: &str) -> Vec<Vec<String>> {
        self.gh_calls()
            .into_iter()
            .filter(|c| {
                c.first().map(String::as_str) == Some(a) && c.get(1).map(String::as_str) == Some(b)
            })
            .collect()
    }

    fn creates(&self) -> Vec<Vec<String>> {
        self.calls_of("issue", "create")
    }

    fn lists(&self) -> Vec<Vec<String>> {
        self.calls_of("issue", "list")
    }

    fn sleeps(&self) -> Vec<u64> {
        self.sandbox
            .read("sleep.log")
            .lines()
            .map(|l| {
                l.trim()
                    .trim_end_matches('s')
                    .parse::<u64>()
                    .unwrap_or_else(|_| panic!("sleep got a non-integer delay {l:?}"))
            })
            .collect()
    }

    fn assert_exit(&self, code: i32) {
        assert_eq!(
            self.code(),
            Some(code),
            "unexpected exit code\n{}",
            self.diag()
        );
    }

    fn assert_local(&self) {
        self.assert_exit(0);
        let out = self.stdout();
        assert!(
            out.contains("tracking_system=local") && out.contains("issue_creation=local-tracking"),
            "step-03 must print the local tracking block\n{}",
            self.diag()
        );
    }

    fn assert_no_gh_write_or_search(&self) {
        for (a, b) in [("issue", "list"), ("issue", "create"), ("label", "create")] {
            assert!(
                self.calls_of(a, b).is_empty(),
                "the default path must not call `gh {a} {b}`\n{}",
                self.diag()
            );
        }
    }

    fn assert_created(&self) {
        self.assert_exit(0);
        assert_eq!(self.stdout().trim(), CREATED_URL, "{}", self.diag());
    }
}

/// Value of a `--flag=value` (or `--flag value`) argument.
fn flag_value(call: &[String], flag: &str) -> Option<String> {
    let eq = format!("{flag}=");
    for (i, a) in call.iter().enumerate() {
        if let Some(v) = a.strip_prefix(&eq) {
            return Some(v.to_owned());
        }
        if a == flag {
            return call.get(i + 1).cloned();
        }
    }
    None
}

fn has_label_flag(call: &[String]) -> bool {
    call.iter()
        .any(|a| a == "--label" || a.starts_with("--label="))
}

// ---------------------------------------------------------------------------
// Runners
// ---------------------------------------------------------------------------

fn run_step(issue_number: &str, task: &str, env: &[(&str, &str)]) -> Run {
    let sandbox = Sandbox::new();
    let mut cmd = sandbox.bash(&step_body("step-03-create-issue"));
    cmd.env("RECIPE_VAR_remote_host_type", "github")
        .env("RECIPE_VAR_issue_number", issue_number)
        .env("TASK_DESCRIPTION", task)
        .env(
            "RECIPE_VAR_final_requirements",
            format!("Issue #1881 test requirements {BODY_MARKER}"),
        );
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run step-03-create-issue");
    Run { output, sandbox }
}

/// Opted in, with a small backoff budget and a 3s secondary floor (the
/// default is 60s) so give-up paths finish fast.
fn run_opted_in(task: &str, env: &[(&str, &str)]) -> Run {
    let mut all: Vec<(&str, &str)> = vec![
        ("RECIPE_VAR_create_issue", "true"),
        ("AMPLIHACK_ISSUE_BACKOFF_BASE_SECS", "2"),
        ("AMPLIHACK_ISSUE_BACKOFF_MAX_DELAY_SECS", "8"),
        ("AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS", "30"),
        ("AMPLIHACK_ISSUE_BACKOFF_SECONDARY_MIN_SECS", "3"),
    ];
    all.extend_from_slice(env);
    run_step("", task, &all)
}

fn exact_list_json(title: &str, url: &str) -> String {
    serde_json::json!([{ "number": 1700, "title": title, "url": url }]).to_string()
}

/// Source both helpers under `set -euo pipefail`, as step-03 does, and run
/// `call`.
fn run_helper(call: &str, env: &[(&str, &str)]) -> Run {
    let sandbox = Sandbox::new();
    let script = format!(
        "set -euo pipefail\nTASK_DESC=\"${{TASK_DESCRIPTION:-}}\"\nEXISTING_ISSUE_NUMBER=\"\"\n. \"$TRACKING\"\n. \"$CREATE\"\n{call}\n"
    );
    let mut cmd = sandbox.bash(&script);
    cmd.env("TRACKING", tracking_helper())
        .env("CREATE", create_helper())
        .env("TASK_DESCRIPTION", TITLE);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run helper");
    Run { output, sandbox }
}

/// Run step-03b on step-03's stdout and return the `issue_number` it prints.
fn step_03b_issue_number(run: &Run, task: &str) -> String {
    let sandbox = Sandbox::new();
    let output = sandbox
        .bash(&step_body("step-03b-extract-issue-number"))
        .env("RECIPE_VAR_issue_creation", run.stdout())
        .env("TASK_DESCRIPTION", task)
        .output()
        .expect("run step-03b");
    assert!(
        output.status.success(),
        "step-03b must succeed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

// ===========================================================================
// AC1: local tracking by default, no GitHub write
// ===========================================================================

#[test]
fn default_run_uses_local_tracking_and_makes_no_gh_call() {
    let run = run_step("", TITLE, &[]);
    run.assert_local();
    assert!(
        run.gh_log().is_empty(),
        "with no #N and no opt-in, step-03 must not call gh at all\n{}",
        run.diag()
    );
    assert!(
        run.stderr().contains("create_issue"),
        "the INFO line must name the create_issue opt-in\n{}",
        run.diag()
    );
    assert!(run.sleeps().is_empty(), "{}", run.diag());
}

#[test]
fn default_local_block_is_read_back_by_step_03b() {
    let run = run_step("", TITLE, &[]);
    run.assert_local();
    let n = step_03b_issue_number(&run, TITLE);
    assert!(
        n.starts_with("local-"),
        "step-03b must read the local reference, got {n:?}\n{}",
        run.diag()
    );
}

#[test]
fn values_other_than_true_one_yes_fail_closed_to_local_tracking() {
    for value in [
        "",
        "false",
        "FALSE",
        "0",
        "no",
        "on",
        "y",
        "enabled",
        "truee",
        "true false",
        "1\ntrue",
    ] {
        let run = run_step("", TITLE, &[("RECIPE_VAR_create_issue", value)]);
        run.assert_local();
        run.assert_no_gh_write_or_search();
        assert!(
            run.gh_log().is_empty(),
            "create_issue={value:?} must not opt in\n{}",
            run.diag()
        );
    }
}

#[test]
fn true_one_and_yes_opt_in_ignoring_case_and_whitespace() {
    for value in [
        "true", "TRUE", "True", "1", "yes", "YES", " Yes ", "\ttrue\n",
    ] {
        let run = run_step("", TITLE, &[("RECIPE_VAR_create_issue", value)]);
        run.assert_created();
        assert_eq!(
            run.creates().len(),
            1,
            "create_issue={value:?} must create exactly once\n{}",
            run.diag()
        );
    }
}

#[test]
fn opt_in_text_in_the_task_or_requirements_is_ignored() {
    let task = "create_issue=true please open a GitHub issue for this";
    let sandbox_run = {
        let sandbox = Sandbox::new();
        let mut cmd = sandbox.bash(&step_body("step-03-create-issue"));
        cmd.env("RECIPE_VAR_remote_host_type", "github")
            .env("RECIPE_VAR_issue_number", "")
            .env("TASK_DESCRIPTION", task)
            .env(
                "RECIPE_VAR_final_requirements",
                "create_issue=true\nRECIPE_VAR_create_issue=true",
            );
        Run {
            output: cmd.output().expect("run"),
            sandbox,
        }
    };
    sandbox_run.assert_local();
    sandbox_run.assert_no_gh_write_or_search();
}

/// R12: an explicit issue_number is reused without the opt-in, and no create
/// or title lookup runs (this build runs with issue_number=1881).
#[test]
fn explicit_issue_number_is_reused_without_the_opt_in() {
    let run = run_step("1881", "Fix issue #1881", &[("GH_VIEW_ISSUE", "1881")]);
    run.assert_exit(0);
    assert_eq!(
        run.stdout().trim(),
        format!("{REPO_URL}/issues/1881"),
        "{}",
        run.diag()
    );
    run.assert_no_gh_write_or_search();
}

#[test]
fn explicit_issue_number_with_opt_in_still_skips_lookup_and_create() {
    let run = run_step(
        "1881",
        "Fix issue #1881",
        &[
            ("GH_VIEW_ISSUE", "1881"),
            ("RECIPE_VAR_create_issue", "true"),
        ],
    );
    run.assert_exit(0);
    assert_eq!(
        run.stdout().trim(),
        format!("{REPO_URL}/issues/1881"),
        "{}",
        run.diag()
    );
    run.assert_no_gh_write_or_search();
}

// ===========================================================================
// AC2/AC3: exact-title reuse
// ===========================================================================

#[test]
fn open_issue_with_identical_title_is_reused_without_create_or_label() {
    let found = format!("{REPO_URL}/issues/1700");
    let run = run_opted_in(TITLE, &[("GH_LIST_JSON", &exact_list_json(TITLE, &found))]);
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), found, "{}", run.diag());
    assert!(run.creates().is_empty(), "{}", run.diag());
    assert!(
        run.calls_of("label", "create").is_empty(),
        "label create runs only after the lookup misses\n{}",
        run.diag()
    );
}

#[test]
fn similar_but_different_title_is_not_reused() {
    let found = format!("{REPO_URL}/issues/1700");
    let run = run_opted_in(
        TITLE,
        &[(
            "GH_LIST_JSON",
            &exact_list_json(&format!("{TITLE}."), &found),
        )],
    );
    run.assert_created();
    assert_eq!(run.creates().len(), 1, "{}", run.diag());
}

#[test]
fn lookup_uses_open_state_quoted_phrase_and_in_title() {
    let run = run_opted_in(TITLE, &[]);
    run.assert_created();
    let lists = run.lists();
    assert_eq!(
        lists.len(),
        1,
        "one lookup before the first create\n{}",
        run.diag()
    );
    let call = &lists[0];
    assert_eq!(
        flag_value(call, "--state").as_deref(),
        Some("open"),
        "{call:?}"
    );
    assert_eq!(
        flag_value(call, "--search").as_deref(),
        Some(format!("\"{TITLE}\" in:title").as_str()),
        "search must be the quoted title plus in:title: {call:?}"
    );
    assert!(
        call.iter().any(|a| a.starts_with("--search=")),
        "the search phrase must be passed as --search=VALUE (S7): {call:?}"
    );
    let json = flag_value(call, "--json").unwrap_or_default();
    assert!(
        json.contains("title") && json.contains("url") && !json.contains("author"),
        "reuse does not depend on who opened the issue: {call:?}"
    );
}

#[test]
fn search_qualifiers_and_quotes_in_the_title_are_neutralised() {
    let task = r#"Fix "quoted" thing repo:evil/x is:closed"#;
    let run = run_opted_in(task, &[]);
    run.assert_created();
    let search = flag_value(&run.lists()[0], "--search").unwrap_or_default();
    assert_eq!(
        search,
        r#""Fix quoted thing repo:evil/x is:closed" in:title"#,
        "double quotes are removed and the title is one quoted phrase\n{}",
        run.diag()
    );
}

#[test]
fn title_ending_in_backslash_is_searched_without_an_escaped_quote() {
    let task = r"Fix path C:\dir\";
    let run = run_opted_in(task, &[]);
    run.assert_created();
    assert_eq!(run.creates().len(), 1, "{}", run.diag());
    let search = flag_value(&run.lists()[0], "--search").unwrap_or_default();
    assert!(
        !search.contains('\\'),
        "backslashes are removed from the search phrase (no \\\" escape): {search:?}\n{}",
        run.diag()
    );
    assert!(
        search.ends_with("\" in:title"),
        "{search:?}\n{}",
        run.diag()
    );
}

#[test]
fn invalid_backoff_setting_warns_and_the_run_still_completes() {
    let run = run_step(
        "",
        TITLE,
        &[
            ("RECIPE_VAR_create_issue", "true"),
            ("AMPLIHACK_ISSUE_BACKOFF_BASE_SECS", "abc"),
        ],
    );
    run.assert_created();
    assert!(
        run.stderr().contains(
            "WARNING: ignoring invalid AMPLIHACK_ISSUE_BACKOFF_BASE_SECS (3 characters); using default 5"
        ),
        "{}",
        run.diag()
    );
    assert!(
        !run.stderr().contains("=abc") && !run.stderr().contains(" abc"),
        "no part of the invalid value is printed\n{}",
        run.diag()
    );
    assert!(
        !run.stdout().contains("WARNING"),
        "the warning goes to stderr only\n{}",
        run.diag()
    );
}

#[test]
fn title_with_quotes_matches_exactly_through_jq_arg() {
    let task = r#"Fix "x" and \(.url) | .title"#;
    let found = format!("{REPO_URL}/issues/1700");
    let run = run_opted_in(task, &[("GH_LIST_JSON", &exact_list_json(task, &found))]);
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), found, "{}", run.diag());
    assert!(run.creates().is_empty(), "{}", run.diag());
}

#[test]
fn title_is_trimmed_and_used_for_both_lookup_and_create() {
    let run = run_opted_in(&format!("   {TITLE}   "), &[]);
    run.assert_created();
    let create = &run.creates()[0];
    assert_eq!(
        flag_value(create, "--title").as_deref(),
        Some(TITLE),
        "{create:?}"
    );
    assert!(
        create.iter().any(|a| a.starts_with("--title=")),
        "the title must be passed as --title=VALUE (S7): {create:?}"
    );
    assert_eq!(
        flag_value(&run.lists()[0], "--search").as_deref(),
        Some(format!("\"{TITLE}\" in:title").as_str())
    );
}

#[test]
fn trimmed_title_matches_its_own_earlier_issue() {
    let found = format!("{REPO_URL}/issues/1700");
    let run = run_opted_in(
        &format!("{TITLE}  \n"),
        &[("GH_LIST_JSON", &exact_list_json(TITLE, &found))],
    );
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), found, "{}", run.diag());
}

#[test]
fn title_cut_at_200_then_trimmed() {
    let task = format!("{} tail words after the cut", "a".repeat(199));
    let run = run_opted_in(&task, &[]);
    run.assert_created();
    assert_eq!(
        flag_value(&run.creates()[0], "--title").as_deref(),
        Some("a".repeat(199).as_str()),
        "a title ending in a space after the 200-character cut must be trimmed\n{}",
        run.diag()
    );
}

#[test]
fn newlines_become_spaces_and_control_characters_are_removed() {
    let run = run_opted_in("Fix\x07 the\x1b bell\r\nthing", &[]);
    run.assert_created();
    assert_eq!(
        flag_value(&run.creates()[0], "--title").as_deref(),
        Some("Fix the bell  thing"),
        "{}",
        run.diag()
    );
}

#[test]
fn title_that_looks_like_a_flag_is_one_argument() {
    let task = "--repo=evil/x --help";
    let run = run_opted_in(task, &[]);
    run.assert_created();
    let create = &run.creates()[0];
    assert!(
        create.iter().any(|a| a == &format!("--title={task}")),
        "the whole title must be one --title= argument: {create:?}"
    );
    assert!(!create.iter().any(|a| a == "--repo=evil/x"), "{create:?}");
}

#[test]
fn same_title_in_another_repository_is_skipped_for_a_later_match() {
    let good = format!("{REPO_URL}/issues/1702");
    let json = serde_json::json!([
        { "number": 1, "title": TITLE, "url": "https://github.com/other-org/other-repo/issues/1" },
        { "number": 2, "title": TITLE, "url": format!("{REPO_URL}/pull/2") },
        { "number": 3, "title": TITLE, "url": format!("{REPO_URL}/issues/3\n") },
        { "number": 1702, "title": TITLE, "url": good.replace("example-org", "Example-Org") },
    ])
    .to_string();
    let run = run_opted_in(TITLE, &[("GH_LIST_JSON", &json)]);
    run.assert_exit(0);
    assert_eq!(
        run.stdout().trim(),
        good.replace("example-org", "Example-Org"),
        "only the result in this repository (owner case ignored) may be reused\n{}",
        run.diag()
    );
    assert!(run.creates().is_empty(), "{}", run.diag());
}

#[test]
fn only_results_from_other_repositories_lead_to_a_create() {
    let json = serde_json::json!([
        { "number": 1, "title": TITLE, "url": "https://github.com/other-org/other-repo/issues/1" },
        { "number": 2, "title": TITLE, "url": "http://github.com/example-org/example-repo/issues/2" },
    ])
    .to_string();
    let run = run_opted_in(TITLE, &[("GH_LIST_JSON", &json)]);
    run.assert_created();
}

/// A failed search is followed by the recent listing, which uses the core
/// quota rather than the search quota; when that succeeds with no match the
/// create goes ahead.
#[test]
fn failed_search_falls_back_to_the_recent_listing_then_creates() {
    let run = run_opted_in(TITLE, &[("GH_LIST_SEQ", "fail default")]);
    run.assert_created();
    assert_eq!(run.creates().len(), 1, "{}", run.diag());
    let lists = run.lists();
    assert_eq!(lists.len(), 2, "{}", run.diag());
    assert!(
        flag_value(&lists[0], "--search").is_some(),
        "{}",
        run.diag()
    );
    assert!(
        flag_value(&lists[1], "--search").is_none(),
        "the fallback lookup must not use the search API\n{}",
        run.diag()
    );
    assert!(
        !run.stderr().contains("ghp_FAKEFAKEFAKEFAKE"),
        "{}",
        run.diag()
    );
    assert!(
        run.stderr().contains("HTTP 502: Bad Gateway") && run.stderr().contains("<redacted-token>"),
        "the failed lookup's gh stderr is shown, sanitised\n{}",
        run.diag()
    );
}

#[test]
fn failed_search_then_recent_match_reuses_without_create() {
    let found = format!("{REPO_URL}/issues/1700");
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_LIST_SEQ", "fail found"),
            ("GH_LIST_FOUND_JSON", &exact_list_json(TITLE, &found)),
        ],
    );
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), found, "{}", run.diag());
    assert!(run.creates().is_empty(), "{}", run.diag());
}

/// #1881 review: the duplicates came from separate runs of the same task, so
/// a run that cannot check for an existing issue must not create one blind.
#[test]
fn failed_first_lookup_never_creates_and_uses_local_tracking() {
    let run = run_opted_in(TITLE, &[("GH_LIST_SEQ", "fail")]);
    run.assert_local();
    assert!(run.creates().is_empty(), "{}", run.diag());
    assert!(run.calls_of("label", "create").is_empty(), "{}", run.diag());
    assert_eq!(
        run.lists().len(),
        2,
        "search, then the recent listing\n{}",
        run.diag()
    );
    assert!(
        run.stderr().contains("not creating one blind"),
        "{}",
        run.diag()
    );
    assert!(run.sleeps().is_empty(), "{}", run.diag());
}

#[test]
fn non_array_lookup_output_counts_as_a_failed_lookup() {
    for tok in ["garbage", "object"] {
        let run = run_opted_in(TITLE, &[("GH_LIST_SEQ", tok)]);
        run.assert_local();
        assert!(run.creates().is_empty(), "{tok}\n{}", run.diag());
        assert!(
            run.stderr().contains("WARNING: exact-title lookup failed"),
            "{tok}\n{}",
            run.diag()
        );
    }
}

#[test]
fn failed_repo_view_counts_as_a_failed_lookup() {
    let found = format!("{REPO_URL}/issues/1700");
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_REPO_VIEW_EXIT", "1"),
            ("GH_LIST_JSON", &exact_list_json(TITLE, &found)),
        ],
    );
    run.assert_local();
    assert!(run.creates().is_empty(), "{}", run.diag());
    assert!(
        run.stderr().contains("WARNING: exact-title lookup failed"),
        "a match that cannot be checked against this repository must not be reused\n{}",
        run.diag()
    );
}

// ===========================================================================
// AC4: secondary rate limits back off with jitter, bounded
// ===========================================================================

#[test]
fn secondary_limit_then_success_retries_once_after_a_lookup() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "secondary ok")]);
    run.assert_created();
    let creates = run.creates();
    assert_eq!(creates.len(), 2, "{}", run.diag());
    assert!(
        creates.iter().all(|c| has_label_flag(c)),
        "a rate-limited create must not trigger the no-label fallback\n{}",
        run.diag()
    );
    assert_eq!(
        run.lists().len(),
        2,
        "a lookup must run before the retry\n{}",
        run.diag()
    );
    let sleeps = run.sleeps();
    assert_eq!(sleeps.len(), 1, "{}", run.diag());
    assert!((1..=8).contains(&sleeps[0]), "{}", run.diag());
    assert!(
        run.stderr().contains("secondary rate limit; backing off"),
        "{}",
        run.diag()
    );
}

#[test]
fn abuse_detection_and_429_secondary_are_also_backed_off() {
    for tok in ["abuse", "secondary429", "graphql_secondary", "too_quickly"] {
        let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", &format!("{tok} ok"))]);
        run.assert_created();
        assert_eq!(run.creates().len(), 2, "{tok}\n{}", run.diag());
        assert_eq!(run.sleeps().len(), 1, "{tok}\n{}", run.diag());
        assert_eq!(
            run.lists().len(),
            2,
            "{tok}: a lookup must run before the retry\n{}",
            run.diag()
        );
    }
}

/// A create GitHub accepted but reported as failed is found by the lookup
/// before the retry and reused, never created twice.
#[test]
fn lookup_before_retry_reuses_an_issue_the_failed_create_made() {
    let found = format!("{REPO_URL}/issues/1777");
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_CREATE_SEQ", "secondary ok"),
            ("GH_LIST_SEQ", "default found"),
            ("GH_LIST_FOUND_JSON", &exact_list_json(TITLE, &found)),
        ],
    );
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), found, "{}", run.diag());
    assert_eq!(run.creates().len(), 1, "no second create\n{}", run.diag());
}

#[test]
fn persistent_secondary_limit_gives_up_within_budget_with_jittered_delays() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "secondary")]);
    run.assert_local();
    let sleeps = run.sleeps();
    let total: u64 = sleeps.iter().sum();
    assert!(
        sleeps.len() >= 3,
        "floor 3, base 2, cap 8, total 30 gives at least 3 rounds\n{}",
        run.diag()
    );
    assert!(
        sleeps.iter().all(|s| (4..=11).contains(s)),
        "every delay is floor + a jitter delay in [1, cap]\n{}",
        run.diag()
    );
    assert!(total <= 30, "total sleep within MAX_TOTAL\n{}", run.diag());
    assert!(
        sleeps.iter().any(|s| *s != sleeps[0]),
        "delays must not be at a fixed interval\n{}",
        run.diag()
    );
    // floor 3 plus d/2..d: round 0 (d=2) waits 4..5, round 2 (d=8) waits 7..11.
    assert!(sleeps[0] <= 5 && sleeps[2] >= 7, "{}", run.diag());
    assert_eq!(
        run.creates().len(),
        sleeps.len() + 1,
        "one create per round, none during a skipped wait\n{}",
        run.diag()
    );
    assert!(
        run.creates().iter().all(|c| has_label_flag(c)),
        "{}",
        run.diag()
    );
    let stderr = run.stderr();
    assert!(
        stderr.contains(&format!(
            "WARNING: GitHub issue creation gave up after {total}s (AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS=30); falling back to local tracking."
        )),
        "give-up line must report the sum of planned waits\n{}",
        run.diag()
    );
    assert!(stderr.contains("(round 0, 0/30s used)"), "{}", run.diag());
    assert!(
        !stderr.contains(BODY_MARKER),
        "the issue body is never logged\n{}",
        run.diag()
    );
    let n = step_03b_issue_number(&run, TITLE);
    assert!(
        n.starts_with("local-"),
        "step-03b reads the local block, got {n:?}"
    );
}

/// #1881 review: GitHub asks for at least a minute before retrying a
/// secondary limit that sent no retry-after, so the default first wait is the
/// 60s floor plus round 0's jitter (BASE 5: 2..4).
#[test]
fn default_secondary_backoff_waits_at_least_one_minute() {
    let run = run_step(
        "",
        TITLE,
        &[
            ("RECIPE_VAR_create_issue", "true"),
            ("GH_CREATE_SEQ", "secondary ok"),
        ],
    );
    run.assert_created();
    let sleeps = run.sleeps();
    assert_eq!(sleeps.len(), 1, "{}", run.diag());
    assert!(
        (62..=64).contains(&sleeps[0]),
        "floor 60 plus round 0's 2..4\n{}",
        run.diag()
    );
}

/// With the defaults a worker that stays secondary-limited makes at most five
/// creates in its 300s budget, then tracks locally.
#[test]
fn default_persistent_secondary_limit_makes_few_creates() {
    let run = run_step(
        "",
        TITLE,
        &[
            ("RECIPE_VAR_create_issue", "true"),
            ("GH_CREATE_SEQ", "secondary"),
        ],
    );
    run.assert_local();
    let sleeps = run.sleeps();
    assert!(sleeps.iter().all(|s| *s >= 60), "{}", run.diag());
    assert!(sleeps.iter().sum::<u64>() <= 300, "{}", run.diag());
    assert!((3..=5).contains(&run.creates().len()), "{}", run.diag());
}

#[test]
fn invalid_or_oversized_budget_settings_fall_back_to_defaults() {
    for bad in [
        "0",
        "-5",
        "abc",
        "1234567",
        "99999999999999999999",
        "3.5",
        " ",
    ] {
        let run = run_step(
            "",
            TITLE,
            &[
                ("RECIPE_VAR_create_issue", "true"),
                ("GH_CREATE_SEQ", "secondary"),
                ("AMPLIHACK_ISSUE_BACKOFF_BASE_SECS", bad),
                ("AMPLIHACK_ISSUE_BACKOFF_MAX_DELAY_SECS", bad),
                ("AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS", bad),
                ("AMPLIHACK_ISSUE_BACKOFF_SECONDARY_MIN_SECS", bad),
            ],
        );
        run.assert_local();
        assert!(
            run.stderr()
                .contains("AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS=300"),
            "{bad:?} must fall back to the 300s default\n{}",
            run.diag()
        );
        let sleeps = run.sleeps();
        assert!(sleeps.iter().sum::<u64>() <= 300, "{bad:?}\n{}", run.diag());
        assert!(
            sleeps.iter().all(|s| (61..=120).contains(s)),
            "{bad:?}: the 60s floor plus a jitter delay of at most 60\n{}",
            run.diag()
        );
    }
}

#[test]
fn retry_after_is_honoured_uncapped_and_unjittered() {
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_CREATE_SEQ", "retry:11 ok"),
            ("AMPLIHACK_ISSUE_BACKOFF_MAX_DELAY_SECS", "4"),
        ],
    );
    run.assert_created();
    assert_eq!(
        run.sleeps(),
        vec![11],
        "Retry-After wins over the cap\n{}",
        run.diag()
    );
}

#[test]
fn retry_after_zero_is_raised_to_one() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "retry:0 ok")]);
    run.assert_created();
    assert_eq!(run.sleeps(), vec![1], "{}", run.diag());
}

#[test]
fn retry_after_longer_than_budget_gives_up_without_sleeping() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "retry:50")]);
    run.assert_local();
    assert!(
        run.sleeps().is_empty(),
        "never shortened to fit, never slept\n{}",
        run.diag()
    );
    assert_eq!(run.creates().len(), 1, "{}", run.diag());
    assert!(run.stderr().contains("gave up after 0s"), "{}", run.diag());
}

#[test]
fn backoff_lookup_lists_recent_issues_without_search() {
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_CREATE_SEQ", "secondary ok"),
            ("GH_LIST_SEQ", "default"),
        ],
    );
    run.assert_created();
    let lists = run.lists();
    assert_eq!(lists.len(), 2, "{}", run.diag());
    assert!(
        flag_value(&lists[0], "--search").is_some(),
        "the first lookup searches by title\n{}",
        run.diag()
    );
    assert!(
        !lists[1].iter().any(|a| a.starts_with("--search")),
        "the backoff lookup must not depend on the search index: {:?}\n{}",
        lists[1],
        run.diag()
    );
    assert!(
        flag_value(&lists[1], "--state").as_deref() == Some("open"),
        "{:?}",
        lists[1]
    );
}

/// #1881 review: the requirement is the same title in the same repository.
/// The gh account on a host switches routinely, so who opened the issue does
/// not matter.
#[test]
fn same_title_opened_by_another_account_is_reused() {
    let theirs = format!("{REPO_URL}/issues/1700");
    let json = serde_json::json!([
        { "number": 1700, "title": TITLE, "url": theirs, "author": { "login": "someone-else" } },
    ])
    .to_string();
    let run = run_opted_in(TITLE, &[("GH_LIST_JSON", &json)]);
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), theirs, "{}", run.diag());
    assert!(run.creates().is_empty(), "{}", run.diag());
}

#[test]
fn repository_is_fetched_once_per_run_and_the_login_never() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "secondary secondary ok")]);
    run.assert_created();
    assert_eq!(run.lists().len(), 3, "{}", run.diag());
    assert_eq!(run.calls_of("repo", "view").len(), 1, "{}", run.diag());
    assert!(run.calls_of("api", "user").is_empty(), "{}", run.diag());
}

/// A GitHub App installation token cannot read `/user`; lookups must not
/// depend on it.
#[test]
fn token_that_cannot_read_user_still_dedupes() {
    let found = format!("{REPO_URL}/issues/1700");
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_API_USER_EXIT", "1"),
            ("GH_LIST_JSON", &exact_list_json(TITLE, &found)),
        ],
    );
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), found, "{}", run.diag());
    assert!(run.creates().is_empty(), "{}", run.diag());
    assert!(!run.stderr().contains("WARNING"), "{}", run.diag());
}

#[test]
fn timed_out_create_reuses_an_issue_the_lookup_finds() {
    let found = format!("{REPO_URL}/issues/1777");
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_CREATE_SEQ", "timeout ok"),
            ("GH_LIST_SEQ", "default found"),
            ("GH_LIST_FOUND_JSON", &exact_list_json(TITLE, &found)),
        ],
    );
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), found, "{}", run.diag());
    assert_eq!(run.creates().len(), 1, "{}", run.diag());
    let lists = run.lists();
    assert_eq!(lists.len(), 2, "{}", run.diag());
    assert!(
        !lists[1].iter().any(|a| a.starts_with("--search")),
        "the lookup after a timeout lists recent issues: {:?}",
        lists[1]
    );
    assert!(run.sleeps().is_empty(), "{}", run.diag());
}

#[test]
fn timed_out_create_without_a_match_fails_without_retrying() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "timeout ok")]);
    run.assert_exit(1);
    assert!(run.stdout().is_empty(), "{}", run.diag());
    assert_eq!(
        run.creates().len(),
        1,
        "no retry after a timeout\n{}",
        run.diag()
    );
    assert_eq!(run.lists().len(), 2, "{}", run.diag());
    assert!(
        run.stderr().contains("timed out after 60s"),
        "{}",
        run.diag()
    );
}

// ===========================================================================
// AC5: a failed lookup during backoff makes no create that round
// ===========================================================================

#[test]
fn failed_lookup_during_backoff_skips_the_create_that_round() {
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_CREATE_SEQ", "secondary ok"),
            ("GH_LIST_SEQ", "default fail default"),
        ],
    );
    run.assert_created();
    assert_eq!(
        run.creates().len(),
        2,
        "exactly one more create after the limit\n{}",
        run.diag()
    );
    assert_eq!(run.lists().len(), 3, "{}", run.diag());
    assert_eq!(
        run.sleeps().len(),
        2,
        "the failed lookup costs one more wait\n{}",
        run.diag()
    );
    assert!(
        run.stderr().contains("WARNING: exact-title lookup failed"),
        "{}",
        run.diag()
    );
}

#[test]
fn persistently_failing_lookup_during_backoff_never_creates_blind() {
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_CREATE_SEQ", "secondary ok"),
            ("GH_LIST_SEQ", "default fail"),
        ],
    );
    run.assert_local();
    assert_eq!(
        run.creates().len(),
        1,
        "no create without a successful lookup\n{}",
        run.diag()
    );
    assert!(run.sleeps().iter().sum::<u64>() <= 30, "{}", run.diag());
    assert!(run.stderr().contains("gave up after"), "{}", run.diag());
}

// ===========================================================================
// AC6: primary limit waits for reset within the same budget
// ===========================================================================

#[test]
fn primary_limit_waits_for_reset_then_retries() {
    let run = run_opted_in(
        TITLE,
        &[("GH_CREATE_SEQ", "primary ok"), ("GH_RESET_IN", "10")],
    );
    run.assert_created();
    let sleeps = run.sleeps();
    assert_eq!(sleeps.len(), 1, "{}", run.diag());
    assert!(
        (11..=14).contains(&sleeps[0]),
        "reset + 3s buffer\n{}",
        run.diag()
    );
    assert_eq!(run.creates().len(), 2, "{}", run.diag());
    assert_eq!(
        run.lists().len(),
        2,
        "lookup before the retry\n{}",
        run.diag()
    );
    assert!(run.stderr().contains("API rate limit"), "{}", run.diag());
}

#[test]
fn primary_reset_beyond_budget_gives_up_without_sleeping() {
    let run = run_opted_in(
        TITLE,
        &[("GH_CREATE_SEQ", "primary"), ("GH_RESET_IN", "1000")],
    );
    run.assert_local();
    assert!(run.sleeps().is_empty(), "{}", run.diag());
    assert_eq!(run.creates().len(), 1, "{}", run.diag());
}

#[test]
fn primary_with_unusable_reset_waits_the_minimum() {
    for epoch in ["garbage", "1234567890123", "1"] {
        let run = run_opted_in(
            TITLE,
            &[("GH_CREATE_SEQ", "primary ok"), ("GH_RESET_EPOCH", epoch)],
        );
        run.assert_created();
        assert_eq!(run.sleeps(), vec![5], "epoch {epoch:?}\n{}", run.diag());
    }
}

#[test]
fn persistent_primary_limit_is_bounded() {
    let run = run_opted_in(
        TITLE,
        &[("GH_CREATE_SEQ", "primary"), ("GH_RESET_EPOCH", "garbage")],
    );
    run.assert_local();
    let sleeps = run.sleeps();
    assert!(
        !sleeps.is_empty() && sleeps.iter().sum::<u64>() <= 30,
        "{}",
        run.diag()
    );
}

// ===========================================================================
// AC7: auth, permission, not-found and label errors
// ===========================================================================

#[test]
fn auth_failure_is_not_retried() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "auth")]);
    run.assert_exit(1);
    assert!(
        run.stderr()
            .contains("ERROR: GitHub issue creation failed."),
        "{}",
        run.diag()
    );
    assert_eq!(run.creates().len(), 1, "exactly one create\n{}", run.diag());
    assert!(run.sleeps().is_empty(), "{}", run.diag());
    assert_eq!(
        run.lists().len(),
        1,
        "no lookup after a 401\n{}",
        run.diag()
    );
    assert!(
        run.calls_of("api", "rate_limit").is_empty(),
        "{}",
        run.diag()
    );
    assert!(
        !run.stdout().contains("tracking_system=local"),
        "{}",
        run.diag()
    );
}

#[test]
fn permission_403_without_rate_limit_text_is_an_error_not_a_retry() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "forbidden ok")]);
    run.assert_exit(1);
    assert_eq!(run.creates().len(), 1, "{}", run.diag());
    assert!(run.sleeps().is_empty(), "{}", run.diag());
}

#[test]
fn unexpected_failure_is_an_error_with_sanitised_output() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "other")]);
    run.assert_exit(1);
    let stderr = run.stderr();
    assert!(
        stderr.contains("ERROR: GitHub issue creation failed."),
        "{}",
        run.diag()
    );
    assert!(
        stderr.contains("unexpected create failure"),
        "{}",
        run.diag()
    );
    assert!(
        !stderr.contains("ghp_secret123"),
        "credentials must be redacted\n{}",
        run.diag()
    );
    assert!(!stderr.contains(BODY_MARKER), "{}", run.diag());
    assert!(run.stdout().trim().is_empty(), "{}", run.diag());
}

#[test]
fn unresolvable_repository_falls_back_to_local_tracking() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "notfound")]);
    run.assert_local();
    assert_eq!(run.creates().len(), 1, "{}", run.diag());
    assert!(run.stderr().contains("WARNING"), "{}", run.diag());
}

#[test]
fn label_failure_retries_once_without_label() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "label ok")]);
    run.assert_created();
    let creates = run.creates();
    assert_eq!(creates.len(), 2, "{}", run.diag());
    assert!(has_label_flag(&creates[0]), "{:?}", creates[0]);
    assert!(!has_label_flag(&creates[1]), "{:?}", creates[1]);
    assert!(run.sleeps().is_empty(), "{}", run.diag());
}

#[test]
fn label_fallback_runs_at_most_once() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "label")]);
    assert_ne!(run.code(), Some(0), "{}", run.diag());
    assert_eq!(run.creates().len(), 2, "{}", run.diag());
}

#[test]
fn label_creation_failure_does_not_stop_the_create() {
    let run = run_opted_in(TITLE, &[("GH_LABEL_EXIT", "1")]);
    run.assert_created();
    assert_eq!(run.calls_of("label", "create").len(), 1, "{}", run.diag());
    assert!(
        run.stderr().contains("label create refused")
            && run.stderr().contains("<redacted-token>")
            && !run.stderr().contains("ghp_LABELFAKE"),
        "the label failure reason is shown, sanitised\n{}",
        run.diag()
    );
}

// ===========================================================================
// Output, prompts and body file
// ===========================================================================

#[test]
fn success_prints_only_the_last_valid_issue_url() {
    let run = run_opted_in(TITLE, &[("GH_CREATE_SEQ", "noisy")]);
    run.assert_exit(0);
    assert_eq!(run.stdout(), format!("{CREATED_URL}\n"), "{}", run.diag());
}

#[test]
fn every_gh_lookup_and_write_runs_with_prompts_disabled() {
    let run = run_opted_in(
        TITLE,
        &[
            ("GH_CREATE_SEQ", "primary secondary ok"),
            ("GH_RESET_IN", "1"),
        ],
    );
    run.assert_created();
    for (a, b) in [
        ("issue", "list"),
        ("label", "create"),
        ("issue", "create"),
        ("api", "rate_limit"),
    ] {
        assert!(
            !run.calls_of(a, b).is_empty(),
            "expected a `gh {a} {b}` call\n{}",
            run.diag()
        );
    }
    assert_eq!(
        run.sandbox.read("gh.prompt"),
        "",
        "calls made without GH_PROMPT_DISABLED=1\n{}",
        run.diag()
    );
}

#[test]
fn body_file_is_private_and_removed_after_the_step() {
    let run = run_opted_in(TITLE, &[]);
    run.assert_created();
    assert_eq!(
        run.sandbox.read("state/body_modes").trim(),
        "600",
        "{}",
        run.diag()
    );
    for p in run.sandbox.read("state/body_paths").lines() {
        assert!(!Path::new(p).exists(), "body file {p} must be removed");
    }
    let create = &run.creates()[0];
    assert!(
        create.iter().any(|a| a.starts_with("--body-file=")),
        "{create:?}"
    );
}

#[test]
fn missing_issue_create_helper_exits_two() {
    let sandbox = Sandbox::new();
    let home = sandbox.path("fakehome");
    let tools = home.join("amplifier-bundle").join("tools");
    fs::create_dir_all(&tools).unwrap();
    for f in fs::read_dir(tools_dir()).unwrap() {
        let f = f.unwrap();
        let name = f.file_name();
        if name != "workflow_issue_create.sh" && f.path().is_file() {
            fs::copy(f.path(), tools.join(&name)).unwrap();
        }
    }
    let output = sandbox
        .bash(&step_body("step-03-create-issue"))
        .env("AMPLIHACK_HOME", &home)
        .env("RECIPE_VAR_remote_host_type", "github")
        .env("RECIPE_VAR_issue_number", "")
        .env("RECIPE_VAR_create_issue", "true")
        .env("TASK_DESCRIPTION", TITLE)
        .output()
        .unwrap();
    let run = Run { output, sandbox };
    run.assert_exit(2);
    assert!(
        run.stderr()
            .contains("ERROR: issue creation helper not found"),
        "{}",
        run.diag()
    );
    assert!(run.creates().is_empty(), "{}", run.diag());
}

// ===========================================================================
// Helper unit tests (sourced directly)
// ===========================================================================

#[test]
fn helper_issue_creation_opted_in_table() {
    let cases = [
        ("true", 0),
        ("TRUE", 0),
        ("1", 0),
        ("yes", 0),
        (" Yes ", 0),
        ("", 1),
        ("false", 1),
        ("0", 1),
        ("no", 1),
        ("on", 1),
        ("y", 1),
        ("tru", 1),
        ("true!", 1),
    ];
    for (value, rc) in cases {
        let run = run_helper(
            "rc=0; issue_creation_opted_in \"$V\" || rc=$?; echo \"rc=$rc\"",
            &[("V", value)],
        );
        run.assert_exit(0);
        assert_eq!(
            run.stdout().trim(),
            format!("rc={rc}"),
            "{value:?}\n{}",
            run.diag()
        );
        assert!(run.gh_log().is_empty(), "{}", run.diag());
    }
}

#[test]
fn helper_classify_issue_create_error_table() {
    let cases = [
        (
            "HTTP 401: Bad credentials (https://api.github.com/graphql)",
            "auth",
        ),
        (
            "gh: To get started with GitHub CLI, please run: gh auth login; not logged in",
            "auth",
        ),
        (
            "HTTP 401: requires authentication; API rate limit exceeded",
            "auth",
        ),
        (
            "HTTP 403: You have exceeded a secondary rate limit.",
            "secondary",
        ),
        ("HTTP 429: Secondary Rate Limit", "secondary"),
        (
            "HTTP 403: You have triggered an abuse detection mechanism.",
            "secondary",
        ),
        (
            "GraphQL: You have exceeded a secondary rate limit",
            "secondary",
        ),
        (
            "GraphQL: was submitted too quickly (createIssue)",
            "secondary",
        ),
        ("GraphQL: API rate limit exceeded for user ID 1.", "primary"),
        ("API rate limit already exceeded for user ID 1.", "primary"),
        ("HTTP 429: Too Many Requests", "primary"),
        (
            "GraphQL: Could not resolve to a Repository with the name 'a/b'.",
            "not_found",
        ),
        ("HTTP 404: Not Found", "not_found"),
        ("could not add label: 'workflow:default' not found", "label"),
        ("HTTP 403: Resource not accessible by integration", "other"),
        ("HTTP 403", "other"),
        ("HTTP 422: Validation Failed (Label)", "other"),
        ("issue 4011 failed to save", "other"),
        ("something else entirely", "other"),
        ("", "other"),
    ];
    for (out, class) in cases {
        let run = run_helper("classify_issue_create_error \"$OUT\"", &[("OUT", out)]);
        run.assert_exit(0);
        assert_eq!(run.stdout().trim(), class, "{out:?}\n{}", run.diag());
    }
}

#[test]
fn helper_classify_treats_output_as_text_not_a_pattern() {
    let run = run_helper(
        "classify_issue_create_error \"$OUT\"",
        &[("OUT", "-e .* [[ ( rate")],
    );
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), "other", "{}", run.diag());
    let big = format!("{}secondary rate limit 403", "x".repeat(70_000));
    let run = run_helper("classify_issue_create_error \"$OUT\"", &[("OUT", &big)]);
    run.assert_exit(0);
    assert_eq!(
        run.stdout().trim(),
        "other",
        "only the first 65536 bytes are read\n{}",
        run.diag()
    );
}

#[test]
fn helper_backoff_int_validates_values() {
    for (value, expect) in [
        ("7", "7"),
        ("999999", "999999"),
        ("0", "42"),
        ("-1", "42"),
        ("abc", "42"),
        ("1234567", "42"),
        ("07", "42"),
        ("", "42"),
    ] {
        let run = run_helper(
            "_issue_backoff_int AMPLIHACK_ISSUE_BACKOFF_BASE_SECS 42",
            &[("AMPLIHACK_ISSUE_BACKOFF_BASE_SECS", value)],
        );
        run.assert_exit(0);
        assert_eq!(run.stdout().trim(), expect, "{value:?}\n{}", run.diag());
    }
    let secret = "ghp_notarealtokenbutlookslikeone";
    let run = run_helper(
        "_issue_backoff_int AMPLIHACK_ISSUE_BACKOFF_BASE_SECS 42",
        &[("AMPLIHACK_ISSUE_BACKOFF_BASE_SECS", secret)],
    );
    assert_eq!(run.stdout().trim(), "42", "{}", run.diag());
    assert!(
        run.stderr()
            .contains(&format!("({} characters)", secret.len()))
            && !run.stderr().contains("ghp_")
            && !run.stderr().contains("notarealtoken"),
        "only the length of an invalid value is printed\n{}",
        run.diag()
    );
    let run = run_helper(
        "_issue_backoff_int AMPLIHACK_ISSUE_BACKOFF_BASE_SECS 42",
        &[],
    );
    assert!(
        run.stderr().is_empty(),
        "an unset value is not a warning\n{}",
        run.diag()
    );
    assert_eq!(
        run.stdout().trim(),
        "42",
        "unset gives the default\n{}",
        run.diag()
    );
}

#[test]
fn helper_retry_after_parses_seconds_only() {
    for (out, expect) in [
        ("HTTP 403\nRetry-After: 12", "12"),
        ("retry-after: 3", "3"),
        ("RETRY-AFTER: 999999", "999999"),
        ("no header here", ""),
        ("retry-after: soon", ""),
    ] {
        let run = run_helper("_issue_retry_after \"$OUT\" || true", &[("OUT", out)]);
        run.assert_exit(0);
        assert_eq!(run.stdout().trim(), expect, "{out:?}\n{}", run.diag());
    }
}

#[test]
fn helper_jitter_delay_stays_within_equal_jitter_bounds() {
    // (base, cap): for every round the delay is in [max(1, d/2), d] where
    // d = min(cap, base * 2^min(round, 30)).
    let script = r#"
for spec in "5 60" "1 1" "2 8" "999999 999999"; do
  set -- $spec
  for round in 0 1 2 3 4 5 10 31 40 63; do
    for i in 1 2 3 4 5 6 7 8; do
      printf '%s %s %s %s\n' "$1" "$2" "$round" "$(_issue_jitter_delay "$round" "$1" "$2")"
    done
  done
done
"#;
    let run = run_helper(script, &[]);
    run.assert_exit(0);
    let mut seen_varied = false;
    let mut last: Option<(u64, u64, u64, u64)> = None;
    for line in run.stdout().lines() {
        let v: Vec<u64> = line
            .split_whitespace()
            .map(|x| {
                x.parse()
                    .unwrap_or_else(|_| panic!("bad line {line:?}\n{}", run.diag()))
            })
            .collect();
        let (base, cap, round, delay) = (v[0], v[1], v[2], v[3]);
        let exp = round.min(30) as u32;
        let d = cap.min(base.saturating_mul(2u64.saturating_pow(exp)));
        let lo = (d / 2).max(1);
        assert!(
            delay >= lo && delay <= d.max(1),
            "base {base} cap {cap} round {round}: delay {delay} outside [{lo}, {d}]"
        );
        if let Some((b, c, r, dl)) = last
            && b == base
            && c == cap
            && r == round
            && dl != delay
        {
            seen_varied = true;
        }
        last = Some((base, cap, round, delay));
    }
    assert!(
        seen_varied,
        "jitter must vary between calls\n{}",
        run.diag()
    );
}

#[test]
fn helper_primary_wait_uses_reset_with_buffer_and_minimum() {
    let run = run_helper("_issue_primary_wait", &[("GH_RESET_IN", "100")]);
    run.assert_exit(0);
    let w: u64 = run
        .stdout()
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{}", run.diag()));
    assert!((101..=104).contains(&w), "{}", run.diag());
    for epoch in ["garbage", "1", "1234567890123"] {
        let run = run_helper("_issue_primary_wait", &[("GH_RESET_EPOCH", epoch)]);
        run.assert_exit(0);
        assert_eq!(run.stdout().trim(), "5", "{epoch:?}\n{}", run.diag());
        assert_eq!(
            run.stderr()
                .contains("WARNING: could not read GitHub rate-limit reset; waiting 5s"),
            epoch != "1",
            "{epoch:?}: an unreadable reset warns; a past reset is just the minimum\n{}",
            run.diag()
        );
    }
}

#[test]
fn helper_find_open_issue_by_exact_title_return_codes() {
    let found = format!("{REPO_URL}/issues/1700");
    let call = "rc=0; out=$(find_open_issue_by_exact_title \"$T\") || rc=$?; printf 'rc=%s\\n%s' \"$rc\" \"$out\"";
    // (env, expected rc line, expected URL, warns)
    type Case<'a> = (Vec<(&'a str, String)>, &'a str, &'a str, bool);
    let cases: Vec<Case> = vec![
        (
            vec![("GH_LIST_JSON", exact_list_json(TITLE, &found))],
            "rc=0",
            found.as_str(),
            false,
        ),
        (vec![], "rc=1", "", false),
        (vec![("GH_LIST_SEQ", "fail".into())], "rc=2", "", true),
        (vec![("GH_LIST_SEQ", "garbage".into())], "rc=2", "", true),
        (vec![("GH_LIST_SEQ", "object".into())], "rc=2", "", true),
        (
            vec![
                ("GH_REPO_VIEW_EXIT", "1".into()),
                ("GH_LIST_JSON", exact_list_json(TITLE, &found)),
            ],
            "rc=2",
            "",
            true,
        ),
        (
            vec![
                ("GH_API_USER_EXIT", "1".into()),
                ("GH_LIST_JSON", exact_list_json(TITLE, &found)),
            ],
            "rc=0",
            found.as_str(),
            false,
        ),
    ];
    for (env, rc, url, warns) in cases {
        let mut e: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        e.push(("T", TITLE));
        let run = run_helper(call, &e);
        run.assert_exit(0);
        let out = run.stdout();
        let mut lines = out.lines();
        assert_eq!(lines.next(), Some(rc), "{env:?}\n{}", run.diag());
        assert_eq!(
            lines.collect::<Vec<_>>().join("\n"),
            url,
            "{env:?}\n{}",
            run.diag()
        );
        assert_eq!(
            run.stderr().contains("WARNING: exact-title lookup failed"),
            warns,
            "{env:?}\n{}",
            run.diag()
        );
    }
}

#[test]
fn helper_find_open_issue_before_create_return_codes() {
    let found = format!("{REPO_URL}/issues/1700");
    let call = "rc=0; out=$(find_open_issue_before_create \"$T\") || rc=$?; printf 'rc=%s\\n%s' \"$rc\" \"$out\"";
    // (GH_LIST_SEQ, expected rc line, expected URL, list calls)
    for (seq, rc, url, lists) in [
        ("found", "rc=0", found.as_str(), 1),
        ("default", "rc=1", "", 1),
        ("fail found", "rc=0", found.as_str(), 2),
        ("fail default", "rc=1", "", 2),
        ("fail fail", "rc=2", "", 2),
        ("garbage garbage", "rc=2", "", 2),
    ] {
        let run = run_helper(
            call,
            &[
                ("T", TITLE),
                ("GH_LIST_SEQ", seq),
                ("GH_LIST_FOUND_JSON", &exact_list_json(TITLE, &found)),
            ],
        );
        run.assert_exit(0);
        let out = run.stdout();
        let mut l = out.lines();
        assert_eq!(l.next(), Some(rc), "{seq}\n{}", run.diag());
        assert_eq!(
            l.collect::<Vec<_>>().join("\n"),
            url,
            "{seq}\n{}",
            run.diag()
        );
        let calls = run.lists();
        assert_eq!(calls.len(), lists, "{seq}\n{}", run.diag());
        if lists == 2 {
            assert!(
                flag_value(&calls[1], "--search").is_none(),
                "{seq}\n{}",
                run.diag()
            );
        }
    }
}

#[test]
fn helper_secondary_wait_adds_the_floor() {
    let run = run_helper(
        "for r in 0 1 2 3; do _issue_secondary_wait \"$r\" 5 60 60; done; _issue_wait_for_class secondary 'HTTP 403: secondary rate limit' 0 5 60",
        &[],
    );
    run.assert_exit(0);
    let v: Vec<u64> = run.stdout().lines().map(|l| l.parse().unwrap()).collect();
    assert_eq!(v.len(), 5, "{}", run.diag());
    for (got, (lo, hi)) in v
        .iter()
        .zip([(62, 65), (65, 70), (70, 80), (80, 100), (62, 65)])
    {
        assert!((lo..=hi).contains(got), "{v:?}\n{}", run.diag());
    }
}

// ===========================================================================
// Static checks
// ===========================================================================

#[test]
fn step_03_has_no_unbounded_loop_or_old_inline_create() {
    let body = step_body("step-03-create-issue");
    let code = code_only(&body);
    let while_re = Regex::new(r"\bwhile\b").unwrap();
    assert!(
        !while_re.is_match(&code),
        "step-03 must not contain a while loop (#1881)"
    );
    for old in [
        "try_gh_create",
        "gh_create_issue",
        "_issue_is_rate_limit",
        "_issue_is_auth",
        "_issue_wait_for_reset",
        "RL_WINDOWS",
        "no arbitrary cap",
    ] {
        assert!(
            !body.contains(old),
            "step-03 must drop the old inline {old}"
        );
    }
}

#[test]
fn step_03_calls_the_helpers_in_order_and_reads_only_the_recipe_var() {
    let body = step_body("step-03-create-issue");
    let code = code_only(&body);
    let pos = |needle: &str| {
        code.find(needle)
            .unwrap_or_else(|| panic!("step-03 code must contain {needle}"))
    };
    assert!(
        body.contains("workflow_issue_create.sh"),
        "step-03 must source the helper"
    );
    let order = [
        pos("HOST_TYPE"),
        pos("select_github_tracking_issue"),
        pos("issue_creation_opted_in"),
        pos("find_open_issue_before_create"),
        pos("create_github_issue_bounded"),
    ];
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "wrong order: {order:?}"
    );
    assert!(
        body.contains("RECIPE_VAR_create_issue"),
        "opt-in comes from RECIPE_VAR_create_issue"
    );
}

#[test]
fn workflow_prep_stays_within_its_brick_budget() {
    let lines = fs::read_to_string(recipe_file("workflow-prep"))
        .unwrap()
        .lines()
        .count();
    assert!(
        lines <= 400,
        "workflow-prep.yaml has {lines} lines; the limit is 400"
    );
}

#[test]
fn create_issue_defaults_to_false_in_recipe_contexts() {
    for name in ["workflow-prep", "default-workflow", "smart-orchestrator"] {
        let recipe = load_recipe(name);
        let value = recipe
            .get("context")
            .and_then(|c| c.get("create_issue"))
            .and_then(Value::as_str);
        assert_eq!(
            value,
            Some("false"),
            "{name}.yaml must declare create_issue: \"false\""
        );
    }
}

#[test]
fn helper_header_lists_every_function() {
    let text = create_helper_text();
    let header: String = text
        .lines()
        .take_while(|l| l.starts_with('#') || l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let fn_re = Regex::new(r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(\)\s*\{").unwrap();
    let names: Vec<String> = fn_re
        .captures_iter(&text)
        .map(|c| c[1].to_owned())
        .collect();
    for required in [
        "issue_creation_opted_in",
        "load_issue_identity",
        "find_open_issue_by_exact_title",
        "classify_issue_create_error",
        "create_github_issue_bounded",
        "_issue_backoff_int",
        "_issue_retry_after",
        "_issue_jitter_delay",
        "_issue_primary_wait",
    ] {
        assert!(
            names.iter().any(|n| n == required),
            "helper must define {required}"
        );
    }
    for n in &names {
        assert!(header.contains(n.as_str()), "helper header must list {n}");
    }
}

#[test]
fn helper_sets_no_options_and_uses_no_trap_eval_or_sh_c() {
    let code = code_only(&create_helper_text());
    for (re, what) in [
        (r"(?m)^\s*set\s+[-+]", "set options"),
        (r"\btrap\b", "trap"),
        (r"\beval\b", "eval"),
        (r"\b(ba)?sh\s+-c\b", "sh -c"),
        (r"\(\(\s*[A-Za-z_]+\+\+", "((x++)) under set -e"),
    ] {
        assert!(
            !Regex::new(re).unwrap().is_match(&code),
            "helper must not use {what}"
        );
    }
}

#[test]
fn helper_wraps_every_gh_call_with_timeout_no_prompt_and_null_stdin() {
    let text = create_helper_text();
    for literal in [
        "timeout 30 gh label create",
        "timeout 60 gh issue create",
        "timeout 30 gh issue list",
        "timeout 30 gh repo view",
        "timeout 30 gh api rate_limit",
    ] {
        assert!(text.contains(literal), "helper must contain `{literal}`");
    }
    assert!(
        !text.contains("gh api user"),
        "lookups must not depend on /user, which app tokens cannot read"
    );
    let code = code_only(&text);
    let call_re = Regex::new(r"timeout\s+\d+\s+gh\s").unwrap();
    for line in code.lines().filter(|l| call_re.is_match(l)) {
        assert!(
            line.contains("GH_PROMPT_DISABLED=1 timeout"),
            "gh call without GH_PROMPT_DISABLED=1: {line}"
        );
        assert!(
            line.contains("</dev/null"),
            "gh call without </dev/null: {line}"
        );
    }
    let bare_re = Regex::new(r"(^|[;&|(]\s*|\$\(\s*)gh\s+(issue|label|repo|api)\b").unwrap();
    for line in code.lines() {
        assert!(!bare_re.is_match(line), "gh call without timeout: {line}");
    }
    assert!(!text.contains("retrying once") && !text.contains("GH_RETRY_DELAY_SECONDS"));
}

/// #1881 review: a decomposed run hands its opt-in to every workstream;
/// multitask reads AMPLIHACK_CREATE_ISSUE into each workstream's context.
#[test]
fn parallel_launch_forwards_create_issue_to_workstreams() {
    let text = fs::read_to_string(recipe_file("smart-execute-routing")).unwrap();
    let start = text
        .find("id: \"launch-parallel-round-1\"")
        .expect("launch-parallel-round-1 step");
    let step = &text[start..];
    let step = &step[..step.find("output: \"round_1_result\"").unwrap()];
    assert!(
        step.contains("AMPLIHACK_CREATE_ISSUE=\"${RECIPE_VAR_create_issue:-}\""),
        "launch-parallel-round-1 must pass create_issue to `orch run`"
    );
}

// ===========================================================================
// strict_delivery (docs/reference/workflow-delivery-inputs.md)
// ===========================================================================

/// A strict run whose issue (`plan/tasks#42`) lives outside this checkout's
/// repository (`example-org/example-repo`, the fake origin).
const STRICT_URL: &str = "https://github.com/plan/tasks/issues/42";

fn strict_env<'a>(overrides: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut env: Vec<(&str, &str)> = vec![
        ("RECIPE_VAR_strict_delivery", "true"),
        ("RECIPE_VAR_repository", "plan/tasks"),
        ("RECIPE_VAR_pr_repository", "example-org/example-repo"),
        ("RECIPE_VAR_closing_reference", "plan/tasks#42"),
        ("RECIPE_VAR_existing_branch", "feat/plan-tasks-42"),
        ("RECIPE_VAR_create_issue", "false"),
        ("STRICT_REPO", "plan/tasks"),
        ("STRICT_NUMBER", "42"),
    ];
    for (k, v) in overrides {
        env.retain(|(key, _)| key != k);
        env.push((k, v));
    }
    env
}

/// `(issue_number, input overrides, expected reason)` for one refused run.
type StrictCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

fn run_strict(issue_number: &str, overrides: &[(&str, &str)]) -> Run {
    // The task text names another issue and asks for a new one; a strict run
    // must ignore both.
    run_step(
        issue_number,
        "Create another issue #999 for this",
        &strict_env(overrides),
    )
}

impl Run {
    /// Exit 2, nothing on stdout (no URL and no local tracking block), the
    /// reason on stderr, and no issue search, creation or label call.
    fn assert_strict_refusal(&self, reason: &str) {
        self.assert_exit(2);
        assert!(
            self.stdout().trim().is_empty(),
            "a refused strict run prints nothing for step-03b\n{}",
            self.diag()
        );
        assert!(
            self.stderr().contains(reason),
            "stderr must say {reason:?}\n{}",
            self.diag()
        );
        assert!(
            !self.stderr().contains("ghp_"),
            "gh's stderr must stay discarded\n{}",
            self.diag()
        );
        self.assert_no_gh_write_or_search();
        assert!(
            self.calls_of("issue", "view").is_empty(),
            "a strict run never looks up a task-text or local issue\n{}",
            self.diag()
        );
    }
}

#[test]
fn strict_delivery_confirms_the_owning_repository_issue_with_one_exact_read() {
    let run = run_strict("42", &[]);
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), STRICT_URL, "{}", run.diag());
    let calls = run.gh_calls();
    assert_eq!(calls.len(), 1, "exactly one gh call\n{}", run.diag());
    let call = &calls[0];
    assert_eq!(
        call[..5],
        [
            "api",
            "--hostname",
            "github.com",
            "repos/plan/tasks/issues/42",
            "--jq"
        ],
        "the read names the host and the owning repository's issue endpoint\n{}",
        run.diag()
    );
    assert!(
        call[5].contains("pull_request")
            && call[5].contains("state")
            && call[5].contains("html_url"),
        "the filter must report a pull request, the issue's state and its URL: {:?}",
        call[5]
    );
    assert!(
        run.sandbox.read("gh.prompt").is_empty(),
        "the read runs with GH_PROMPT_DISABLED=1"
    );
    run.assert_no_gh_write_or_search();
    assert_eq!(
        step_03b_issue_number(&run, "Create another issue #999 for this"),
        "42",
        "step-03b keeps the caller's number"
    );
}

#[test]
fn strict_delivery_same_repository_issue_uses_a_bare_reference() {
    let run = run_strict(
        "42",
        &[
            ("RECIPE_VAR_repository", "example-org/example-repo"),
            ("RECIPE_VAR_closing_reference", "#42"),
            ("STRICT_REPO", "example-org/example-repo"),
        ],
    );
    run.assert_exit(0);
    assert_eq!(
        run.stdout().trim(),
        "https://github.com/example-org/example-repo/issues/42",
        "{}",
        run.diag()
    );
}

#[test]
fn strict_delivery_ignores_the_remote_host_type() {
    // base_branch no longer forces a host; strict_delivery reads GitHub
    // because its contract is GitHub's, whatever host detection said.
    for host in ["other", "azdo"] {
        let run = run_strict("42", &[("RECIPE_VAR_remote_host_type", host)]);
        run.assert_exit(0);
        assert_eq!(run.stdout().trim(), STRICT_URL, "{host}: {}", run.diag());
    }
}

#[test]
fn strict_delivery_fails_closed_when_the_issue_cannot_be_read() {
    let run = run_strict("42", &[("STRICT_ISSUE", "fail")]);
    run.assert_strict_refusal("could not read plan/tasks#42 on github.com (gh exit 1)");
    assert_eq!(run.calls_of("api", "--hostname").len(), 1);
}

#[test]
fn strict_delivery_rejects_a_pull_request_number() {
    run_strict("42", &[("STRICT_ISSUE", "pull")])
        .assert_strict_refusal("plan/tasks#42 on github.com is not an issue");
}

#[test]
fn strict_delivery_rejects_an_issue_that_answers_from_another_repository() {
    run_strict("42", &[("STRICT_ISSUE", "moved")])
        .assert_strict_refusal("plan/tasks#42 on github.com is not an issue");
}

#[test]
fn strict_delivery_rejects_a_closed_issue() {
    // A task closed (here as not planned) while it waited in the caller's
    // queue is not implemented, as on the ordinary path (#1595).
    let run = run_strict("42", &[("STRICT_ISSUE", "closed")]);
    run.assert_strict_refusal("plan/tasks#42 is closed, not open");
    assert_eq!(run.calls_of("api", "--hostname").len(), 1);
}

#[test]
fn strict_delivery_rejects_each_missing_or_contradictory_input_before_any_read() {
    let cases: &[StrictCase] = &[
        ("", &[], "issue_number must be the number"),
        ("#42", &[], "issue_number must be the number"),
        ("local-issue-42", &[], "issue_number must be the number"),
        (
            "42",
            &[("RECIPE_VAR_create_issue", "true")],
            "create_issue must not be set",
        ),
        (
            "42",
            &[("RECIPE_VAR_create_issue", " Yes ")],
            "create_issue must not be set",
        ),
        (
            "42",
            &[("RECIPE_VAR_repository", "")],
            "repository must be the owner/repo",
        ),
        (
            "42",
            &[("RECIPE_VAR_repository", "plan/../tasks")],
            "repository must be the owner/repo",
        ),
        (
            "42",
            &[("RECIPE_VAR_pr_repository", "")],
            "pr_repository must be the owner/repo",
        ),
        (
            "42",
            &[("RECIPE_VAR_pr_repository", "someone/else")],
            "pr_repository 'someone/else' is not this checkout's origin repository ('example-org/example-repo')",
        ),
        (
            "42",
            &[("RECIPE_VAR_closing_reference", "#42")],
            "closing_reference must be 'plan/tasks#42'",
        ),
        (
            "42",
            &[("RECIPE_VAR_closing_reference", "")],
            "closing_reference must be 'plan/tasks#42'",
        ),
        (
            "42",
            &[("RECIPE_VAR_existing_branch", "")],
            "strict_delivery=true requires existing_branch",
        ),
        (
            "42",
            &[("RECIPE_VAR_base_branch", "dev")],
            "base_branch 'dev' has no remote-tracking ref origin/dev",
        ),
        (
            "42",
            &[
                ("RECIPE_VAR_base_branch", "coordination/tasks"),
                (
                    "RECIPE_VAR_forbidden_base_branches",
                    "release/* coordination/*",
                ),
                ("GIT_ORIGIN_BRANCHES", "coordination/tasks"),
            ],
            "matches forbidden_base_branches pattern 'coordination/*'",
        ),
        (
            "42",
            &[
                ("RECIPE_VAR_base_branch", "feat/plan-tasks-42"),
                ("GIT_ORIGIN_BRANCHES", "feat/plan-tasks-42"),
            ],
            "is the branch being delivered",
        ),
    ];
    for (issue, overrides, reason) in cases {
        let run = run_strict(issue, overrides);
        run.assert_strict_refusal(reason);
        assert!(
            run.gh_calls().is_empty(),
            "issue={issue:?} {overrides:?}: inputs are checked before any gh call\n{}",
            run.diag()
        );
    }
}

#[test]
fn strict_delivery_accepts_an_existing_authorized_base() {
    let run = run_strict(
        "42",
        &[
            ("RECIPE_VAR_base_branch", "dev"),
            (
                "RECIPE_VAR_forbidden_base_branches",
                "release/* coordination/*",
            ),
            ("GIT_ORIGIN_BRANCHES", "main dev"),
        ],
    );
    run.assert_exit(0);
    assert_eq!(run.stdout().trim(), STRICT_URL, "{}", run.diag());
}

#[test]
fn strict_delivery_off_ignores_the_strict_inputs_and_base_branch() {
    // Neither base_branch nor the strict inputs change ordinary selection: on
    // a non-GitHub host step-03 still uses local tracking, and on GitHub the
    // explicit number is looked up in this repository as before.
    let mut env = strict_env(&[
        ("RECIPE_VAR_strict_delivery", "false"),
        ("RECIPE_VAR_base_branch", "dev"),
        ("RECIPE_VAR_remote_host_type", "other"),
    ]);
    let run = run_step("42", "Fix the thing", &env);
    run.assert_local();
    assert!(run.gh_calls().is_empty(), "{}", run.diag());

    env.retain(|(k, _)| *k != "RECIPE_VAR_remote_host_type");
    env.push(("GH_VIEW_ISSUE", "42"));
    let run = run_step("42", "Fix the thing", &env);
    run.assert_exit(0);
    assert_eq!(
        run.stdout().trim(),
        format!("{REPO_URL}/issues/42"),
        "{}",
        run.diag()
    );
    assert!(
        run.calls_of("api", "--hostname").is_empty(),
        "{}",
        run.diag()
    );
}
