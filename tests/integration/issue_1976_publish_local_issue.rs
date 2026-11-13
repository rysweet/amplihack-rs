//! Issue #1976 (R7, D7, AC6): step-16 (`workflow_publish_pr.sh`) publishes a
//! local tracking reference as "no linked issue" instead of failing on a
//! non-numeric `issue_number`.
//!
//! Contract (docs/recipes/issue-815-804-local-tracking-issue-number.md §3):
//! - all digits: unchanged (`(#N)` title suffix, `workflow_issue_reference.sh`
//!   keyword, claim check);
//! - empty, or `^local-[A-Za-z0-9._:-]+$` with at most 128 characters after
//!   `local-`: no linked issue: no `(#N)`, `## Issue` reads
//!   ``No linked issue (local tracking reference `<v>`).`` (or
//!   `No linked issue.` when empty), no reference helper, no claim check, and
//!   neither title nor body contains `#local` or `Closes`;
//! - anything else: `finish_publish FAILED_INVALID_INPUT ... 1`, with the
//!   value cut to 64 characters and control characters removed; no PR.
//!
//! The repo has a bare-repo remote holding `main`; `origin`'s fetch URL is
//! github-shaped (step-16 derives the repo identity from it) and its push URL
//! is the bare repo, so nothing leaves this machine. `gh`, the claim-check
//! helper and the issue-reference helper are stubs that log every call.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn tool(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("tools")
        .join(name)
}

fn git(dir: &Path, args: &[&str]) {
    let mut cmd = amplihack_git::command_in(dir);
    cmd.args(args);
    crate::isolated_amplihack::no_git_prompt(&mut cmd);
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn executable(path: &Path, body: &str) {
    crate::exec_stub::write_executable(path, body, 0o755);
}

/// `gh`: logs argv; `pr list` finds nothing; `pr create` saves its title and
/// body and prints a PR URL.
const GH_STUB: &str = r#"#!/usr/bin/env bash
set -uo pipefail
printf '%s\n' "$*" >> "$STUB_LOG_DIR/gh.log"
case "${1:-} ${2:-}" in
  "pr list") printf '[]\n' ;;
  "pr view") exit 1 ;;
  "pr create")
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --body-file) cp "$2" "$STUB_LOG_DIR/pr-body.md"; shift ;;
        --title) printf '%s' "$2" > "$STUB_LOG_DIR/pr-title.txt"; shift ;;
      esac
      shift
    done
    echo "https://github.com/owner/repo/pull/99"
    ;;
  *) exit 0 ;;
esac
"#;

/// Claim check: logs argv and reports "not claimed".
const CLAIM_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$STUB_LOG_DIR/claim.log"
exit 0
"#;

/// Issue reference: logs the call and answers with the closing keyword, so a
/// local reference that reached it would show up as `Closes` in the body.
const REFERENCE_STUB: &str = r#"#!/usr/bin/env bash
cat >/dev/null
printf '%s\n' "$*" >> "$STUB_LOG_DIR/reference.log"
echo Closes
"#;

struct Fx {
    _tmp: TempDir,
    root: PathBuf,
    repo: PathBuf,
    bin: PathBuf,
    stubs: PathBuf,
}

struct Run {
    out: Output,
    logs: PathBuf,
}

impl Run {
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.logs.join(name)).unwrap_or_default()
    }
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }
    fn result(&self) -> serde_json::Value {
        let stdout = self.stdout();
        let line = stdout
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with('{'))
            .unwrap_or_else(|| {
                panic!(
                    "no publish result JSON\nstdout:\n{stdout}\nstderr:\n{}",
                    self.stderr()
                )
            });
        serde_json::from_str(line).expect("publish result is JSON")
    }
    fn pr_created(&self) -> bool {
        self.read("gh.log")
            .lines()
            .any(|l| l.starts_with("pr create"))
    }
}

