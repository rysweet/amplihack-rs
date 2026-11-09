//! Issue #2018 (part 1): a headless recipe step stops when the process that
//! started it exits, and an interactive launch keeps its current behaviour.
//!
//! Black-box through the real binary, with the fixture in
//! `support::step_2018`. A stand-in runner (`sh`) starts `amplihack claude`
//! the way `recipe-runner-rs` does, as a direct child forked from a thread
//! that outlives the step, with `AMPLIHACK_RECIPE_RUN_ID` set and stdout to
//! a file. The parent-death signal cannot be observed from a shell stub, so
//! each guarantee is tested by its effect. Every wait is on an event: a
//! pidfd for an exit, `Child::wait` for a child, a FIFO for a release.
//! Linux only (`/proc`, `PR_SET_PDEATHSIG`).

#![cfg(target_os = "linux")]

use crate::support::proc_watch::{alive, assert_gone, ppid, wait_gone_or_fs};
use crate::support::step_2018::{Fx, SUPERVISED, UNSUPERVISED, sigkill, signal, text};
use serde_json::{Value, json};
use std::fs;

const RUN_ID: &str = "issue-2018-lifetime-run";

/// SIGKILL to the runner ends the launcher (its parent-death signal) and,
/// through the launcher's SIGTERM handling, the agent. The launcher leaves
/// through its graceful path, so its record goes too.
#[test]
fn a_killed_runner_takes_its_headless_step_with_it() {
    let fx = Fx::new();
    let mut runner = fx.runner(RUN_ID, UNSUPERVISED);
    let runner_pid = fx.pid("runner.pid");
    let launcher = fx.pid("launcher.pid");
    let agent = fx.pid("agent.pid");
    assert_eq!(runner_pid, runner.id() as i32);
    fx.the_record(RUN_ID);
    assert!(
        alive(launcher) && alive(agent),
        "the step must be running before the kill"
    );

    sigkill(runner_pid);
    runner.wait().unwrap();

    assert_gone(&[launcher, agent]);
    assert!(
        !fx.run_dir(RUN_ID).exists(),
        "a launcher ended by SIGTERM removes its record and the empty run-id \
         directory; left {:?}; launcher log:\n{}",
        fx.entries(RUN_ID),
        fx.log()
    );
}

/// The runner dies while the launcher is still starting up (update notice,
/// self-heal, asset repair), before dispatch binds it. The launcher has then
/// been reparented, so a parent read at the binding would be the reaper and
/// would pass the "parent already gone" check. The launcher must compare
/// against the parent it read at the start of `main`: it starts no agent,
/// writes no record and exits with status 1.
///
/// The debug-only `AMPLIHACK_TEST_STARTUP_GATE` holds the launcher between
/// self-heal and dispatch. The launcher opens the FIFO for reading; this
/// test's open for writing returns once the launcher is at the gate, and the
/// launcher goes on when the write end is closed.
#[cfg(debug_assertions)]
#[test]
fn a_runner_that_dies_while_the_launcher_starts_up_starts_no_agent() {
    let fx = Fx::new();
    let gate = fx.fifo("startup-gate");
    let mut cmd = fx.runner_cmd(RUN_ID, UNSUPERVISED);
    cmd.env("AMPLIHACK_TEST_STARTUP_GATE", &gate);
    let mut runner = cmd.spawn().expect("start the stand-in runner");
    let runner_pid = fx.pid("runner.pid");
    let launcher = fx.pid("launcher.pid");

    let held = fs::OpenOptions::new()
        .write(true)
        .open(&gate)
        .expect("open the startup gate");
    assert_eq!(ppid(launcher), Some(runner_pid), "held inside startup");
    sigkill(runner_pid);
    runner.wait().unwrap();
    assert_ne!(
        ppid(launcher),
        Some(runner_pid),
        "the launcher has been reparented"
    );
    drop(held);

    // Ends on whichever comes first: the launcher exiting, or an agent
    // starting (its pid file is renamed into place). A launcher that wrongly
    // starts the agent then fails below instead of hanging.
    let agent_pid = fx.records.join("agent.pid");
    wait_gone_or_fs(launcher, &fx.records, || agent_pid.exists());
    let log = fx.log();
    assert!(
        log.contains("amplihack: headless step parent exited before launch; not starting"),
        "the launcher must refuse to start; log:\n{log}"
    );
    assert!(!agent_pid.exists(), "no agent may start; log:\n{log}");
    assert_gone(&[launcher]);
    assert!(
        !fx.run_dir(RUN_ID).exists(),
        "no record may be written; left {:?}",
        fx.entries(RUN_ID)
    );
}

/// D2 and D4: with the usage-limit supervisor in the chain, the supervisor
/// binds to the runner in its dispatch arm and its attempt child binds to
/// the supervisor. The attempt child writes the one record and names the
/// supervisor as its parent. Killing the runner ends the whole chain.
#[test]
fn a_killed_runner_also_ends_a_supervised_step() {
    let fx = Fx::new();
    let mut runner = fx.runner(RUN_ID, SUPERVISED);
    let runner_pid = fx.pid("runner.pid");
    let supervisor = fx.pid("launcher.pid");
    let agent = fx.pid("agent.pid");
    let attempt = ppid(agent).expect("the agent is running");
    fx.remember("attempt.pid", attempt);
    assert_ne!(
        attempt,
        supervisor,
        "the supervisor must sit between the runner and the agent; log:\n{}",
        fx.log()
    );
    assert_eq!(
        ppid(attempt),
        Some(supervisor),
        "attempt child of the supervisor"
    );

    let path = fx.the_record(RUN_ID);
    let rec: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        rec["launcher"]["pid"],
        json!(attempt),
        "the attempt child writes it"
    );
    assert_eq!(
        rec["parent"]["pid"],
        json!(supervisor),
        "and names the supervisor"
    );
    assert_eq!(rec["agent"]["pid"], json!(agent));

    sigkill(runner_pid);
    runner.wait().unwrap();

    assert_gone(&[supervisor, attempt, agent]);
    assert!(
        !fx.run_dir(RUN_ID).exists(),
        "the attempt child's record must be removed; left {:?}; log:\n{}",
        fx.entries(RUN_ID),
        fx.log()
    );
}

