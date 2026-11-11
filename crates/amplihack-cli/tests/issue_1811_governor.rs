//! Issue #1811: the capacity governor, end to end through the real
//! `amplihack` binary. See `issue_1811_fixture` for the isolation rules and
//! docs/reference/capacity-governor.md for the contract.

#![cfg(unix)]

use crate::issue_1811_fixture::{Fixture, describe, now, text};
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};

const DENIED: i32 = 80;

/// Owns the wrapper and its FIFO-blocked shell/cat descendants from spawn
/// onward, including while an assertion or readiness wait unwinds.
struct GovernedChild(Option<Child>);

impl GovernedChild {
    fn spawn(cmd: &mut Command) -> Self {
        Self(Some(
            cmd.process_group(0)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn governed claude"),
        ))
    }

    fn try_output(&mut self) -> Option<Output> {
        self.0.as_mut()?.try_wait().expect("poll governed claude")?;
        Some(self.0.take().unwrap().wait_with_output().unwrap())
    }

    fn output(mut self) -> Output {
        // Leave timeout cleanup to Drop so the group is killed before its
        // leader is reaped and the PID can be reused.
        let mut output = None;
        crate::support::wait::wait_until("a governed claude to exit", || {
            output = self.try_output();
            output.is_some()
        });
        output.unwrap()
    }
}

impl Drop for GovernedChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            // SAFETY: spawn put this child in its own process group. The
            // negative PID targets only that group, including blocked stubs.
            unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
            let _ = child.wait();
        }
    }
}

#[test]
fn no_readings_fails_open_and_status_names_the_reason() {
    let f = Fixture::new();
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 1);
    let st = f.run(&["governor", "status", "--json"]);
    assert!(st.status.success(), "{}", describe(&st));
    let v: serde_json::Value = serde_json::from_slice(&st.stdout).unwrap();
    assert_eq!(v["fail_open_reason"]["code"], "no-readings", "{v}");
}

#[test]
fn weekly_exhaustion_exits_80_without_starting_claude() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0), ("beta", -1.0, 100.0)]);
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(DENIED), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 0, "the child must not start");
    let err = text(&out.stderr);
    assert!(
        err.contains("governor: not started (weekly-exhausted)"),
        "{err}"
    );
    assert!(
        !text(&out.stdout).contains("governor"),
        "admission writes nothing to stdout"
    );
}

#[test]
fn launch_is_governed_too() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 100.0)]);
    let out = f.run(&["launch", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(DENIED), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 0);
}

#[test]
fn room_on_another_profile_admits_and_says_so_on_stderr_only() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 30.0)]);
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 1);
    assert!(
        text(&out.stderr).contains("governor: admitted (room)"),
        "{}",
        describe(&out)
    );
    assert!(
        !text(&out.stdout).contains("governor"),
        "{}",
        describe(&out)
    );
}

#[test]
fn room_on_the_active_profile_prints_nothing() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 30.0), ("beta", 10.0, 30.0)]);
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        !text(&out.stderr).contains("governor"),
        "{}",
        describe(&out)
    );
}

