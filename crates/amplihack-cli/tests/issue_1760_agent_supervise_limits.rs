//! Issue #1760, R9.2(4) and (5): the crash-loop limit (D14) and the stagger.
//! An agent resumed `--max-restarts` times in the last hour is stopped
//! instead: its state becomes `stopped`, `alert` is set, the recovery line is
//! followed by the ALERT line, and `status` shows the alert in both forms.
//! Lost agents are resumed dirtiest first (ties by name) in batches of
//! `--batch`, `--gap` seconds apart. git runs without fsmonitor, optional
//! locks or an inherited `GIT_DIR` (S17). Flag ranges are clap's (R1.3).

#![cfg(target_os = "linux")]

use crate::issue_1760_fixture::*;
use serde_json::{Value, json};

fn pass(fx: &Fx, extra: &[&str]) -> String {
    let out = fx.supervise_once(extra);
    expect_code(&out, 0);
    stdout(&out)
}

/// A lost agent with a transcript and `restarts` at `ago` seconds back.
fn lost_with_restarts(fx: &Fx, name: &str, conv: &str, ago: &[i64]) -> Value {
    let d = fx.dir(name);
    fx.transcript(&d, conv);
    let mut e = fx.entry(name, &d, Some(conv));
    let mut ago = ago.to_vec();
    ago.sort_by(|a, b| b.cmp(a));
    e["restarts"] = json!(ago.iter().map(|s| ts(now() - s)).collect::<Vec<_>>());
    e
}

#[test]
fn the_restart_limit_stops_the_agent_and_raises_an_alert() {
    let fx = Fx::new();
    let e = lost_with_restarts(&fx, "area-ci", CONV_A, &[3000, 2000, 1000]);
    fx.write_agents(&[("area-ci", e)]);
    let out = pass(&fx, &["--gap", "0"]);
    assert!(
        fx.new_sessions().is_empty(),
        "a fourth loss in the hour is not resumed"
    );
    let alert = "stopped: restart limit (3 restarts in the last hour)";
    let e = fx.agent("area-ci");
    assert_eq!(e["state"], "stopped", "{e}");
    assert_eq!(e["alert"], alert);
    assert_eq!(e["restarts"].as_array().map(Vec::len), Some(3), "{e}");
    let lines = fx.log_lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let (ts0, rest) = lines[0].split_once(' ').unwrap();
    parse_ts(ts0);
    assert!(
        rest.starts_with(&format!("agent=area-ci conversation={CONV_A} lost=")),
        "{rest}"
    );
    assert!(
        rest.ends_with(" resumed=- result=stopped: restart limit"),
        "{rest}"
    );
    assert!(
        !rest.contains("lost=- "),
        "lost_at is set on this loss: {rest}"
    );
    let (ts1, alert_line) = lines[1].split_once(' ').unwrap();
    parse_ts(ts1);
    assert_eq!(alert_line, format!("ALERT: agent area-ci {alert}"));
    assert!(
        out.contains(&lines[1]),
        "the ALERT line is printed too:\n{out}"
    );

    let text = fx.run(&["agent", "status"]);
    expect_code(&text, 0);
    let text = stdout(&text);
    assert!(
        text.lines().any(|l| l == format!("ALERT area-ci: {alert}")),
        "{text}"
    );
    let row = text
        .lines()
        .find(|l| l.starts_with("area-ci "))
        .expect("a row");
    assert!(
        row.contains(" stopped ") && row.trim_end().ends_with("stopped: restart limit"),
        "{row}"
    );

    let json_out = fx.run(&["agent", "status", "--json"]);
    expect_code(&json_out, 0);
    let v: Value = serde_json::from_str(stdout(&json_out).trim()).unwrap();
    let a = &v["agents"][0];
    assert_eq!(
        (a["name"].clone(), a["state"].clone()),
        (json!("area-ci"), json!("stopped"))
    );
    assert_eq!(a["alert"], alert);
    assert_eq!(a["restarts_last_hour"], 3);
    assert_eq!(a["last_recovery"]["result"], "stopped: restart limit");

    pass(&fx, &["--gap", "0"]);
    assert_eq!(
        fx.log_lines().len(),
        2,
        "a stopped agent is not looked at again"
    );
    assert!(fx.new_sessions().is_empty());
}

#[test]
fn restarts_older_than_an_hour_do_not_count() {
    let fx = Fx::new();
    let e = lost_with_restarts(&fx, "coord", CONV_A, &[7200, 5400, 3700]);
    fx.write_agents(&[("coord", e)]);
    pass(&fx, &["--gap", "0"]);
    assert_eq!(fx.new_sessions().len(), 1);
    let e = fx.agent("coord");
    assert_eq!(e["state"], "active");
    assert_eq!(
        e["restarts"].as_array().map(Vec::len),
        Some(4),
        "kept for 24 h: {e}"
    );
    let v: Value = serde_json::from_str(&stdout(&fx.run(&["agent", "status", "--json"]))).unwrap();
    assert_eq!(v["agents"][0]["restarts_last_hour"], 1);
}