impl Fx {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().canonicalize().unwrap();
        let bare = root.join("remote.git");
        let repo = root.join("repo");
        let bin = root.join("bin");
        let stubs = root.join("stubs");
        for d in [&repo, &bin, &stubs] {
            fs::create_dir_all(d).unwrap();
        }
        git(
            &root,
            &["init", "--bare", "-b", "main", bare.to_str().unwrap()],
        );
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Workflow Test"]);
        fs::write(repo.join("README.md"), "base\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "base"]);
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        );
        git(
            &repo,
            &["config", "remote.origin.pushurl", bare.to_str().unwrap()],
        );
        git(&repo, &["push", bare.to_str().unwrap(), "main:main"]);
        git(
            &repo,
            &[
                "fetch",
                bare.to_str().unwrap(),
                "+refs/heads/main:refs/remotes/origin/main",
            ],
        );
        git(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        git(&repo, &["switch", "-c", "feat/issue-1976-local"]);
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/feature.txt"), "feature\n").unwrap();
        git(&repo, &["add", "src/feature.txt"]);
        git(&repo, &["commit", "-m", "feature work"]);

        executable(&bin.join("gh"), GH_STUB);
        executable(&stubs.join("claim.sh"), CLAIM_STUB);
        executable(&stubs.join("reference.sh"), REFERENCE_STUB);
        Self {
            _tmp: tmp,
            root,
            repo,
            bin,
            stubs,
        }
    }

    fn publish(&self, label: &str, issue_number: &str) -> Run {
        let logs = self.root.join(format!("logs-{label}"));
        fs::create_dir_all(&logs).unwrap();
        let old_path = crate::amplihack_path::path_with_amplihack_bin();
        let mut cmd = Command::new("bash");
        cmd.arg(tool("workflow_publish_pr.sh"))
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", format!("{}:{old_path}", self.bin.display()))
            .env("HOME", &self.root)
            .env("TMPDIR", &self.root)
            .env("STUB_LOG_DIR", &logs)
            .env("REMOTE_HOST_TYPE", "github")
            .env("ISSUE_NUMBER", issue_number)
            .env(
                "WORKFLOW_RUNTIME_ARTIFACT_HELPER",
                tool("workflow_runtime_artifacts.sh"),
            )
            .env(
                "WORKFLOW_ISSUE_CLAIM_CHECK_HELPER",
                self.stubs.join("claim.sh"),
            )
            .env(
                "WORKFLOW_ISSUE_REFERENCE_HELPER",
                self.stubs.join("reference.sh"),
            );
        crate::isolated_amplihack::no_git_prompt(&mut cmd);
        let out = cmd.output().expect("run workflow_publish_pr.sh");
        Run { out, logs }
    }
}

fn assert_no_link(run: &Run, value: &str) {
    let ctx = format!(
        "issue_number={value:?}\nstdout:\n{}\nstderr:\n{}",
        run.stdout(),
        run.stderr()
    );
    assert_eq!(run.out.status.code(), Some(0), "{ctx}");
    assert!(run.pr_created(), "a PR must be created: {ctx}");
    assert_eq!(run.result()["terminal_status"], "success", "{ctx}");

    let title = run.read("pr-title.txt");
    let body = run.read("pr-body.md");
    for text in [&title, &body] {
        assert!(!text.contains("#local"), "{ctx}\n{text}");
        assert!(!text.contains("Closes"), "{ctx}\n{text}");
    }
    assert!(
        !title.contains("(#"),
        "no (#N) title suffix: {title:?} {ctx}"
    );
    let expected = if value.is_empty() {
        "No linked issue.".to_string()
    } else {
        format!("No linked issue (local tracking reference `{value}`).")
    };
    assert!(
        body.contains(&format!("## Issue\n{expected}")),
        "the Issue section says there is no linked issue:\n{body}\n{ctx}"
    );
    assert!(
        run.read("reference.log").is_empty(),
        "workflow_issue_reference.sh must not run: {ctx}"
    );
    assert!(
        run.read("claim.log").is_empty(),
        "the claim check must be skipped: {ctx}"
    );
}

