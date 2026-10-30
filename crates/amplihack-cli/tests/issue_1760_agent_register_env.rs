//! Issue #1760, R2.4, B10, S8b, S11, S21: the recorded environment.
//!
//! - Only the eleven allowlisted names are copied, from the `amplihack claude`
//!   ancestor's environment, or from the `claude` process's when there is no
//!   such ancestor.
//! - Credentials, API redirections and config-directory selectors planted in
//!   both environments never reach `registry.json`.
//! - `--env KEY=VALUE` adds or replaces an allowlisted name; any other name is
//!   refused with the allowlist, and the argument is not repeated.
//! - `AMPLIHACK_AGENT_BINARY` must be an agent binary name.

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fixture::{CONV_A, Fx, expect_code, stderr, stdout};
use serde_json::json;
use std::path::PathBuf;

const CLAUDE_PID: u32 = 5000;
const AMP_PID: u32 = 4000;

const PLANTED: &[(&str, &str)] = &[
    ("ANTHROPIC_API_KEY", "sk-ant-PLANTED-key"),
    ("GH_TOKEN", "ghp_PLANTEDtoken"),
    ("GITHUB_TOKEN", "ghs_PLANTEDtoken"),
    ("ANTHROPIC_BASE_URL", "https://PLANTED.example"),
    ("ANTHROPIC_AUTH_TOKEN", "PLANTED-auth"),
    ("ANTHROPIC_CUSTOM_HEADERS", "x-PLANTED: 1"),
    ("CLAUDE_CONFIG_DIR", "/PLANTED/config"),
    ("AMPLIHACK_CLAUDE_PROFILE", "PLANTEDprofile"),
    ("AMPLIHACK_CLAUDE_PROFILE_ROOT", "/PLANTED/root"),
    ("AMPLIHACK_SESSION_ID", "PLANTED-session"),
    ("PATH", "/PLANTED/bin:/usr/bin"),
];

fn with_planted(mut p: Proc) -> Proc {
    for (k, v) in PLANTED {
        p = p.env(k, v);
    }
    p
}

/// `self -> claude -> [amplihack claude ->] 1`, conversation `CONV_A`.
fn tree(fx: &Fx, amp_env: Option<&[(&str, &str)]>, claude_env: &[(&str, &str)]) -> PathBuf {
    let d = fx.dir("d");
    let parent = match amp_env {
        Some(vars) => {
            let mut amp = with_planted(Proc::new(AMP_PID, 1, &["amplihack", "claude"], &d));
            for (k, v) in vars {
                amp = amp.env(k, v);
            }
            fx.add_proc(&amp);
            AMP_PID
        }
        None => 1,
    };
    let mut claude = with_planted(Proc::new(CLAUDE_PID, parent, &["claude"], &d).start(31));
    for (k, v) in claude_env {
        claude = claude.env(k, v);
    }
    fx.add_proc(&claude);
    fx.sessions_file(&fx.claude_dir, CLAUDE_PID, CONV_A, 31, &d);
    fx.set_self_parent(CLAUDE_PID);
    d
}

fn register(fx: &Fx, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "agent",
        "register",
        "coord",
        "--session",
        "coord",
        "--window",
        "main",
    ];
    args.extend(extra);
    fx.run(&args)
}

fn assert_nothing_planted(fx: &Fx) {
    let raw = String::from_utf8(fx.registry_raw()).unwrap();
    for (k, v) in PLANTED {
        assert!(!raw.contains(v), "{k}'s value reached the registry:\n{raw}");
    }
    for k in [
        "ANTHROPIC_API_KEY",
        "GH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CONFIG_DIR",
    ] {
        assert!(!raw.contains(k), "{k} reached the registry:\n{raw}");
    }
}

#[test]
fn allowlisted_variables_come_from_the_amplihack_ancestor() {
    let fx = Fx::new();
    let amp = [
        ("ANTHROPIC_MODEL", "claude-opus-5-5"),
        ("CLAUDE_CODE_EFFORT_LEVEL", "high"),
        ("AMPLIHACK_AGENT_BINARY", "claude"),
        ("MAX_THINKING_TOKENS", "32000"),
    ];
    tree(
        &fx,
        Some(&amp[..]),
        &[("ANTHROPIC_MODEL", "from-the-claude-process")],
    );
    let o = register(&fx, &[]);
    expect_code(&o, 0);
    assert_eq!(
        fx.agent("coord")["env"],
        json!({
            "AMPLIHACK_AGENT_BINARY": "claude",
            "ANTHROPIC_MODEL": "claude-opus-5-5",
            "CLAUDE_CODE_EFFORT_LEVEL": "high",
            "MAX_THINKING_TOKENS": "32000"
        })
    );
    assert_nothing_planted(&fx);
    let out = format!("{}{}", stdout(&o), stderr(&o));
    assert!(
        !out.contains("PLANTED"),
        "an environment value was printed: {out}"
    );
}

