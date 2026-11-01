//! Issue #1760, R6.3 and R9.2(3), SessionEnd path: a marker
//! `~/.amplihack/agents/ended/<id>` written after the agent's `launched_at`
//! ends the `active` agent with that conversation (`ended_by: session-end`),
//! writes no log line, and is deleted, so a later tmux loss is not resumed.
//! A stale marker is deleted and ignored; an unmatched one is kept for 7
//! days; a marker is kept while the recorded Claude process still runs (A6);
//! a marker that fails the S14 checks is ignored and removed only when old;
//! an `ended/` that is a symlink is not followed.

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fixture::*;
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, SystemTime};

const DAY: i64 = 86_400;

/// A lost `active` agent launched `ago` seconds back, with a transcript.
fn lost(fx: &Fx, name: &str, conv: &str, ago: i64) -> Value {
    let d = fx.dir(name);
    fx.transcript(&d, conv);
    let mut e = fx.entry(name, &d, Some(conv));
    e["registered_at"] = json!(ts(now() - ago));
    e["launched_at"] = json!(ts(now() - ago));
    e
}

fn pass(fx: &Fx) {
    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
}

/// Sets the file's mtime `secs` seconds back.
fn age(p: &Path, secs: u64) {
    let t = SystemTime::now() - Duration::from_secs(secs);
    std::fs::File::open(p).unwrap().set_modified(t).unwrap();
}

#[test]
fn a_marker_newer_than_the_launch_ends_the_agent_without_a_line() {
    let fx = Fx::new();
    let a = lost(&fx, "coord", CONV_A, 7200);
    let b = lost(&fx, "area", CONV_B, 7200);
    fx.write_agents(&[("coord", a), ("area", b)]);
    let ma = fx.marker(CONV_A, "prompt_input_exit", now() - 60);
    let mb = fx.marker(CONV_B, "logout", now() - 30);
    pass(&fx);
    for name in ["coord", "area"] {
        let e = fx.agent(name);
        assert_eq!(e["state"], "ended", "{name}: {e}");
        assert_eq!(e["ended_by"], "session-end", "{name}");
        assert!(parse_ts(e["ended_at"].as_str().unwrap()) >= now() - 60);
    }
    assert!(!ma.exists() && !mb.exists(), "applied markers are deleted");
    assert!(
        fx.new_sessions().is_empty(),
        "an ended agent is never resumed"
    );
    assert!(fx.log_lines().is_empty(), "{:?}", fx.log_lines());
    let status = stdout(&fx.run(&["agent", "status"]));
    assert!(status.contains("(session-end)"), "{status}");
}

#[test]
fn a_marker_not_newer_than_the_launch_is_stale() {
    let fx = Fx::new();
    let older = lost(&fx, "older", CONV_A, 60);
    let same = lost(&fx, "same", CONV_B, 120);
    let launched = parse_ts(same["launched_at"].as_str().unwrap());
    fx.write_agents(&[("older", older), ("same", same)]);
    let m1 = fx.marker(CONV_A, "prompt_input_exit", now() - 7200);
    let m2 = fx.marker(CONV_B, "prompt_input_exit", launched);
    pass(&fx);
    assert!(!m1.exists() && !m2.exists(), "stale markers are deleted");
    for name in ["older", "same"] {
        assert_eq!(fx.agent(name)["state"], "active", "{name}");
    }
    assert_eq!(fx.new_sessions().len(), 2, "both were lost and are resumed");
}

#[test]
fn a_marker_for_an_agent_that_is_not_active_is_deleted() {
    let fx = Fx::new();
    let mut e = lost(&fx, "done", CONV_C, 7200);
    e["state"] = json!("ended");
    e["ended_at"] = json!(ts(now() - 3600));
    e["ended_by"] = json!("wrapper");
    fx.write_agents(&[("done", e)]);
    let before = fx.agent("done");
    let m = fx.marker(CONV_C, "logout", now() - 60);
    pass(&fx);
    assert!(!m.exists());
    assert_eq!(fx.agent("done"), before);
}

#[test]
fn unmatched_markers_are_kept_for_seven_days() {
    let fx = Fx::new();
    fx.write_agents(&[]);
    let young = fx.marker(CONV_D, "prompt_input_exit", now() - DAY);
    let old = fx.marker(CONV_C, "prompt_input_exit", now() - 8 * DAY);
    age(&old, 8 * DAY as u64);
    pass(&fx);
    assert!(
        young.exists(),
        "a marker may arrive before its agent is registered"
    );
    assert!(
        !old.exists(),
        "an unmatched marker older than 7 days is deleted"
    );
}