#[test]
fn concurrent_starts_never_exceed_the_limit() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 20.0)]);
    f.governor_config("max_concurrency = 2");
    // Every admitted stub is held until the test releases it, so the runs
    // overlap for real however slow the host is (issue #1977: no sleep).
    let mut children: Vec<_> = (0..4)
        .map(|i| {
            GovernedChild::spawn(
                f.cmd(&[
                    "claude",
                    "--workstream",
                    &format!("ws-{i}"),
                    "--",
                    "-p",
                    "x",
                ])
                .env("AMPLIHACK_GOVERNOR_WAIT", "120")
                .env("STUB_HOLD", "1"),
            )
        })
        .collect();
    let mut released = std::collections::HashSet::new();
    let mut outputs = Vec::new();
    while released.len() < 4 {
        // Wait until as many stubs are held as the limit allows; release
        // them only then, so a governor that admitted more is seen.
        let want = (4 - released.len()).min(2);
        crate::support::wait::wait_until("the admitted stubs to be held", || {
            // A slot release may miss its lock on a loaded host. Reap
            // exited wrappers so their PIDs cannot keep those slots live.
            children.retain_mut(|child| {
                if let Some(output) = child.try_output() {
                    outputs.push(output);
                    false
                } else {
                    true
                }
            });
            f.held(&released).len() >= want
        });
        let held = f.held(&released);
        assert!(held.len() <= 2, "admitted {} at once: {held:?}", held.len());
        for pid in held {
            f.release(&pid);
            released.insert(pid);
        }
    }
    outputs.extend(children.into_iter().map(GovernedChild::output));
    for out in outputs {
        assert!(out.status.success(), "{}", describe(&out));
    }
    assert_eq!(f.claude_runs(), 4);
    assert!(f.max_overlap() <= 2, "overlap {}", f.max_overlap());
    let st = f.run(&["governor", "status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&st.stdout).unwrap();
    assert_eq!(
        v["slots"],
        serde_json::json!([]),
        "every slot is released: {v}"
    );
}

#[test]
fn governor_failures_kill_and_reap_wrappers_and_blocked_descendants() {
    use std::io::Read;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc;
    use std::time::Duration;

    for timeout in [false, true] {
        let f = Fixture::new();
        f.readings(&[("alpha", 10.0, 20.0)]);
        f.governor_config("max_concurrency = 2");
        let (tx, rx) = mpsc::channel();
        let mut pids = Vec::new();
        let mut held_before_failure = false;
        let failed = catch_unwind(AssertUnwindSafe(|| {
            let children: Vec<_> = (0..3)
                .map(|_| {
                    let mut child = GovernedChild::spawn(
                        f.cmd(&["claude", "--", "-p", "x"])
                            .env("AMPLIHACK_GOVERNOR_WAIT", "120")
                            .env("STUB_HOLD", "1"),
                    );
                    let wrapper = child.0.as_mut().unwrap();
                    pids.push(wrapper.id());
                    let mut pipe = wrapper.stdout.take().unwrap();
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        let result = pipe.read_to_end(&mut Vec::new());
                        let _ = tx.send(result);
                    });
                    child
                })
                .collect();
            crate::support::wait::wait_until("FIFO-held stubs before a fixture failure", || {
                f.held(&Default::default()).len() >= 2
            });
            held_before_failure = true;
            if timeout {
                crate::support::wait::wait_until_with_ceiling(
                    "an intentionally missing readiness event",
                    Duration::ZERO,
                    || false,
                );
            } else {
                assert!(
                    children.is_empty(),
                    "intentional admission assertion failure"
                );
            }
        }));
        assert!(failed.is_err(), "the failure path must unwind");
        assert!(held_before_failure, "the stubs must be held before failing");
        for pid in pids {
            // SAFETY: waitpid writes nothing with a null status pointer.
            let waited =
                unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG) };
            assert_eq!(waited, -1, "wrapper {pid} was not reaped");
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
            // EOF requires every inherited writer (wrapper, shell and cat)
            // to close, so killing just the wrapper cannot pass this check.
            rx.recv_timeout(crate::support::wait::CEILING)
                .expect("waiting for killed governor descendants to close stdout")
                .expect("read governor stdout");
        }
    }
}

#[test]
fn a_dead_process_slot_is_reclaimed() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 20.0)]);
    f.governor_config("max_concurrency = 1");
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    fs::create_dir_all(f.gov()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(f.gov(), fs::Permissions::from_mode(0o700)).unwrap();
    let state = serde_json::json!({
        "version": 1, "limit": 1, "last_evaluated_at": now(), "next_seq": 1,
        "slots": [{"pid": dead, "start_time": 1, "workstream": "ws-dead", "class": "ordinary",
                   "tree_id": "t-dead", "admitted_at": now() - 60, "nested_children": 0}],
        "admissions": [], "fail_open": null
    });
    fs::write(f.gov().join("state.json"), state.to_string()).unwrap();
    fs::set_permissions(
        f.gov().join("state.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(
        out.status.success(),
        "the dead slot must not hold the start: {}",
        describe(&out)
    );
    assert_eq!(f.claude_runs(), 1);
}

#[test]
fn recipe_run_with_limit_one_admits_its_agent_steps_as_nested() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 20.0)]);
    f.governor_config("max_concurrency = 1");
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "nested steps must not deadlock: {}",
        describe(&out)
    );
    assert_eq!(f.claude_runs(), 2);
    let tree_ids: std::collections::BTreeSet<String> = fs::read_dir(&f.records)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with("env."))
        .flat_map(|p| {
            fs::read_to_string(p)
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|l| l.starts_with("AMPLIHACK_TREE_ID="))
        .collect();
    assert_eq!(
        tree_ids.len(),
        1,
        "every step carries the slot's tree id: {tree_ids:?}"
    );
}

#[test]
fn recipe_run_with_exhausted_budget_exits_80_before_the_runner() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 100.0)]);
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(DENIED), "{}", describe(&out));
    assert!(
        !f.records.join("runner").exists(),
        "recipe-runner-rs must not start"
    );
}

#[test]
fn task_text_never_reaches_state_or_status() {
    const CANARY: &str = "TASKSECRET";
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 20.0)]);
    let out = f.run(&["claude", "--", "-p", "TASKSECRET do the thing"]);
    assert!(out.status.success(), "{}", describe(&out));
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .args(["-c", "task_description=TASKSECRET in a recipe"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let files = f.tree(&f.gov());
    assert!(files.len() > 1, "the governor saved no state");
    for (path, bytes) in files {
        assert!(!text(&bytes).contains(CANARY), "{}", path.display());
    }
    for args in [
        &["governor", "status", "--json"][..],
        &["governor", "status"],
    ] {
        let st = f.run(args);
        assert!(st.status.success(), "{}", describe(&st));
        assert!(!text(&st.stdout).contains(CANARY), "{args:?}");
        assert!(!text(&st.stderr).contains(CANARY), "{args:?}");
    }
}
