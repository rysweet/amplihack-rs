//! Issue #1760, R5.4 and R7: the pre-checks on a lost agent and what each
//! attempt writes. Missing conversation id, launch directory or transcript
//! (looked for only in the config directory the resume will use, D12 and A4)
//! fail without starting tmux; a tmux refusal is `failed: tmux: LINE` and is
//! not counted as a restart. One line per attempt, a repeat of the same
//! result is not written again (D13), a healthy agent writes nothing, and
//! each agent's `last_recovery` and `status --json` equal its latest log line
//! (B8).

#![cfg(target_os = "linux")]

use crate::issue_1760_fixture::*;
use serde_json::{Value, json};
use std::fs;

fn lines_for(fx: &Fx, name: &str) -> Vec<String> {
    let tag = format!(" agent={name} ");
    fx.log_lines()
        .into_iter()
        .filter(|l| l.contains(&tag))
        .collect()
}

fn result_of(fx: &Fx, name: &str) -> Option<String> {
    let lines = lines_for(fx, name);
    let line = lines.last()?;
    line.split_once(" result=").map(|(_, r)| r.to_string())
}

/// A high restart limit: failed launches count as restarts (P6), and these
/// tests repeat passes without testing the limit.
fn pass(fx: &Fx) -> String {
    let out = fx.supervise_once(&["--gap", "0", "--max-restarts", "20"]);
    expect_code(&out, 0);
    stdout(&out)
}

#[test]
fn a_lost_agent_without_a_transcript_fails_and_starts_nothing() {
    let fx = Fx::new();
    let d = fx.dir("coord");
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    pass(&fx);
    assert!(fx.new_sessions().is_empty());
    assert_eq!(
        result_of(&fx, "coord").as_deref(),
        Some("failed: no transcript")
    );
    let e = fx.agent("coord");
    assert_eq!(e["state"], "active");
    assert!(e["lost_at"].is_string(), "the loss is recorded: {e}");
    assert_eq!(e["restarts"], json!([]));
}

/// D12: only the config directory the resume will use counts; A4: either
/// encoding of the launch directory counts there.
#[test]
fn the_transcript_must_be_in_the_config_directory_the_resume_uses() {
    let fx = Fx::new();
    let work = fx.profile_dir("work");
    let (a, b, c) = (
        fx.dir("prof-only"),
        fx.dir("home-only"),
        fx.dir("with_prof"),
    );
    fx.transcript(&a, CONV_A);
    fx.transcript_in(&work, &b, CONV_B);
    let enc = c
        .display()
        .to_string()
        .replace(|ch: char| !ch.is_ascii_alphanumeric(), "-");
    fs::create_dir_all(work.join("projects").join(&enc)).unwrap();
    fs::write(
        work.join("projects")
            .join(&enc)
            .join(format!("{CONV_C}.jsonl")),
        "{}\n",
    )
    .unwrap();
    let mut ea = fx.entry("prof-only", &a, Some(CONV_A));
    ea["amplihack_flags"]["profile"] = json!("work");
    let mut ec = fx.entry("with_prof", &c, Some(CONV_C));
    ec["amplihack_flags"]["profile"] = json!("work");
    let eb = fx.entry("home-only", &b, Some(CONV_B));
    fx.write_agents(&[("prof-only", ea), ("home-only", eb), ("with_prof", ec)]);
    pass(&fx);
    assert_eq!(
        result_of(&fx, "prof-only").as_deref(),
        Some("failed: no transcript")
    );
    assert_eq!(
        result_of(&fx, "home-only").as_deref(),
        Some("failed: no transcript")
    );
    assert_eq!(result_of(&fx, "with_prof").as_deref(), Some("resumed"));
    let started = fx.new_sessions();
    assert_eq!(started.len(), 1, "{started:?}");
    let argv = started[0].pane_argv();
    assert!(
        argv.windows(2).any(|w| w == ["--profile", "work"]),
        "{argv:?}"
    );
}

