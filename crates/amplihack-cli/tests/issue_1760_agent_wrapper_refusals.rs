//! Issue #1760, R1.5, D9, D18, B2, C3 and S12: what `amplihack claude
//! --supervise NAME` refuses. Flag conflicts are clap usage errors (exit 2);
//! every other refusal exits 1 before Claude starts and before any registry
//! write: no tmux, a print run, an invalid NAME, a tmux session or a
//! conversation another active agent already has, and a `$TMUX` socket whose
//! directory other users can enter (checked before tmux is ever run).

#![cfg(target_os = "linux")]

use crate::issue_1760_fixture::*;
use serde_json::json;
use std::path::Path;
use std::process::{Command, Output};

const NAME: &str = "area-docs";

fn supervised(fx: &Fx, flags: &[&str], claude_args: &[&str]) -> Command {
    fx.tmux_pane("%7", "area", "main", 4321);
    let mut args = vec!["claude"];
    args.extend(flags);
    args.extend(["--supervise", NAME, "--"]);
    args.extend(claude_args);
    let mut c = fx.cmd(&args);
    c.env("TMUX", fx.tmux_env()).env("TMUX_PANE", "%7");
    c
}

/// Exit 1 with `message` on stderr, Claude not started, nothing recorded
/// under NAME.
fn assert_refused(fx: &Fx, out: &Output, message: &str) {
    expect_code(out, 1);
    let err = stderr(out);
    assert!(
        err.contains(&format!("error: {message}")),
        "want {message:?}:\n{err}"
    );
    assert!(!fx.claude_started(), "Claude started:\n{err}");
    if fx.registry_path().exists() {
        assert!(
            fx.registry()["agents"].get(NAME).is_none(),
            "{}",
            fx.registry()
        );
    }
}

#[test]
fn modes_that_are_not_long_lived_sessions_are_usage_errors() {
    let fx = Fx::new();
    let cases: [&[&str]; 4] = [
        &["--auto"],
        &["--append", "more"],
        &["--docker"],
        &["--checkout-repo", "rysweet/amplihack-rs"],
    ];
    for flags in cases {
        let out = fx.output(&mut supervised(&fx, flags, &[]));
        expect_code(&out, 2);
        let err = stderr(&out);
        assert!(
            err.contains("--supervise") && err.contains(flags[0]),
            "{flags:?}:\n{err}"
        );
        assert!(!fx.claude_started());
    }
    assert!(!fx.registry_path().exists());
}

#[test]
fn without_tmux_nothing_starts() {
    let fx = Fx::new();
    let msg = "amplihack claude --supervise needs tmux: $TMUX and $TMUX_PANE are not set";
    let mut no_tmux = supervised(&fx, &[], &[]);
    no_tmux.env_remove("TMUX");
    assert_refused(&fx, &fx.output(&mut no_tmux), msg);
    let mut no_pane = supervised(&fx, &[], &[]);
    no_pane.env_remove("TMUX_PANE");
    assert_refused(&fx, &fx.output(&mut no_pane), msg);
    let mut empty = supervised(&fx, &[], &[]);
    empty.env("TMUX", "");
    assert_refused(&fx, &fx.output(&mut empty), msg);
    assert!(
        fx.tmux_records().is_empty(),
        "tmux was asked: {:?}",
        fx.tmux_records()
    );
}

/// B2: a print run is not a long-lived session.
#[test]
fn print_mode_is_refused() {
    let fx = Fx::new();
    let msg = "amplihack claude --supervise needs an interactive session; \
        -p and --print are not supported";
    for args in [
        &["-p", "do it"][..],
        &["--print", "do it"],
        &["--model", "opus", "-p"],
    ] {
        assert_refused(&fx, &fx.output(&mut supervised(&fx, &[], args)), msg);
    }
}

#[test]
fn an_invalid_name_is_refused_without_repeating_it() {
    let fx = Fx::new();
    let mut c = fx.cmd(&["claude", "--supervise", "bad/name#1", "--"]);
    c.env("TMUX", fx.tmux_env()).env("TMUX_PANE", "%7");
    let out = fx.output(&mut c);
    expect_code(&out, 1);
    let err = stderr(&out);
    assert!(err.contains("error: NAME: "), "{err}");
    assert!(!err.contains("bad/name#1"), "the value was repeated: {err}");
    assert!(!fx.claude_started());
}

/// D9 and C3: another active agent already owns the tmux session or the
/// conversation; an ended one does not.
#[test]
fn a_session_or_conversation_owned_by_another_active_agent_is_refused() {
    let fx = Fx::new();
    let d = fx.dir("other");
    let mut same_session = fx.entry("area", &d, Some(CONV_D));
    same_session["tmux_socket"] = json!(fx.socket);
    fx.write_agents(&[("other", same_session.clone())]);
    let before = fx.registry_raw();
    let out = fx.output(&mut supervised(&fx, &[], &[]));
    assert_refused(
        &fx,
        &out,
        "tmux session area is already used by registered agent other",
    );
    assert_eq!(fx.registry_raw(), before, "the registry was written");

    let mut elsewhere = fx.entry("elsewhere", &d, Some(CONV_A));
    elsewhere["tmux_socket"] = json!(fx.socket);
    fx.write_agents(&[("other", elsewhere)]);
    let out = fx.output(&mut supervised(&fx, &[], &["--resume", CONV_A]));
    assert_refused(
        &fx,
        &out,
        "this conversation is already used by registered agent other",
    );

    same_session["state"] = json!("ended");
    same_session["ended_at"] = json!(ts(now() - 60));
    same_session["ended_by"] = json!("wrapper");
    fx.write_agents(&[("other", same_session)]);
    fx.claude_mode("exit:0");
    expect_code(&fx.output(&mut supervised(&fx, &[], &[])), 0);
    assert_eq!(fx.agent(NAME)["tmux_session"], "area");
}

/// S12: a tmux client sends its environment to whatever listens on the
/// socket, so an open socket directory is refused before tmux runs.
#[test]
fn a_tmux_socket_in_a_directory_open_to_others_is_refused() {
    let fx = Fx::new();
    let loose = fx.tmux_tmpdir.join("loose");
    std::fs::create_dir(&loose).unwrap();
    chmod(&loose, 0o777);
    let mut c = supervised(&fx, &[], &[]);
    c.env(
        "TMUX",
        format!("{},4242,0", loose.join("default").display()),
    );
    let msg = "tmux socket: its directory must be owned by you and closed to other users";
    assert_refused(&fx, &fx.output(&mut c), msg);
    assert!(fx.tmux_records().is_empty(), "{:?}", fx.tmux_records());
    assert!(Path::new(&loose).read_dir().unwrap().next().is_none());
}