#[test]
fn without_an_amplihack_ancestor_the_claude_process_is_read() {
    let fx = Fx::new();
    tree(&fx, None, &[("CLAUDE_CODE_SUBAGENT_MODEL", "haiku")]);
    expect_code(&register(&fx, &[]), 0);
    assert_eq!(
        fx.agent("coord")["env"],
        json!({ "CLAUDE_CODE_SUBAGENT_MODEL": "haiku" })
    );
    assert_nothing_planted(&fx);
}

#[test]
fn env_flags_add_and_replace_allowlisted_values() {
    let fx = Fx::new();
    tree(&fx, Some(&[("CLAUDE_CODE_EFFORT_LEVEL", "high")][..]), &[]);
    let o = register(
        &fx,
        &[
            "--env",
            "CLAUDE_CODE_EFFORT_LEVEL=max",
            "--env",
            "AMPLIHACK_DEFAULT_MODEL=opus",
        ],
    );
    expect_code(&o, 0);
    assert_eq!(
        fx.agent("coord")["env"],
        json!({ "AMPLIHACK_DEFAULT_MODEL": "opus", "CLAUDE_CODE_EFFORT_LEVEL": "max" })
    );
}

#[test]
fn a_name_off_the_allowlist_is_refused_without_repeating_it() {
    let fx = Fx::new();
    let d = fx.dir("d");
    for arg in [
        "GH_TOKEN=ghp_typedByMistake",
        "ANTHROPIC_API_KEY=sk-ant-typed",
        "CLAUDE_CONFIG_DIR=/x/y",
        "anthropic_model=lowercase",
    ] {
        let o = fx.register("a", "s", &d, CONV_A, &["--env", arg]);
        expect_code(&o, 1);
        let err = stderr(&o);
        assert!(
            err.contains("--env: only these variables can be recorded: "),
            "{arg}: {err}"
        );
        for listed in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
            "CLAUDE_CODE_EFFORT_LEVEL",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "MAX_THINKING_TOKENS",
            "AMPLIHACK_DEFAULT_MODEL",
            "AMPLIHACK_AGENT_BINARY",
        ] {
            assert!(
                err.contains(listed),
                "{listed} missing from the list: {err}"
            );
        }
        let (name, value) = arg.split_once('=').unwrap();
        assert!(!err.contains(value), "the value was repeated: {err}");
        assert!(!err.contains(name), "the name was repeated: {err}");
    }
    assert!(!fx.registry_path().exists());
}

#[test]
fn malformed_env_flags_are_refused() {
    let fx = Fx::new();
    let d = fx.dir("d");
    let long = format!("ANTHROPIC_MODEL={}", "m".repeat(4097));
    for (arg, message) in [
        ("ANTHROPIC_MODEL", "--env: expected KEY=VALUE"),
        (
            long.as_str(),
            "--env: value must be at most 4096 bytes, with no control characters",
        ),
        (
            "ANTHROPIC_MODEL=a\nb",
            "--env: value must be at most 4096 bytes, with no control characters",
        ),
    ] {
        let o = fx.register("a", "s", &d, CONV_A, &["--env", arg]);
        expect_code(&o, 1);
        assert!(stderr(&o).contains(message), "{}", stderr(&o));
    }
    assert!(!fx.registry_path().exists());
}

#[test]
fn the_agent_binary_must_be_an_agent_binary_name() {
    let fx = Fx::new();
    let d = fx.dir("d");
    let o = fx.register(
        "a",
        "s",
        &d,
        CONV_A,
        &["--env", "AMPLIHACK_AGENT_BINARY=/bin/sh"],
    );
    expect_code(&o, 1);
    assert!(stderr(&o).contains("--env"), "{}", stderr(&o));
    assert!(!stderr(&o).contains("/bin/sh"), "{}", stderr(&o));
    assert!(!fx.registry_path().exists());
    let o = fx.register(
        "a",
        "s",
        &d,
        CONV_A,
        &["--env", "AMPLIHACK_AGENT_BINARY=copilot"],
    );
    expect_code(&o, 0);
    assert_eq!(
        fx.agent("a")["env"],
        json!({ "AMPLIHACK_AGENT_BINARY": "copilot" })
    );
}

#[test]
fn a_discovered_value_that_fails_the_rules_is_dropped_with_a_warning() {
    let fx = Fx::new();
    tree(
        &fx,
        Some(
            &[
                ("AMPLIHACK_AGENT_BINARY", "/bin/sh"),
                ("ANTHROPIC_MODEL", "opus"),
            ][..],
        ),
        &[],
    );
    let o = register(&fx, &[]);
    expect_code(&o, 0);
    let err = stderr(&o);
    assert!(
        err.contains(
            "warning: ignored AMPLIHACK_AGENT_BINARY from the environment of process 4000: "
        ),
        "{err}"
    );
    assert!(!err.contains("/bin/sh"), "the value was printed: {err}");
    assert_eq!(
        fx.agent("coord")["env"],
        json!({ "ANTHROPIC_MODEL": "opus" })
    );
}
