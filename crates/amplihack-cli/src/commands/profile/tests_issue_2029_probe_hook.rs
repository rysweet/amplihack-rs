//! #2029: exercise the real common probe preparation, including fallbacks.
//! Adapted from the saved lane's builder tests. The marker is now specific
//! to usage probes; no helper that merely sets an environment variable is
//! substituted for the production preparation path.

#![cfg(unix)]

use super::name::ProfileName;
use super::probe_limits::ProbeMode;
use super::probe_sandbox::{Notice, ProbeSandbox, Unavailable};
use super::run::{build_default_dir_command, build_home_command, build_profile_command};
use super::store::ProfileStore;
use super::usage_cmd::probe_with_sandbox;
use super::usage_parse::UsageState;
use crate::binary_finder::BinaryInfo;
use std::ffi::OsStr;
use std::fs;
use std::process::Command;
use std::time::Duration;

const MARKER: &str = "AMPLIHACK_USAGE_PROBE";
const NOW: i64 = 1_790_000_000;
const ANSWER: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"amplihack-usage","response":{"rate_limits_available":true,"extra_usage":{"is_enabled":false},"rate_limits":{"five_hour":{"utilization":7},"seven_day":{"utilization":9},"extra_usage":{"is_enabled":false}}}}}"#;

// These finite scripts exit on every path. No sleeps, retries or real Claude.
fn attempt(mode: ProbeMode, response: &str, wrapped: bool) -> (String, String) {
    let _env = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = crate::test_support::tempdir_outside_framework_roots();
    let home = dir.path().join("home");
    fs::create_dir_all(home.join(".claude/projects")).unwrap();
    let marks = dir.path().join("attempts");
    let wraps = dir.path().join("wraps");
    let claude = dir.path().join("claude");
    crate::test_support::write_executable(
        &claude,
        r#"#!/bin/sh
printf '%s|%s\n' "${AMPLIHACK_USAGE_PROBE-unset}" "$*" >> "$MARKS"
case "$*" in
  *--input-format*)
    IFS= read -r request
    case "$RESPONSE" in
      accepted) printf '%s\n' "$ANSWER" ;;
      malformed) printf 'invalid json\n' ;;
      refused) printf '{"type":"control_response","response":{"subtype":"error","request_id":"amplihack-usage"}}\n' ;;
      failed) exit 2 ;;
    esac ;;
  *) printf 'Current session: 7%% used\nCurrent week (all models): 9%% used\n' ;;
esac
"#,
    );
    let bwrap = dir.path().join("bwrap");
    crate::test_support::write_executable(
        &bwrap,
        r#"#!/bin/sh
printf '%s\n' "${AMPLIHACK_USAGE_PROBE-unset}" >> "$WRAPS"
while [ "$#" -gt 0 ] && [ "$1" != -- ]; do shift; done
[ "$#" -gt 0 ] || exit 64
shift
exec "$@"
"#,
    );
    let sandbox = if wrapped {
        ProbeSandbox::Bwrap(bwrap)
    } else {
        ProbeSandbox::Unavailable(Unavailable::Test)
    };
    let probe = probe_with_sandbox(
        |args| {
            let mut cmd = Command::new(&claude);
            cmd.env_clear()
                .args(args)
                .env("HOME", &home)
                .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
                .env("AMPLIHACK_HOME", home.join(".amplihack"))
                .env("CODEX_HOME", home.join(".codex"))
                .env("XDG_CONFIG_HOME", home.join(".config"))
                .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", home.join("profiles"))
                .env("MARKS", &marks)
                .env("WRAPS", &wraps)
                .env("RESPONSE", response)
                .env("ANSWER", ANSWER)
                // Normal launchers remove an inherited marker. Preparation
                // must override that removal for every probe attempt.
                .env_remove(MARKER);
            Ok(cmd)
        },
        &sandbox,
        &home,
        &Notice::new(),
        mode,
        Duration::from_secs(60), // Existing subprocess protection only.
        NOW,
        false,
    )
    .unwrap();
    assert_eq!(probe.report.state, UsageState::Ok);
    assert_eq!(probe.report.session.unwrap().used_percentage, 7.0);
    (
        fs::read_to_string(marks).unwrap(),
        fs::read_to_string(wraps).unwrap_or_default(),
    )
}

#[test]
fn text_probe_suppresses_hooks_before_sandbox_wrapping() {
    for wrapped in [false, true] {
        let (attempts, wrappers) = attempt(ProbeMode::Text, "accepted", wrapped);
        assert_eq!(attempts, "1|-p /usage --no-session-persistence\n");
        assert_eq!(wrappers, if wrapped { "1\n" } else { "" });
    }
}

#[test]
fn structured_probe_suppresses_hooks_without_starting_a_text_probe() {
    let (attempts, wrappers) = attempt(ProbeMode::Auto, "accepted", true);
    assert_eq!(
        attempts.lines().count(),
        1,
        "accepted structured answer must not fall back"
    );
    assert!(
        attempts.starts_with("1|-p --input-format stream-json"),
        "{attempts}"
    );
    assert!(attempts.contains("--no-session-persistence"));
    assert_eq!(wrappers, "1\n");
}

#[test]
fn every_structured_failure_keeps_suppression_on_the_text_fallback() {
    for response in ["malformed", "refused", "failed"] {
        for wrapped in [false, true] {
            let (attempts, wrappers) = attempt(ProbeMode::Auto, response, wrapped);
            let lines: Vec<_> = attempts.lines().collect();
            assert_eq!(lines.len(), 2, "{response}: {attempts}");
            assert!(
                lines
                    .iter()
                    .all(|s| s.starts_with("1|") && s.contains("--no-session-persistence")),
                "{response}: {attempts}"
            );
            assert_eq!(lines[1], "1|-p /usage --no-session-persistence");
            assert_eq!(wrappers, if wrapped { "1\n1\n" } else { "" });
        }
    }
}

#[test]
fn ordinary_profile_live_and_home_builders_explicitly_clear_inherited_suppression() {
    let _env = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let store = ProfileStore::new(tmp.path().join("profiles"));
    let name = ProfileName::parse("work").unwrap();
    store.add(&name).unwrap();
    let bin = BinaryInfo {
        name: "claude".into(),
        path: tmp.path().join("unused-claude"),
        version: None,
    };
    let commands = [
        build_profile_command(&bin, &store, &name, &[]).unwrap(),
        build_default_dir_command(&bin, &store, &name, &[]).unwrap(),
        build_home_command(&bin, &[]),
    ];
    for cmd in commands {
        assert_eq!(
            cmd.get_envs()
                .find(|(k, _)| *k == OsStr::new(MARKER))
                .map(|(_, v)| v),
            Some(None),
            "ordinary launcher must actively remove {MARKER}"
        );
    }
}
