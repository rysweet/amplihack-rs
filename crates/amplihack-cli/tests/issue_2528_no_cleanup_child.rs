//! Issue #2528 (T5, D10): no cleanup process outlives a recipe run.
//!
//! The first attempt at #2528 removed per-run bundle copies with detached
//! `recipe bundle-reap` processes, and one build lane leaked 847 of them.
//! Cleanup must happen in the run's own exit path, under its
//! `RuntimeGuard` or through its bundle lease, with no process, thread or
//! timer started for it.
//!
//! The run is started in its own session (`setsid` in `pre_exec`) and joined
//! with `wait()`. Every process the run starts, directly or through the
//! runner and its steps, inherits that session unless it calls `setsid`
//! itself; the steps here do not. After the join, one read of `/proc` must
//! find no process left in the session, and the snapshot path from the run's
//! early record must be gone. This is repeated for a run whose step fails.
//! The unwind case is the bundle lease unit test
//! `the_lease_is_released_on_panic_unwind` in `recipe/run/tests_bundle_pin.rs`.
//!
//! A detached reaper that calls `setsid` itself would escape the session
//! check, so the test also checks that no process outside the session has
//! the run's snapshot path or run id on its command line.
//!
//! No sleeps, polls or timeouts. Linux only.

#![cfg(target_os = "linux")]

use crate::issue_2528_one_bundle::{World, put, recipe, records, runner_or_skip};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

/// `(pid, session id, command line)` of every process, from one `/proc` read.
fn processes() -> Vec<(i32, i32, String)> {
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        // A process may exit between listing and reading; skip it.
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, after)) = stat.rsplit_once(')') else {
            continue;
        };
        // Fields after the comm: state(3) ppid(4) pgrp(5) session(6).
        let Some(session) = after
            .split_whitespace()
            .nth(3)
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        let cmdline = fs::read(entry.path().join("cmdline"))
            .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
            .unwrap_or_default();
        found.push((pid, session, cmdline));
    }
    found
}

/// Runs `recipe` in a new session, joins it, and checks nothing is left.
fn run_and_check(world: &World, runner: &Path, recipe_name: &str, expect_success: bool) {
    let mut cmd = world.command(
        Some(runner),
        &world.root,
        &[recipe_name, "-w", world.root.to_str().unwrap()],
    );
    let stderr_path = world.records.join(format!("{recipe_name}.stderr"));
    cmd.stdout(std::process::Stdio::null())
        .stderr(fs::File::create(&stderr_path).unwrap());
    // SAFETY: setsid is async-signal-safe and takes no arguments.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().expect("spawn recipe run");
    let session = child.id() as i32;
    let status = child.wait().expect("join recipe run");
    let stderr = fs::read_to_string(&stderr_path).unwrap();
    assert_eq!(status.success(), expect_success, "{recipe_name}:\n{stderr}");

    let early = records(&stderr)
        .into_iter()
        .find(|r| r["event"] == "early")
        .unwrap_or_else(|| panic!("no early record:\n{stderr}"));
    let snapshot = PathBuf::from(
        early["recipe_bundle"]["snapshot"]
            .as_str()
            .unwrap_or_else(|| panic!("no snapshot in {early}")),
    );
    let run_id = early["run_id"].as_str().unwrap().to_owned();

    let all = processes();
    let left: Vec<_> = all.iter().filter(|(_, s, _)| *s == session).collect();
    assert!(
        left.is_empty(),
        "processes from the run's session outlived it: {left:?}\n{stderr}"
    );
    let me = std::process::id() as i32;
    let snapshot_text = snapshot.to_str().unwrap();
    let escaped: Vec<_> = all
        .iter()
        .filter(|(pid, _, cmd)| {
            *pid != me && (cmd.contains(snapshot_text) || cmd.contains(&run_id))
        })
        .collect();
    assert!(
        escaped.is_empty(),
        "a process naming the run outlived it: {escaped:?}"
    );
    assert!(
        !snapshot.exists(),
        "snapshot {} is removed by the run's own exit path",
        snapshot.display()
    );
}

fn world_with(name: &str, body: &str) -> World {
    let world = World::new();
    let bundle = world.installed.join("amplifier-bundle");
    put(
        &bundle,
        "tools/identity.sh",
        "#!/bin/sh\necho tool=A\n",
        0o755,
    );
    put(
        &bundle,
        &format!("recipes/{name}.yaml"),
        &recipe(name, "T5", &[("step", body.to_owned())]),
        0o644,
    );
    world
}

#[test]
fn t5_a_successful_run_leaves_no_cleanup_child_and_no_snapshot() {
    let Some(runner) = runner_or_skip("t5 success") else {
        return;
    };
    let world = world_with(
        "t5-ok",
        "\"$AMPLIHACK_HOME/amplifier-bundle/tools/identity.sh\" > /dev/null\n",
    );
    run_and_check(&world, &runner, "t5-ok", true);
}

#[test]
fn t5_a_failed_run_leaves_no_cleanup_child_and_no_snapshot() {
    let Some(runner) = runner_or_skip("t5 failure") else {
        return;
    };
    let world = world_with("t5-fail", "exit 3\n");
    run_and_check(&world, &runner, "t5-fail", false);
}
