//! Issue #1760, G1: the supervisor starts a tmux server in a scope unit of
//! its own. Before the first resume on each socket in a pass it runs
//! `list-sessions`; when no server answers, `new-session` runs through
//! `systemd-run --user --scope --collect --quiet
//! --unit=amplihack-tmux-server-TS-N`. The four cases:
//!
//! 1. a scope is created, and each unit name is `-TS-N` with N counting up;
//! 2. without `systemd-run`, tmux starts plainly and a WARNING is logged;
//! 3. `systemd-run` fails after the session exists: no second launch;
//! 4. `systemd-run` times out: treated as case 3.
//!
//! Plus: a running server is used without `systemd-run`, and a
//! `systemd-run` that fails before tmux ran leads to one plain launch.

#![cfg(target_os = "linux")]

use crate::issue_1760_fixture::{CONV_A, CONV_B, Fx, expect_code, now, stdout};
use serde_json::{Value, json};

const UNIT_PREFIX: &str = "--unit=amplihack-tmux-server-";
const SCOPE_ARGS: [&str; 4] = ["--user", "--scope", "--collect", "--quiet"];

/// One lost agent `coord` with a transcript on the default socket.
fn lost(fx: &Fx, name: &str, conv: &str) -> (String, Value) {
    let d = fx.dir(name);
    fx.transcript(&d, conv);
    (name.to_string(), fx.entry(name, &d, Some(conv)))
}

fn write(fx: &Fx, agents: Vec<(String, Value)>) {
    let refs: Vec<(&str, Value)> = agents
        .iter()
        .map(|(n, e)| (n.as_str(), e.clone()))
        .collect();
    fx.write_agents(&refs);
}

fn pass(fx: &Fx) -> String {
    let out = fx.supervise_once(&["--gap", "0"]);
    expect_code(&out, 0);
    stdout(&out)
}

fn warnings(fx: &Fx) -> Vec<String> {
    let lines = fx.log_lines();
    lines
        .into_iter()
        .filter(|l| l.contains(" WARNING: "))
        .collect()
}

fn results(fx: &Fx) -> Vec<String> {
    let lines = fx.log_lines();
    let r = lines.iter().filter_map(|l| l.split_once(" result="));
    r.map(|(_, r)| r.to_string()).collect()
}

/// `(TS, N)` of a `--unit=` argument.
fn unit_parts(arg: &str) -> (i64, u32) {
    let rest = arg
        .strip_prefix(UNIT_PREFIX)
        .unwrap_or_else(|| panic!("{arg}"));
    let (ts, n) = rest.split_once('-').unwrap_or_else(|| panic!("{arg}"));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    assert!(digits(ts) && digits(n), "only TS and N in the name: {arg}");
    (ts.parse().unwrap(), n.parse().unwrap())
}

/// Case 1: two sockets, two scopes, numbered 1 and 2 within the pass.
#[test]
fn an_absent_server_is_started_in_a_numbered_scope() {
    let fx = Fx::new();
    // Two sockets with no server yet: the fake tmux keeps one session list.
    let socket = fx.socket_dir.join("ci");
    let (ci, mut ci_entry) = lost(&fx, "ci", CONV_B);
    ci_entry["tmux_socket"] = json!(socket);
    let (co, mut co_entry) = lost(&fx, "coord", CONV_A);
    co_entry["tmux_socket"] = json!(fx.socket_dir.join("coord"));
    write(&fx, vec![(co, co_entry), (ci, ci_entry)]);
    let before = now();
    let out = pass(&fx);
    let after = now();
    let calls = fx.systemd_run_calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    let mut numbers = Vec::new();
    for argv in &calls {
        assert_eq!(argv[..4], SCOPE_ARGS, "{argv:?}");
        let (ts, n) = unit_parts(&argv[4]);
        assert!(
            (before..=after).contains(&ts),
            "{ts} not in {before}..={after}"
        );
        numbers.push(n);
        assert_eq!(argv[5..7], ["--", "tmux"], "{argv:?}");
        let tmux = &argv[7..];
        let at = usize::from(tmux.first().map(String::as_str) == Some("-S")) * 2;
        assert_eq!(tmux[at], "new-session", "{argv:?}");
    }
    assert_eq!(numbers, [1, 2], "ci (sorted first) then coord");
    assert_eq!(
        calls[0][7..9],
        ["-S".to_string(), socket.display().to_string()]
    );
    assert_eq!(
        fx.new_sessions().len(),
        2,
        "each session made once, in its scope"
    );
    assert_eq!(results(&fx), ["resumed", "resumed"]);
    assert!(warnings(&fx).is_empty(), "{:?}", fx.log_lines());
    assert!(!out.contains("WARNING"), "{out}");
}

