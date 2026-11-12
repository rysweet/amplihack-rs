//! Issue #2010: when `amplihack codex -- exec …` adds the #1985 sandbox and
//! hook-trust flags, it writes one line to stderr naming what it added.
//! Nothing goes to stdout or to the `--output-last-message` file, and no line
//! is written when the caller chose a sandbox or nothing is injected.
//!
//! The launch reuses the #1985 helper: a fake Codex in an isolated temporary
//! home, built through `isolated_amplihack::isolated(..)`, that records its
//! argv, prints nothing, and writes a fixed message to the
//! `--output-last-message` path so the test can check the file is unchanged.
#![cfg(unix)]

use std::fs;

use crate::issue_1985_codex_agent_step_flags::{FAKE_FINAL_MESSAGE, Launch, launch};

const PREFIX: &str = "amplihack: codex exec:";
const NOTICE_ALL: &str = "amplihack: codex exec: added --sandbox danger-full-access, \
                          --dangerously-bypass-hook-trust (#1985)";
const HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";

fn stdout(launch: &Launch) -> String {
    String::from_utf8_lossy(&launch.out.stdout).into_owned()
}

fn stderr(launch: &Launch) -> String {
    String::from_utf8_lossy(&launch.out.stderr).into_owned()
}

fn notice_lines(launch: &Launch) -> Vec<String> {
    stderr(launch)
        .lines()
        .filter(|line| line.starts_with("amplihack: codex exec: added"))
        .map(str::to_owned)
        .collect()
}

/// (a) An agent step that chose no sandbox gets exactly one line on stderr,
/// with the full two-item text (#2317), and nothing on stdout or in the output file.
#[test]
fn exec_launch_prints_one_notice_on_stderr_only() {
    let tmp = tempfile::tempdir().unwrap();
    let final_message = tmp.path().join("final-message");
    let final_path = final_message.to_str().unwrap();
    let launch = launch(
        &["exec", "--output-last-message", final_path, "-"],
        b"do the step\n",
    );
    assert!(launch.out.status.success(), "{:?}", launch.out);

    assert_eq!(
        notice_lines(&launch),
        vec![NOTICE_ALL.to_string()],
        "stderr:\n{}",
        stderr(&launch)
    );
    assert!(
        !stdout(&launch).contains(PREFIX),
        "notice leaked to stdout:\n{}",
        stdout(&launch)
    );
    // The fake Codex writes a known message to this path. amplihack must not
    // append to it or replace it, so the file holds exactly that message.
    let written = fs::read_to_string(&final_message)
        .expect("fake codex writes the --output-last-message file");
    assert_eq!(
        written, FAKE_FINAL_MESSAGE,
        "--output-last-message file was altered"
    );

    // The line describes the argv Codex actually received.
    let at = |token: &str| launch.argv.iter().position(|a| a == token);
    let sandbox = at("--sandbox").expect("--sandbox injected");
    assert_eq!(
        launch.argv.get(sandbox + 1).map(String::as_str),
        Some("danger-full-access")
    );
    assert!(
        !launch
            .argv
            .iter()
            .any(|a| a.contains("sandbox_workspace_write")),
        "no network override may be added: {:?}",
        launch.argv
    );
    assert!(
        !stderr(&launch).contains("network access"),
        "{}",
        stderr(&launch)
    );
    assert!(at(HOOK_TRUST).is_some(), "{:?}", launch.argv);
}

/// (b) No line when the caller chose a sandbox (hook trust is still added,
/// silently; #2048 tracks reporting it), when the argv is empty, or when
/// there is no `exec`.
#[test]
fn no_notice_when_caller_chose_sandbox_or_nothing_injected() {
    let chose = launch(&["exec", "--sandbox", "read-only", "-"], b"prompt\n");
    assert!(chose.out.status.success(), "{:?}", chose.out);
    assert!(
        chose.argv.iter().any(|a| a == HOOK_TRUST),
        "hook trust must still be injected: {:?}",
        chose.argv
    );
    assert!(
        !chose.argv.iter().any(|a| a == "danger-full-access"),
        "{:?}",
        chose.argv
    );

    let bare = launch(&[], b"");
    assert!(bare.out.status.success(), "{:?}", bare.out);

    let resumed = launch(&["resume", "--last"], b"");
    assert!(resumed.out.status.success(), "{:?}", resumed.out);

    for (name, run) in [
        ("caller sandbox", &chose),
        ("bare", &bare),
        ("resume", &resumed),
    ] {
        assert!(
            !stderr(run).contains(PREFIX),
            "{name}: unexpected notice on stderr:\n{}",
            stderr(run)
        );
        assert!(
            !stdout(run).contains(PREFIX),
            "{name}: unexpected notice on stdout:\n{}",
            stdout(run)
        );
    }
}