#[test]
fn no_conversation_id_and_a_missing_directory_fail_in_that_order() {
    let fx = Fx::new();
    let d = fx.dir("has-dir");
    let gone = fx.work.join("gone");
    fx.write_agents(&[
        ("no-id", fx.entry("no-id", &d, None)),
        ("no-id-no-dir", fx.entry("no-id-no-dir", &gone, None)),
        ("no-dir", fx.entry("no-dir", &gone, Some(CONV_A))),
    ]);
    pass(&fx);
    assert!(fx.new_sessions().is_empty());
    assert_eq!(
        result_of(&fx, "no-id").as_deref(),
        Some("failed: no conversation id")
    );
    assert_eq!(
        result_of(&fx, "no-id-no-dir").as_deref(),
        Some("failed: no conversation id")
    );
    assert_eq!(
        result_of(&fx, "no-dir").as_deref(),
        Some("failed: launch directory missing")
    );
    let line = lines_for(&fx, "no-id").pop().unwrap();
    assert!(line.contains(" conversation=- "), "{line}");
}

#[test]
fn a_tmux_refusal_is_logged_with_its_first_line_and_counts_as_a_restart() {
    let fx = Fx::new();
    let d = fx.dir("coord");
    fx.transcript(&d, CONV_A);
    fx.tmux_fail_new("create window failed: index 0 in use\nsecond line");
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    pass(&fx);
    assert!(!fx.new_sessions().is_empty(), "attempted");
    assert_eq!(
        result_of(&fx, "coord").as_deref(),
        Some("failed: tmux: create window failed: index 0 in use")
    );
    let e = fx.agent("coord");
    assert_eq!(e["state"], "active");
    assert_eq!(e["restarts"].as_array().map(Vec::len), Some(1), "P6: {e}");
    assert_eq!(
        e["last_recovery"]["result"],
        "failed: tmux: create window failed: index 0 in use"
    );
}

/// D13: one line per attempt; a repeat of the same result is not written,
/// a changed result is, and `resumed` always is.
#[test]
fn repeats_are_written_once_and_a_changed_result_again() {
    let fx = Fx::new();
    let d = fx.dir("coord");
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    pass(&fx);
    let quiet = pass(&fx);
    assert_eq!(lines_for(&fx, "coord").len(), 1, "{:?}", fx.log_lines());
    assert!(
        !quiet.contains("agent=coord"),
        "a suppressed repeat prints nothing: {quiet}"
    );
    fx.transcript(&d, CONV_A);
    fx.tmux_fail_new("server exited unexpectedly");
    pass(&fx);
    pass(&fx);
    assert_eq!(lines_for(&fx, "coord").len(), 2, "{:?}", fx.log_lines());
    fs::remove_file(fx.tmux_state.join("fail_new")).unwrap();
    pass(&fx);
    fs::write(fx.tmux_state.join("sessions"), "").unwrap();
    pass(&fx);
    let results: Vec<String> = lines_for(&fx, "coord")
        .iter()
        .map(|l| l.split_once(" result=").unwrap().1.to_string())
        .collect();
    assert_eq!(
        results,
        [
            "failed: no transcript",
            "failed: tmux: server exited unexpectedly",
            "resumed",
            "resumed"
        ]
    );
}

#[test]
fn a_healthy_agent_writes_no_line_and_clears_lost_at() {
    let fx = Fx::new();
    let d = fx.dir("coord");
    fx.transcript(&d, CONV_A);
    let p = crate::issue_1760_fake_proc::Proc::new(5600, 1, &["claude", "--resume", CONV_A], &d);
    fx.add_proc(&p);
    fx.tmux_session("coord", "main");
    let mut e = fx.entry("coord", &d, Some(CONV_A));
    e["lost_at"] = json!(ts(now() - 600));
    fx.write_agents(&[("coord", e)]);
    let out = pass(&fx);
    assert!(fx.log_lines().is_empty(), "{:?}", fx.log_lines());
    assert!(!out.contains("agent="), "{out}");
    let e = fx.agent("coord");
    assert!(e.get("lost_at").is_none_or(Value::is_null), "{e}");
    assert!(e.get("last_recovery").is_none_or(Value::is_null), "{e}");
}