#[test]
fn a_running_server_is_used_without_systemd_run() {
    let fx = Fx::new();
    fx.tmux_session("other", "main");
    write(&fx, vec![lost(&fx, "coord", CONV_A)]);
    pass(&fx);
    assert!(fx.systemd_run_calls().is_empty());
    assert_eq!(fx.new_sessions().len(), 1);
    assert_eq!(results(&fx), ["resumed"]);
    assert!(warnings(&fx).is_empty());
}

/// Case 2.
#[test]
fn without_systemd_run_the_session_starts_with_a_warning() {
    let fx = Fx::new();
    write(&fx, vec![lost(&fx, "coord", CONV_A)]);
    let path = fx.path_without_systemd_run();
    let mut c = fx.cmd(&["agent", "supervise", "--once", "--gap", "0"]);
    c.env("PATH", path);
    let out = fx.output(&mut c);
    expect_code(&out, 0);
    assert_eq!(fx.new_sessions().len(), 1);
    assert_eq!(results(&fx), ["resumed"]);
    let w = warnings(&fx);
    assert_eq!(w.len(), 1, "{:?}", fx.log_lines());
    let want = " WARNING: agent coord tmux server started without its own scope: \
                systemd-run is not on PATH";
    assert!(w[0].ends_with(want), "{}", w[0]);
    assert!(stdout(&out).contains(&w[0]), "printed too");
}

/// Case 3: `systemd-run` ran tmux, then failed; the session exists.
#[test]
fn a_failed_systemd_run_with_the_session_present_is_not_launched_again() {
    let fx = Fx::new();
    fx.systemd_run_mode("fail-after");
    write(&fx, vec![lost(&fx, "coord", CONV_A)]);
    pass(&fx);
    assert_eq!(fx.systemd_run_calls().len(), 1);
    assert_eq!(fx.new_sessions().len(), 1, "no second new-session");
    let has = fx.tmux_records();
    let has = has.iter().filter(|c| c.command() == "has-session").count();
    assert!(has >= 2, "classify, then the G1-S3 re-check");
    assert_eq!(results(&fx), ["resumed"]);
    assert!(warnings(&fx).is_empty(), "{:?}", fx.log_lines());
    let restarts = fx.agent("coord")["restarts"].as_array().map(Vec::len);
    assert_eq!(restarts, Some(1));
}

/// `systemd-run` failed before tmux ran: one plain launch and a WARNING.
#[test]
fn a_refused_systemd_run_starts_tmux_once_more_with_a_warning() {
    let fx = Fx::new();
    fx.systemd_run_mode("refuse");
    write(&fx, vec![lost(&fx, "coord", CONV_A)]);
    pass(&fx);
    assert_eq!(fx.new_sessions().len(), 1);
    assert_eq!(results(&fx), ["resumed"]);
    let w = warnings(&fx);
    assert_eq!(w.len(), 1, "{:?}", fx.log_lines());
    let reason = "systemd-run: Failed to connect to bus: No medium found";
    assert!(w[0].ends_with(&format!("own scope: {reason}")), "{}", w[0]);
}

/// Case 4: a `systemd-run` that does not finish within 10 s is treated as a
/// failure, and the session it made is not launched again.
#[test]
fn a_systemd_run_timeout_is_treated_as_a_failure() {
    let fx = Fx::new();
    fx.systemd_run_mode("hang");
    write(&fx, vec![lost(&fx, "coord", CONV_A)]);
    let started = std::time::Instant::now();
    pass(&fx);
    assert!(started.elapsed().as_secs() >= 10, "waited for the timeout");
    assert_eq!(fx.new_sessions().len(), 1, "no second new-session");
    assert_eq!(results(&fx), ["resumed"]);
    assert!(warnings(&fx).is_empty(), "{:?}", fx.log_lines());
}