#[test]
fn max_restarts_sets_the_limit() {
    let fx = Fx::new();
    let e = lost_with_restarts(&fx, "coord", CONV_A, &[600]);
    fx.write_agents(&[("coord", e)]);
    pass(&fx, &["--gap", "0", "--max-restarts", "1"]);
    assert!(fx.new_sessions().is_empty());
    let e = fx.agent("coord");
    assert_eq!(e["state"], "stopped");
    assert_eq!(
        e["alert"],
        "stopped: restart limit (1 restarts in the last hour)"
    );
}

/// P6: a failed launch counts as a restart, so an agent whose launch keeps
/// failing is stopped by the limit instead of being retried every pass.
#[test]
fn failed_launches_reach_the_restart_limit() {
    let fx = Fx::new();
    let e = lost_with_restarts(&fx, "coord", CONV_A, &[]);
    fx.write_agents(&[("coord", e)]);
    fx.tmux_fail_new("server exited unexpectedly");
    for n in 1..=2 {
        pass(&fx, &["--gap", "0", "--max-restarts", "2"]);
        let e = fx.agent("coord");
        assert_eq!(e["state"], "active", "pass {n}: {e}");
        assert_eq!(e["restarts"].as_array().map(Vec::len), Some(n), "{e}");
    }
    let tried = fx.new_sessions().len();
    let out = pass(&fx, &["--gap", "0", "--max-restarts", "2"]);
    assert_eq!(fx.new_sessions().len(), tried, "no launch over the limit");
    let e = fx.agent("coord");
    assert_eq!(e["state"], "stopped", "{e}");
    let alert = "stopped: restart limit (2 restarts in the last hour)";
    assert_eq!(e["alert"], alert);
    assert!(
        out.contains(&format!("ALERT: agent coord {alert}")),
        "{out}"
    );
}

/// R9.2(5): `--batch 2 --gap 1`, four lost agents, dirtiest first and ties
/// by name; the third resume starts at least a second after the second.
#[test]
fn lost_agents_are_resumed_dirtiest_first_in_batches() {
    let fx = Fx::new();
    let convs = [CONV_A, CONV_B, CONV_C, CONV_D];
    let dirt = [("alpha", 0), ("bravo", 5), ("charlie", 2), ("delta", 2)];
    let mut agents = Vec::new();
    for ((name, n), conv) in dirt.into_iter().zip(convs) {
        let e = lost_with_restarts(&fx, name, conv, &[]);
        fx.dirty(&fx.work.join(name), n);
        agents.push((name, e));
    }
    fx.write_agents(&agents);
    pass(&fx, &["--batch", "2", "--gap", "1"]);
    let started = fx.new_sessions();
    let order: Vec<&str> = started.iter().map(|c| c.opt("-s").unwrap_or("")).collect();
    assert_eq!(order, ["bravo", "charlie", "delta", "alpha"]);
    // Wall-clock times from `date`; the gap is a monotonic sleep of 1 s.
    let between = started[2].time_ns.saturating_sub(started[1].time_ns);
    assert!(between >= 950_000_000, "batches only {between} ns apart");
}

/// S17: git sees its safe options and none of the supervisor's `GIT_*`
/// redirections.
#[test]
fn git_runs_without_fsmonitor_locks_or_an_inherited_git_dir() {
    let fx = Fx::new();
    let a = lost_with_restarts(&fx, "alpha", CONV_A, &[]);
    let b = lost_with_restarts(&fx, "bravo", CONV_B, &[]);
    fx.write_agents(&[("alpha", a), ("bravo", b)]);
    let planted = fx.root.join("planted.git");
    let mut c = fx.cmd(&["agent", "supervise", "--once", "--gap", "0"]);
    c.env("GIT_DIR", &planted)
        .env("GIT_WORK_TREE", &fx.root)
        .env("GIT_INDEX_FILE", planted.join("index"));
    expect_code(&fx.output(&mut c), 0);
    let calls = fx.git_calls();
    let status: Vec<_> = calls
        .iter()
        .filter(|(argv, _)| argv.iter().any(|a| a == "status"))
        .collect();
    assert_eq!(status.len(), 2, "one git status per candidate: {calls:?}");
    for (argv, env) in status {
        let dir = argv.windows(2).find(|w| w[0] == "-C").map(|w| w[1].clone());
        let dir = dir.unwrap_or_else(|| panic!("no -C DIR: {argv:?}"));
        let want = [
            "-c",
            "core.fsmonitor=false",
            "-C",
            dir.as_str(),
            "status",
            "--porcelain",
        ];
        assert_eq!(argv[..6], want, "{argv:?}");
        assert_eq!(argv[6..], ["--untracked-files=no"], "{argv:?}");
        assert_eq!(env.get("GIT_OPTIONAL_LOCKS").map(String::as_str), Some("0"));
        assert_eq!(
            env.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        for k in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
        ] {
            assert!(!env.contains_key(k), "{k} reached git: {env:?}");
        }
    }
}

#[test]
fn flag_ranges_are_usage_errors() {
    let fx = Fx::new();
    for bad in [
        ["--interval", "0"],
        ["--batch", "0"],
        ["--max-restarts", "0"],
    ] {
        let out = fx.supervise_once(&bad);
        expect_code(&out, 2);
        assert!(stderr(&out).contains(bad[0]), "{}", stderr(&out));
    }
    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
    expect_code(&fx.supervise_once(&["--interval", "1", "--batch", "1"]), 0);
}
