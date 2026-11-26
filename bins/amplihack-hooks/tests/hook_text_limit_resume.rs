//! `amplihack-hooks limit-resume` (the StopFailure hook) is outside
//! AMPLIHACK_HOOK_TEXT altogether. Under `none` it still resumes a session
//! stopped at a limit, by keys or by its exit-2 wake, exactly as it does with
//! the variable unset.
//!
//! Each case runs the hook with the variable unset first (the control), then
//! with `none`, `NONE` and an unknown value, each on a fresh rig from the
//! shared limit-resume harness, and compares exit code, stdout, stderr, the
//! `outcome=` and `detail=` of each `resume.log` line and the keys the
//! stand-in `tmux` received. Nothing reaches a real tmux server or terminal.

#![cfg(target_os = "linux")]

#[path = "support/limit_resume_harness.rs"]
mod harness;
#[path = "../../../tests/support/isolated_hook.rs"]
mod isolated_hook;

use amplihack_hooks::limit_resume::RESUME_MSG;
use harness::{LIMIT_MENU, PLAIN_SCREEN, Rig, StandInRotator, field, with_transcript};
use serde_json::{Value, json};
use std::process::Output;

use amplihack_types::AMPLIHACK_HOOK_TEXT as VAR;
/// The values compared with the control run, which leaves the variable unset.
const SETTINGS: [&str; 3] = ["none", "NONE", "bogus-value"];

fn limit_payload() -> Value {
    json!({
        "hook_event_name": "StopFailure",
        "session_id": "00000000-0000-4000-8000-000000000003",
        "error": "rate_limit",
        "error_details": "placeholder",
        "last_assistant_message": "placeholder"
    })
}

/// A rig showing `screen`, with the hook-text variable set to `setting`.
fn rig(screen: &str, setting: Option<&str>) -> Rig {
    let rig = Rig::new(screen, false);
    match setting {
        Some(value) => rig.with_env(VAR, value),
        None => rig,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Seen {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    /// `outcome=` and `detail=` of each `resume.log` line.
    log: Vec<(Option<String>, Option<String>)>,
    keys: Vec<String>,
}

fn seen(rig: &Rig, out: Output) -> Seen {
    let log = rig
        .resume_log()
        .lines()
        .map(|line| {
            (
                field(line, "outcome").map(str::to_string),
                field(line, "detail").map(str::to_string),
            )
        })
        .collect();
    Seen {
        code: out.status.code(),
        stdout: out.stdout,
        stderr: out.stderr,
        log,
        keys: rig.send_keys_calls(),
    }
}

/// Runs `case` with the variable unset, checks that run with `control`,
/// then requires every setting in [`SETTINGS`] to give the same result.
fn same_under_every_setting(case: impl Fn(Option<&str>) -> Seen, control: impl Fn(&Seen)) {
    let expected = case(None);
    control(&expected);
    for setting in SETTINGS {
        let got = case(Some(setting));
        assert!(
            !String::from_utf8_lossy(&got.stderr).contains(VAR),
            "limit-resume never reads {VAR}: {setting:?}"
        );
        assert_eq!(got, expected, "{setting:?}");
    }
}

/// No rotator runs, so the hook decides at once (`rotator-down`).
#[test]
fn limit_resume_ignores_the_setting_when_no_rotator_runs() {
    same_under_every_setting(
        |setting| {
            let rig = rig(PLAIN_SCREEN, setting);
            rig.private_resume_dir();
            let out = rig.run_plain(Some("1"), limit_payload().to_string().as_bytes());
            seen(&rig, out)
        },
        |control| {
            assert_eq!(control.code, Some(0), "fixture check: {control:?}");
            assert!(
                control
                    .log
                    .iter()
                    .any(|(outcome, _)| outcome.as_deref() == Some("rotator-down")),
                "fixture check: {:?}",
                control.log
            );
        },
    );
}

/// The exit-2 wake: the hook cannot prove a pane, so it wakes the session
/// with `RESUME_MSG` on stderr under every value.
#[test]
fn limit_resume_wakes_the_session_under_none() {
    same_under_every_setting(
        |setting| {
            let rig = rig(PLAIN_SCREEN, setting);
            let _rotator = StandInRotator::start(&rig.root);
            rig.private_resume_dir();
            let transcript = rig.swap_already_happened();
            let out = rig.run_plain(Some("1"), &with_transcript(&limit_payload(), &transcript));
            seen(&rig, out)
        },
        |control| {
            assert_eq!(control.code, Some(2), "fixture check: the control wakes");
            assert_eq!(control.stderr, format!("{RESUME_MSG}\n").into_bytes());
            assert!(control.stdout.is_empty());
            assert!(control.keys.is_empty(), "{control:?}");
        },
    );
}

/// The resume by keys: on a proved pane showing the limit menu, the hook
/// sends the same keys and logs the same outcome under every value.
#[test]
fn limit_resume_sends_keys_under_none() {
    same_under_every_setting(
        |setting| {
            let rig = rig(LIMIT_MENU, setting);
            let _rotator = StandInRotator::start(&rig.root);
            rig.private_resume_dir();
            let transcript = rig.swap_already_happened();
            let out = rig.run_on_pty("1", &with_transcript(&limit_payload(), &transcript));
            seen(&rig, out)
        },
        |control| {
            assert_eq!(control.code, Some(0), "fixture check: {control:?}");
            assert!(control.stdout.is_empty() && control.stderr.is_empty());
            assert_eq!(control.keys.len(), 4, "fixture check: {control:?}");
            assert!(
                control
                    .log
                    .iter()
                    .any(|(outcome, _)| outcome.as_deref() == Some("resumed-keys")),
                "fixture check: {:?}",
                control.log
            );
        },
    );
}
