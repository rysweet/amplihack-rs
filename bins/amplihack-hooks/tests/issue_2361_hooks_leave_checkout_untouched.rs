//! Issue #2361: hooks started by a recipe step never write runtime state into
//! a Git checkout, even when `AMPLIHACK_HOME` points at one.
//!
//! `recipe run -w <checkout>` exports `AMPLIHACK_HOME=<checkout>` so that
//! steps find the bundle. Before the fix the hooks used that value as their
//! state root and wrote `runtime/sessions/<id>/...` at the root of the
//! checkout, where the recipe stagers picked it up.
//!
//! Each test builds a temporary main repository with an `amplifier-bundle/`
//! directory and a linked worktree, runs the real `amplihack-hooks` binary to
//! completion, and compares `git ls-files --stage` and
//! `git --no-optional-locks status --porcelain --untracked-files=all` before
//! and after. A positive control checks that the state was written to
//! `$HOME/.amplihack` instead, so the comparison cannot pass merely because
//! nothing was written.
#![cfg(unix)]

#[path = "../../../tests/support/isolated_hook.rs"]
mod isolated_hook;

use isolated_hook::HookSandbox;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

const SESSION: &str = "test-session";

const POST_TOOL_USE_INPUT: &str = r#"{
    "hook_event_name": "PostToolUse",
    "tool_name": "Bash",
    "tool_input": {"command": "echo hello"},
    "tool_response": {"output": "hello\n"},
    "session_id": "test-session"
}"#;

const PRE_COMPACT_INPUT: &str = r#"{
    "hook_event_name": "PreCompact",
    "session_id": "test-session"
}"#;

fn hooks_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_amplihack-hooks"))
}

/// Run git in `dir` through `amplihack_git::command_in`, with no system or
/// global configuration, an explicit identity, no signing and no hooks.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args([
            "-c",
            "user.name=issue-2361",
            "-c",
            "user.email=issue-2361@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A main repository holding `amplifier-bundle/` and one commit, and a
/// linked worktree of it on `step-branch`.
fn repo_with_worktree(root: &Path) -> (PathBuf, PathBuf) {
    let main = root.join("main");
    let wt = root.join("wt");
    fs::create_dir_all(main.join("amplifier-bundle")).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    fs::write(main.join("README.md"), "base\n").unwrap();
    fs::write(main.join("amplifier-bundle").join("BUNDLE.md"), "bundle\n").unwrap();
    git(&main, &["add", "README.md", "amplifier-bundle/BUNDLE.md"]);
    git(&main, &["commit", "-q", "-m", "base"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "step-branch",
            wt.to_str().unwrap(),
        ],
    );
    assert!(
        main.join(".git").is_dir(),
        "the main checkout has a .git directory"
    );
    assert!(
        wt.join(".git").is_file(),
        "a linked worktree has a .git file"
    );
    (main, wt)
}

/// The two read-only views of a checkout that the review-dir stamp is built
/// from.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    stage: String,
    status: String,
}

fn snapshot(checkout: &Path) -> Snapshot {
    Snapshot {
        stage: git(checkout, &["ls-files", "--stage"]),
        status: git(
            checkout,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--untracked-files=all",
            ],
        ),
    }
}

fn assert_untouched(checkout: &Path, before: &Snapshot, what: &str) {
    let after = snapshot(checkout);
    assert_eq!(
        &after,
        before,
        "{what} changed the checkout {}",
        checkout.display()
    );
    for dir in ["runtime", "private"] {
        assert!(
            !checkout.join(dir).exists(),
            "{what} created {}/ in the checkout {}",
            dir,
            checkout.display()
        );
    }
}

/// Run one hook to completion with `amplihack_home` as `AMPLIHACK_HOME` and
/// `cwd` as the working directory and `CLAUDE_PROJECT_DIR`.
fn run_hook(
    sandbox: &HookSandbox,
    subcommand: &str,
    input: &str,
    amplihack_home: &Path,
    cwd: &Path,
) {
    let mut child = sandbox
        .command_in(hooks_bin(), cwd)
        .env("AMPLIHACK_HOME", amplihack_home)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_BLARIFY_MODE", "skip")
        .env("AMPLIHACK_DISABLE_BLARIFY", "1")
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn amplihack-hooks");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait for amplihack-hooks");
    assert!(
        out.status.success(),
        "{subcommand} must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str::<serde_json::Value>(&stdout)
        .unwrap_or_else(|e| panic!("{subcommand} must print JSON ({e}): {stdout:?}"));
}

fn run_both_hooks(sandbox: &HookSandbox, amplihack_home: &Path, cwd: &Path) {
    run_hook(
        sandbox,
        "post-tool-use",
        POST_TOOL_USE_INPUT,
        amplihack_home,
        cwd,
    );
    run_hook(
        sandbox,
        "pre-compact",
        PRE_COMPACT_INPUT,
        amplihack_home,
        cwd,
    );
}

/// Positive control: the state went to `$HOME/.amplihack`.
fn assert_state_under_home(sandbox: &HookSandbox) {
    let session = sandbox
        .home()
        .join(".amplihack")
        .join("runtime")
        .join("sessions")
        .join(SESSION);
    assert!(
        session.is_dir(),
        "the hooks must write their state under {}",
        session.display()
    );
    assert!(
        session
            .join("metrics")
            .join("post_tool_use_metrics.jsonl")
            .is_file(),
        "post-tool-use must record its metric under {}",
        session.display()
    );
}

#[test]
fn hooks_in_a_linked_worktree_leave_the_main_checkout_untouched() {
    let sandbox = HookSandbox::new();
    let (main, wt) = repo_with_worktree(sandbox.root());
    let before = snapshot(&main);

    // As in a recipe step: AMPLIHACK_HOME is the -w checkout and the step
    // runs in a linked worktree of it.
    run_both_hooks(&sandbox, &main, &wt);

    assert_untouched(&main, &before, "post-tool-use and pre-compact");
    assert_state_under_home(&sandbox);
}

#[test]
fn hooks_with_a_linked_worktree_as_amplihack_home_leave_it_untouched() {
    let sandbox = HookSandbox::new();
    let (main, wt) = repo_with_worktree(sandbox.root());
    let main_before = snapshot(&main);
    let wt_before = snapshot(&wt);

    // AMPLIHACK_HOME is a linked worktree, whose .git is a file.
    run_both_hooks(&sandbox, &wt, &sandbox.project());

    assert_untouched(&wt, &wt_before, "post-tool-use and pre-compact");
    assert_untouched(&main, &main_before, "post-tool-use and pre-compact");
    assert_state_under_home(&sandbox);
}

#[test]
fn hooks_still_use_an_amplihack_home_that_is_not_a_checkout() {
    let sandbox = HookSandbox::new();
    let amplihack_home = sandbox.amplihack_home();

    run_both_hooks(&sandbox, &amplihack_home, &sandbox.project());

    let session = amplihack_home
        .join("runtime")
        .join("sessions")
        .join(SESSION);
    assert!(
        session
            .join("metrics")
            .join("post_tool_use_metrics.jsonl")
            .is_file(),
        "an AMPLIHACK_HOME that is not a checkout stays the state root"
    );
    assert!(
        !sandbox.home().join(".amplihack").exists(),
        "$HOME/.amplihack must not be used when AMPLIHACK_HOME is usable"
    );
}
