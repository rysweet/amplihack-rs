//! Issue #2018 (part 2): each headless step records its launcher, agent and
//! parent where a supervisor can find them, a record failure never stops
//! the step (D6), and the record goes when the step ends.
//!
//! Black-box through the real binary, with the fixture in
//! `support::step_2018` (temporary `HOME`, `isolated(..)`). Linux only
//! (`/proc` start times).

#![cfg(target_os = "linux")]

use crate::support::proc_watch::{assert_gone, start_ticks};
use crate::support::step_2018::{Fx, sigkill, states, text};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

const RUN_ID: &str = "issue-2018-records-run";

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap())
        .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

/// Stderr lines that mention the step record.
fn record_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.contains("step record"))
        .collect()
}

/// Runs a headless step whose agent exits 0 at once, with `run_id`, and
/// returns its stderr after checking that it succeeded and the agent ran.
fn quick_step(fx: &Fx, run_id: &str) -> String {
    let mut c = fx.step(Some(run_id));
    c.env("AGENT_MODE", "exit");
    let out = c.output().expect("run amplihack claude");
    let stderr = text(&out.stderr);
    assert!(
        out.status.success(),
        "a record failure must not stop the step: {}; stderr:\n{stderr}",
        out.status
    );
    fx.pid("agent.pid");
    stderr
}

/// While the step runs there is exactly one record at the documented path,
/// with every format-1 field and private modes, and `recipe steps` reports
/// it live. When the agent exits 0 the record and the run-id directory go.
#[test]
fn a_running_step_has_exactly_one_record_until_it_ends() {
    let fx = Fx::new();
    let run_dir_env = fx.work.join("runs").join("ws-2018");
    let mut cmd = fx.step(Some(RUN_ID));
    cmd.env("AGENT_MODE", "gate")
        .env("AMPLIHACK_RUN_ID", "ws-2018")
        .env("AMPLIHACK_RUN_DIR", &run_dir_env);
    let mut launcher = fx.spawn_logged(cmd);
    let launcher_pid = launcher.id() as i32;
    let agent_pid = fx.pid("agent.pid");
    let me = std::process::id() as i32;

    let path = fx.the_record(RUN_ID);
    assert_eq!(
        path,
        fx.run_dir(RUN_ID).join(format!("{launcher_pid}.json"))
    );
    let rec = read_json(&path);
    assert_eq!(rec["format"], json!(1));
    assert_eq!(rec["run_id"], json!(RUN_ID));
    assert_eq!(rec["workspace_run_id"], json!("ws-2018"));
    assert_eq!(rec["run_dir"], json!(run_dir_env.to_str().unwrap()));
    assert_eq!(rec["tool"], json!("claude"));
    assert_eq!(rec["cwd"], json!(fx.work.to_str().unwrap()));
    let created = rec["created_at"].as_str().expect("created_at");
    chrono::DateTime::parse_from_rfc3339(created)
        .unwrap_or_else(|e| panic!("created_at {created:?} is not RFC 3339: {e}"));
    assert!(
        created.ends_with('Z') && created.len() == 20,
        "UTC to the second: {created:?}"
    );

    for (who, pid) in [
        ("launcher", launcher_pid),
        ("agent", agent_pid),
        ("parent", me),
    ] {
        assert_eq!(rec[who]["pid"], json!(pid), "{who}.pid");
        let want = start_ticks(pid).expect("a live process has a start time");
        assert_eq!(rec[who]["start_ticks"], json!(want), "{who}.start_ticks");
    }
    let mut keys: Vec<&str> = rec
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "agent",
            "created_at",
            "cwd",
            "format",
            "launcher",
            "parent",
            "run_dir",
            "run_id",
            "tool",
            "workspace_run_id"
        ],
        "a record carries the format-1 fields and nothing else"
    );

    assert_eq!(mode(&path), 0o600, "record mode");
    assert_eq!(mode(&fx.run_dir(RUN_ID)), 0o700, "run-id directory mode");
    assert_eq!(mode(&fx.root()), 0o700, "recipe-steps directory mode");

    let listed = fx.steps_json(&[]);
    assert_eq!(states(&listed), vec!["live"], "{listed:?}");
    assert_eq!(listed[0]["path"], json!(path.to_str().unwrap()));
    assert_eq!(listed[0]["record"], rec);

    fx.release();
    let status = launcher.wait().unwrap();
    assert!(status.success(), "status {status}; log:\n{}", fx.log());
    assert!(
        !path.exists(),
        "the record must be removed on a normal exit"
    );
    assert!(
        !fx.run_dir(RUN_ID).exists(),
        "and the empty run-id directory"
    );
    assert!(
        record_lines(&fx.log()).is_empty(),
        "no diagnostic:\n{}",
        fx.log()
    );
}