fn assert_invalid(run: &Run, value: &str) {
    let ctx = format!(
        "issue_number={value:?}\nstdout:\n{}\nstderr:\n{}",
        run.stdout(),
        run.stderr()
    );
    assert_eq!(run.out.status.code(), Some(1), "{ctx}");
    let result = run.result();
    assert_eq!(result["state"], "FAILED_INVALID_INPUT", "{ctx}");
    assert_eq!(result["terminal_status"], "failure", "{ctx}");
    let message = result["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("is neither numeric nor a local tracking id"),
        "{ctx}"
    );
    assert!(!run.pr_created(), "no PR may be created: {ctx}");
    assert!(run.read("claim.log").is_empty(), "no claim check: {ctx}");
    assert!(run.read("reference.log").is_empty(), "no reference: {ctx}");
}

#[test]
fn local_tracking_references_publish_with_no_linked_issue() {
    let fx = Fx::new();
    let max_len = format!("local-{}", "a".repeat(128));
    for (label, value) in [
        ("hash", "local-abc123def456"),
        ("issue", "local-issue-763"),
        ("ab", "local-ab-42"),
        ("legacy", "local-tracking:123"),
        ("dotted", "local-a.b_c-d"),
        ("max", max_len.as_str()),
        ("empty", ""),
    ] {
        assert_no_link(&fx.publish(label, value), value);
    }
}

#[test]
fn a_value_that_is_neither_numeric_nor_local_fails_invalid_input() {
    let fx = Fx::new();
    let too_long = format!("local-{}", "a".repeat(129));
    for (label, value) in [
        ("garbage", "garbage"),
        ("bare-prefix", "local-"),
        ("hash-sign", "local-a#b"),
        ("space", "local-a b"),
        ("backtick", "local-`id`"),
        ("too-long", too_long.as_str()),
        ("mixed", "12a"),
        ("hashed-number", "#123"),
        ("negative", "-1"),
        ("upper-prefix", "LOCAL-abc"),
    ] {
        assert_invalid(&fx.publish(label, value), value);
    }
}

#[test]
fn an_invalid_value_is_truncated_and_stripped_of_control_characters() {
    let fx = Fx::new();
    let value = format!("garbage\u{7}\u{1b}[31mred\nline2{}", "z".repeat(200));
    let run = fx.publish("control", &value);
    assert_invalid(&run, &value);
    let message = run.result()["message"].as_str().unwrap().to_string();
    assert!(
        !message.chars().any(char::is_control),
        "control characters must be removed: {message:?}"
    );
    let quoted = message
        .split_once('\'')
        .and_then(|(_, rest)| rest.rsplit_once("' is neither"))
        .map(|(v, _)| v)
        .unwrap_or_else(|| panic!("value is quoted in the message: {message:?}"));
    assert!(quoted.starts_with("garbage"), "{quoted:?}");
    assert!(
        quoted.chars().count() <= 64,
        "value cut to 64 characters: {} in {quoted:?}",
        quoted.chars().count()
    );
}

#[test]
fn a_numeric_issue_keeps_todays_behaviour() {
    let fx = Fx::new();
    let run = fx.publish("numeric", "123");
    let ctx = format!("stdout:\n{}\nstderr:\n{}", run.stdout(), run.stderr());
    assert_eq!(run.out.status.code(), Some(0), "{ctx}");
    assert!(run.pr_created(), "{ctx}");
    let title = run.read("pr-title.txt");
    assert!(
        title.ends_with("(#123)"),
        "title keeps the suffix: {title:?}"
    );
    let body = run.read("pr-body.md");
    assert!(
        body.contains("## Issue\nCloses #123"),
        "the reference helper's keyword links the issue:\n{body}"
    );
    assert!(
        !run.read("reference.log").is_empty(),
        "workflow_issue_reference.sh runs for a numeric issue"
    );
    assert!(
        run.read("claim.log").contains("--issue 123"),
        "the claim check runs for a numeric issue: {}",
        run.read("claim.log")
    );
}