/// Review issue 10: a healthy pass clears a stored skip, so the same skip
/// after the agent recovered is written again.
#[test]
fn a_skip_is_written_again_after_the_agent_was_healthy() {
    let fx = Fx::new();
    let d = fx.dir("coord");
    fx.transcript(&d, CONV_A);
    fx.tmux_session("coord", "main");
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    pass(&fx);
    let p = crate::issue_1760_fake_proc::Proc::new(5600, 1, &["claude", "--resume", CONV_A], &d);
    fx.add_proc(&p);
    pass(&fx);
    let e = fx.agent("coord");
    assert!(e.get("last_recovery").is_none_or(Value::is_null), "{e}");
    fx.remove_proc(5600);
    pass(&fx);
    let skipped = "skipped: tmux session exists";
    assert_eq!(result_of(&fx, "coord").as_deref(), Some(skipped));
    assert_eq!(lines_for(&fx, "coord").len(), 2, "{:?}", fx.log_lines());
}

/// A log line as the `last_recovery` object, `-` read as null.
fn parse_line(line: &str) -> Value {
    let (head, result) = line.split_once(" result=").unwrap();
    let f: Vec<&str> = head.split(' ').collect();
    assert_eq!(f.len(), 5, "{line}");
    let field = |i: usize, key: &str| {
        let v = f[i].strip_prefix(&format!("{key}=")).unwrap();
        if v == "-" { Value::Null } else { json!(v) }
    };
    json!({
        "logged_at": f[0], "conversation_id": field(2, "conversation"),
        "lost": field(3, "lost"), "resumed": field(4, "resumed"), "result": result
    })
}

/// The registry omits absent fields; status JSON writes them as null.
fn with_nulls(v: &Value) -> Value {
    let mut out = json!({ "conversation_id": null, "lost": null, "resumed": null });
    for (k, x) in v.as_object().expect("last_recovery is set") {
        out[k] = x.clone();
    }
    out
}

/// B8: after two passes with different outcomes, each agent's
/// `last_recovery` and its `status --json` object equal its latest line.
#[test]
fn last_recovery_and_status_json_match_the_latest_log_line() {
    let fx = Fx::new();
    let (a, b, c) = (fx.dir("alpha"), fx.dir("bravo"), fx.dir("charlie"));
    fx.transcript(&a, CONV_A);
    fx.write_agents(&[
        ("alpha", fx.entry("alpha", &a, Some(CONV_A))),
        ("bravo", fx.entry("bravo", &b, Some(CONV_B))),
        ("charlie", fx.entry("charlie", &c, None)),
    ]);
    pass(&fx);
    pass(&fx);
    assert_eq!(
        result_of(&fx, "alpha").as_deref(),
        Some("skipped: tmux session exists")
    );
    let out = fx.run(&["agent", "status", "--json"]);
    expect_code(&out, 0);
    let status: Value = serde_json::from_str(stdout(&out).trim()).expect("status --json");
    let agents = status["agents"].as_array().expect("agents array");
    assert_eq!(agents.len(), 3);
    for name in ["alpha", "bravo", "charlie"] {
        let want = parse_line(lines_for(&fx, name).last().expect("a line"));
        let stored = with_nulls(&fx.agent(name)["last_recovery"]);
        assert_eq!(stored, want, "{name}: registry");
        let row = agents.iter().find(|r| r["name"] == name).expect("row");
        assert_eq!(row["last_recovery"], want, "{name}: status --json");
        assert_eq!(row["conversation_id"], want["conversation_id"], "{name}");
    }
}