/// SIGKILL to the launcher alone ends the agent (the agent's parent-death
/// signal). The record is left behind by design; `issue_2018_step_records`
/// checks how `recipe steps` reports it.
#[test]
fn a_killed_launcher_takes_its_agent_with_it() {
    let fx = Fx::new();
    let mut launcher = fx.spawn_logged(fx.step(Some(RUN_ID)));
    let agent = fx.pid("agent.pid");
    assert!(alive(agent));
    assert_eq!(
        ppid(agent),
        Some(launcher.id() as i32),
        "the launcher is the parent"
    );

    sigkill(launcher.id() as i32);
    launcher.wait().unwrap();

    assert_gone(&[agent]);
}

/// D1: a headless launcher stopped by a signal ends its agent, removes its
/// record and exits `128 + N`, so a runner that is still alive sees the
/// step fail.
#[test]
fn a_signalled_headless_launcher_exits_128_plus_n_and_removes_its_record() {
    for (sig, code) in [
        (libc::SIGTERM, 143),
        (libc::SIGINT, 130),
        (libc::SIGHUP, 129),
    ] {
        let fx = Fx::new();
        let mut launcher = fx.spawn_logged(fx.step(Some(RUN_ID)));
        let agent = fx.pid("agent.pid");
        let path = fx.the_record(RUN_ID);

        signal(launcher.id() as i32, sig);
        let status = launcher.wait().unwrap();

        assert_eq!(
            status.code(),
            Some(code),
            "signal {sig}; log:\n{}",
            fx.log()
        );
        assert_gone(&[agent]);
        assert!(!path.exists(), "signal {sig}: the record must be removed");
        assert!(
            !fx.run_dir(RUN_ID).exists(),
            "signal {sig}: and its empty directory"
        );
    }
}

/// #1424 is unchanged: an interactive launch stopped by SIGTERM exits 0.
#[test]
fn a_signalled_interactive_launch_still_exits_0() {
    let fx = Fx::new();
    let mut launcher = fx.spawn_logged(fx.step(None));
    let agent = fx.pid("agent.pid");

    signal(launcher.id() as i32, libc::SIGTERM);
    let status = launcher.wait().unwrap();

    assert_eq!(status.code(), Some(0), "log:\n{}", fx.log());
    assert_gone(&[agent]);
}

/// Without `AMPLIHACK_RECIPE_RUN_ID` the launch is interactive and unbound,
/// so the agent outlives a SIGKILLed launcher as it always has.
///
/// The agent is a `gate` stub. A bound agent would have SIGTERM queued by
/// the kernel before `launcher.wait()` returns (the signal is sent while the
/// launcher exits, and the launcher cannot be reaped before that), and the
/// stub would die on it before it ran another instruction. So an agent that
/// is released after the launcher is reaped, and then writes
/// `agent.finished`, was not bound.
#[test]
fn an_interactive_launch_is_not_bound_to_its_launcher() {
    let fx = Fx::new();
    let mut cmd = fx.step(None);
    cmd.env("AGENT_MODE", "gate");
    let mut launcher = fx.spawn_logged(cmd);
    let agent = fx.pid("agent.pid");

    sigkill(launcher.id() as i32);
    launcher.wait().unwrap();

    fx.release();
    assert_gone(&[agent]);
    assert!(
        fx.agent_finished(),
        "an interactive launch must keep its current behaviour; log:\n{}",
        fx.log()
    );
}

/// An empty run id is the same as none: unbound, and no record. The agent
/// is checked as in `an_interactive_launch_is_not_bound_to_its_launcher`.
#[test]
fn an_empty_run_id_is_not_a_headless_step() {
    let fx = Fx::new();
    let mut cmd = fx.step(Some(""));
    cmd.env("AGENT_MODE", "gate");
    let mut launcher = fx.spawn_logged(cmd);
    let agent = fx.pid("agent.pid");

    sigkill(launcher.id() as i32);
    launcher.wait().unwrap();

    fx.release();
    assert_gone(&[agent]);
    assert!(fx.agent_finished(), "log:\n{}", fx.log());
    assert!(!fx.root().exists(), "no record without a run id");
}

/// When the agent exits 0, the launcher exits 0, removes its record and the
/// empty run-id directory, and prints no record diagnostic.
#[test]
fn a_step_that_ends_normally_leaves_no_record() {
    let fx = Fx::new();
    let mut c = fx.step(Some(RUN_ID));
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
        !fx.run_dir(RUN_ID).exists(),
        "record directory left behind: {:?}",
        fx.entries(RUN_ID)
    );
    assert!(
        !stderr.contains("step record"),
        "a clean step prints no record diagnostic; stderr:\n{stderr}"
    );
}