/// D7: an unset or empty `AMPLIHACK_RUN_ID` and `AMPLIHACK_RUN_DIR` are
/// written as `null`, never left out.
#[test]
fn unset_or_empty_workspace_values_are_null() {
    let fx = Fx::new();
    let mut cmd = fx.step(Some(RUN_ID));
    cmd.env("AGENT_MODE", "gate")
        .env("AMPLIHACK_RUN_ID", "")
        .env_remove("AMPLIHACK_RUN_DIR");
    let mut launcher = fx.spawn_logged(cmd);
    fx.pid("agent.pid");
    let rec = read_json(&fx.the_record(RUN_ID));
    assert!(rec["workspace_run_id"].is_null(), "{rec}");
    assert!(rec["run_dir"].is_null(), "{rec}");
    fx.release();
    assert!(launcher.wait().unwrap().success());
}

/// A SIGKILLed launcher cannot remove its record. Once its agent has gone,
/// `recipe steps` reports the record `stale`, `--orphans` leaves it out,
/// and neither run removes it.
#[test]
fn a_killed_launcher_leaves_a_record_reported_stale() {
    let fx = Fx::new();
    let mut launcher = fx.spawn_logged(fx.step(Some(RUN_ID)));
    let agent = fx.pid("agent.pid");
    let path = fx.the_record(RUN_ID);

    sigkill(launcher.id() as i32);
    launcher.wait().unwrap();
    assert_gone(&[agent]);

    assert_eq!(states(&fx.steps_json(&[])), vec!["stale"]);
    assert!(fx.steps_json(&["--orphans"]).is_empty());
    assert!(path.is_file(), "recipe steps never removes a record");
}

/// D6: a regular file where `recipe-steps` belongs. This holds even when
/// the tests run as root, where `chmod` would not. The step still runs and
/// exits 0, with exactly the documented line.
#[test]
fn an_unsafe_record_directory_never_blocks_the_step() {
    let fx = Fx::new();
    fs::create_dir_all(fx.root().parent().unwrap()).unwrap();
    fs::write(fx.root(), "in the way").unwrap();

    let stderr = quick_step(&fx, RUN_ID);
    let want = format!(
        "amplihack: not writing a step record: unsafe directory {}; continuing",
        fx.root().display()
    );
    assert_eq!(
        record_lines(&stderr),
        vec![want.as_str()],
        "stderr:\n{stderr}"
    );
    assert_eq!(fs::read_to_string(fx.root()).unwrap(), "in the way");
}

/// D6 with a symlinked `recipe-steps`: refused, and nothing is written at
/// the link's target.
#[test]
fn a_symlinked_record_directory_is_refused_and_not_written_through() {
    let fx = Fx::new();
    let target = fx.work.join("elsewhere");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(fx.root().parent().unwrap()).unwrap();
    symlink(&target, fx.root()).unwrap();

    let stderr = quick_step(&fx, RUN_ID);
    let lines = record_lines(&stderr);
    assert_eq!(lines.len(), 1, "stderr:\n{stderr}");
    assert!(
        lines[0].starts_with("amplihack: not writing a step record: unsafe directory "),
        "{lines:?}"
    );
    assert_eq!(
        fs::read_dir(&target).unwrap().count(),
        0,
        "written through the link"
    );
}

/// D6: a run id outside `[A-Za-z0-9_-]{1,128}` writes nothing, prints the
/// one fixed line without the value, and the step still runs.
#[test]
fn an_invalid_run_id_writes_no_record_and_is_not_printed() {
    let fx = Fx::new();
    let stderr = quick_step(&fx, "../escape-2018");
    assert_eq!(
        record_lines(&stderr),
        vec!["amplihack: not writing a step record: invalid recipe run id; continuing"],
        "stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("escape-2018"),
        "the value must not be printed"
    );
    assert!(!fx.home.join(".amplihack/runtime/escape-2018").exists());
    let planted = fs::read_dir(fx.root()).map(|d| d.count()).unwrap_or(0);
    assert_eq!(planted, 0, "no record for an invalid run id");
}

/// An interactive launch writes no record and prints no record diagnostic.
#[test]
fn a_launch_without_a_run_id_writes_no_record() {
    let fx = Fx::new();
    let mut c = fx.step(None);
    c.env("AGENT_MODE", "exit");
    let out = c.output().expect("run amplihack claude");
    let stderr = text(&out.stderr);
    assert!(
        out.status.success(),
        "status {}; stderr:\n{stderr}",
        out.status
    );
    fx.pid("agent.pid");
    assert!(
        !fx.root().exists(),
        "no record directory for an interactive launch"
    );
    assert!(record_lines(&stderr).is_empty(), "stderr:\n{stderr}");
}