/// A6: another `claude --resume ID` in a different terminal ended; the
/// agent's own process is still running.
#[test]
fn a_marker_is_kept_while_the_recorded_process_lives() {
    let fx = Fx::new();
    let mut e = lost(&fx, "coord", CONV_A, 7200);
    let d = fx.work.join("coord");
    fx.add_proc(&Proc::new(5500, 1, &["claude"], &d).start(31));
    e["claude_pid"] = json!(5500);
    e["proc_start"] = json!(31);
    fx.tmux_session("coord", "main");
    fx.write_agents(&[("coord", e)]);
    let m = fx.marker(CONV_A, "prompt_input_exit", now() - 60);
    pass(&fx);
    assert!(m.exists(), "kept while the recorded process runs");
    assert_eq!(fx.agent("coord")["state"], "active");
    assert!(
        fx.log_lines().is_empty(),
        "a healthy agent: {:?}",
        fx.log_lines()
    );
}

/// S14: only a private, well-formed marker counts. Invalid files are left
/// alone until they are 7 days old.
#[test]
fn invalid_markers_are_ignored_and_removed_only_when_old() {
    let fx = Fx::new();
    let names = ["open", "wrong-id", "clear", "junk"];
    let convs = [CONV_A, CONV_B, CONV_C, CONV_D];
    let mut agents = Vec::new();
    for (n, c) in names.iter().zip(convs) {
        fx.tmux_session(n, "main");
        agents.push((*n, lost(&fx, n, c, 7200)));
    }
    fx.write_agents(&agents);
    let open = fx.marker(CONV_A, "prompt_input_exit", now() - 60);
    chmod(&open, 0o644);
    let wrong = fx.marker(CONV_B, "logout", now() - 60);
    let body = json!({ "session_id": CONV_C, "reason": "logout", "at": now() - 60 });
    std::fs::write(&wrong, body.to_string()).unwrap();
    let clear = fx.marker(CONV_C, "clear", now() - 60);
    let junk = fx.marker(CONV_D, "logout", now() - 60);
    std::fs::write(&junk, "not json").unwrap();
    let old_junk = fx.ended_dir().join("0ccc2222-3d3d-4e4e-8f8f-9a9a9a9a9a01");
    std::fs::write(&old_junk, "not json").unwrap();
    chmod(&old_junk, 0o600);
    age(&old_junk, 8 * DAY as u64);
    pass(&fx);
    for n in names {
        assert_eq!(
            fx.agent(n)["state"],
            "active",
            "{n} was ended by an invalid marker"
        );
    }
    for m in [&open, &wrong, &clear, &junk] {
        assert!(m.exists(), "{} is younger than 7 days", m.display());
    }
    assert!(
        !old_junk.exists(),
        "an invalid file older than 7 days is deleted"
    );
}

/// Security review finding 1: nothing in the target of a symlinked `ended/`
/// is applied or deleted, and the pass warns once.
#[test]
fn a_symlinked_ended_directory_is_not_followed() {
    let fx = Fx::new();
    fx.tmux_session("coord", "main");
    fx.write_agents(&[("coord", lost(&fx, "coord", CONV_A, 7200))]);
    let target = fx.root.join("elsewhere");
    std::fs::create_dir(&target).unwrap();
    chmod(&target, 0o700);
    let old = target.join("notes.txt");
    std::fs::write(&old, "keep me").unwrap();
    chmod(&old, 0o600);
    age(&old, 8 * DAY as u64);
    let marker = target.join(CONV_A);
    let body = json!({ "session_id": CONV_A, "reason": "logout", "at": now() - 60 });
    std::fs::write(&marker, body.to_string()).unwrap();
    chmod(&marker, 0o600);
    std::os::unix::fs::symlink(&target, fx.ended_dir()).unwrap();
    let out = fx.supervise_once(&["--gap", "0"]);
    expect_code(&out, 0);
    assert!(old.exists() && marker.exists(), "the target is left alone");
    assert_eq!(fx.agent("coord")["state"], "active");
    let warning =
        "warning: ~/.amplihack/agents/ended: is not a real directory; no markers were read";
    assert_eq!(stderr(&out).matches(warning).count(), 1, "{}", stderr(&out));
}
