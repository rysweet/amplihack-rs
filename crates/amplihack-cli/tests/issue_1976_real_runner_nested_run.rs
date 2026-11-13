//! Issue #1976, crusty review of PR #2112 (concern 3): the orphan watch under
//! the real `recipe-runner-rs`, not a stub runner.
//!
//! The other #1976 tests replace the runner with a shell stub and use `cat`
//! as the parent. If the real runner filtered a step's environment, a nested
//! run would never see `AMPLIHACK_PARENT_RECIPE_PID` and `_STARTTIME`, the
//! watch would do nothing in production, and those tests would still pass.
//! This test closes that gap:
//!
//! 1. A real outer `amplihack recipe run` runs one bash step under the real
//!    runner. The step records the two variables and the `/proc` stat of the
//!    process they name, then starts a nested `amplihack recipe run`.
//! 2. The nested run's own step, also under the real runner, writes a
//!    readiness file and blocks reading a FIFO nobody writes.
//! 3. The test kills the outer `recipe run` with `SIGKILL`, so it cannot tear
//!    anything down. The outer step's shell lives on (the runner has its own
//!    process group) and writes the nested run's exit code.
//!
//! The test asserts that the step saw the outer run's own pid and start time,
//! and that the nested run stopped with exit 75 and the orphan line naming
//! that pid. No sleeps and no wall-clock windows: the waits are the readiness
//! file, the exit-code file and child exits.
//!
//! The real runner is `RECIPE_RUNNER_QA_BINARY`, else `recipe-runner-rs` on
//! `PATH`. Without one the test says so and returns, unless
//! `AMPLIHACK_1976_REQUIRE_RECIPE_RUNNER` is set, when it fails. Linux only.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::wait::{wait_child, wait_until};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REQUIRE_ENV: &str = "AMPLIHACK_1976_REQUIRE_RECIPE_RUNNER";

/// The real runner, or `None` when none is installed.
fn real_runner() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("RECIPE_RUNNER_QA_BINARY") {
        return Some(PathBuf::from(explicit));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("recipe-runner-rs"))
        .find(|candidate| {
            candidate.is_file()
                && Command::new(candidate)
                    .arg("--version")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
        })
}

fn starttime_in(stat: &str) -> u64 {
    stat.rsplit_once(')')
        .expect("stat has a `)`")
        .1
        .split_whitespace()
        .nth(19)
        .expect("stat has field 22")
        .parse()
        .expect("field 22 is a number")
}

fn quoted(path: &Path) -> String {
    let text = path.to_str().expect("UTF-8 temp path");
    assert!(!text.contains('\''), "temp path holds a quote: {text}");
    format!("'{text}'")
}

/// A one-step bash recipe; `body` is indented under `command: |`.
fn recipe(name: &str, body: &str) -> String {
    let block: String = body.lines().map(|line| format!("      {line}\n")).collect();
    format!(
        "name: {name}\ndescription: issue 1976 real-runner check\nversion: 1.0.0\n\
         context: {{}}\nsteps:\n  - id: step\n    type: bash\n    command: |\n{block}"
    )
}

/// On any exit, kill the outer run if it lives and release a step still
/// blocked on the FIFO, so a failed assertion leaves nothing running.
struct Cleanup {
    fifo: PathBuf,
    parent_pid: Option<i32>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(pid) = self.parent_pid {
            // SAFETY: a process this test started; it is reaped by the test
            // or, if the test failed first, by init.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        // A non-blocking open succeeds only while a reader waits; closing it
        // gives that reader end of file. ENXIO (no reader) is fine.
        use std::os::unix::fs::OpenOptionsExt;
        let _ = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.fifo);
    }
}

#[test]
fn a_step_under_the_real_runner_gets_the_parent_variables_and_its_nested_run_stops_with_75() {
    let Some(runner) = real_runner() else {
        assert!(
            std::env::var_os(REQUIRE_ENV).is_none(),
            "{REQUIRE_ENV} is set but no recipe-runner-rs was found"
        );
        eprintln!("recipe-runner-rs unavailable; skipping the real-runner orphan check");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (home, records) = (root.join("home"), root.join("records"));
    for dir in [&home, &records] {
        fs::create_dir_all(dir).unwrap();
    }
    let fifo = records.join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo")
            .success()
    );
    let amplihack = amplihack_bin();

    let inner = root.join("inner.yaml");
    fs::write(
        &inner,
        recipe(
            "issue-1976-real-runner-inner",
            &format!(
                ": > {ready}\nread line < {fifo}\n",
                ready = quoted(&records.join("ready")),
                fifo = quoted(&fifo),
            ),
        ),
    )
    .unwrap();
    let outer = root.join("outer.yaml");
    fs::write(
        &outer,
        recipe(
            "issue-1976-real-runner-outer",
            &format!(
                "printf '%s\\n%s\\n' \"${{AMPLIHACK_PARENT_RECIPE_PID:-}}\" \
                 \"${{AMPLIHACK_PARENT_RECIPE_STARTTIME:-}}\" > {vars}\n\
                 cat \"/proc/${{AMPLIHACK_PARENT_RECIPE_PID:-0}}/stat\" > {stat} 2>/dev/null || true\n\
                 {amplihack} recipe run {inner} -w {root} > /dev/null 2> {nested_err}\n\
                 echo $? > {rc_tmp} && mv {rc_tmp} {rc}\n",
                vars = quoted(&records.join("step-vars.txt")),
                stat = quoted(&records.join("parent.stat")),
                amplihack = quoted(&amplihack),
                inner = quoted(&inner),
                root = quoted(&root),
                nested_err = quoted(&records.join("nested.stderr")),
                rc_tmp = quoted(&records.join("nested.rc.tmp")),
                rc = quoted(&records.join("nested.rc")),
            ),
        ),
    )
    .unwrap();

    let path_env = format!(
        "{}:/usr/local/bin:/usr/bin:/bin",
        runner.parent().expect("runner has a directory").display()
    );
    let outer_err = root.join("outer.stderr");
    let mut cmd = crate::isolated_amplihack::isolated(&amplihack);
    cmd.args(["recipe", "run"])
        .arg(&outer)
        .arg("-w")
        .arg(&root)
        .current_dir(&root)
        .env_clear()
        .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
        .env("TMPDIR", std::env::temp_dir())
        .env("PATH", &path_env)
        .env("HOME", &home)
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env(
            "AMPLIHACK_HOME",
            crate::support::installed_bundle::home(home.join(".amplihack")),
        )
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
        .env("AMPLIHACK_SKIP_REFLECTION", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&outer_err).unwrap());
    let mut parent = cmd.spawn().expect("spawn the outer recipe run");
    let parent_pid = parent.id();
    let mut cleanup = Cleanup {
        fifo: fifo.clone(),
        parent_pid: Some(parent_pid as i32),
    };

    let ready = records.join("ready");
    let rc_file = records.join("nested.rc");
    wait_until(
        "the nested run's step to start, or the nested run to end",
        || ready.is_file() || rc_file.is_file() || matches!(parent.try_wait(), Ok(Some(_))),
    );
    let describe = || {
        format!(
            "outer stderr:\n{}\nnested stderr:\n{}",
            fs::read_to_string(&outer_err).unwrap_or_default(),
            fs::read_to_string(records.join("nested.stderr")).unwrap_or_default()
        )
    };
    assert!(
        ready.is_file(),
        "the nested run's step never started\n{}",
        describe()
    );

    // The premise: the real runner passes both variables to the step, and
    // they name the outer `recipe run` itself.
    let vars = fs::read_to_string(records.join("step-vars.txt")).unwrap();
    let mut lines = vars.lines();
    let (pid, start) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""));
    assert_eq!(
        pid,
        parent_pid.to_string(),
        "the step's AMPLIHACK_PARENT_RECIPE_PID names the outer run\n{}",
        describe()
    );
    let stat = fs::read_to_string(records.join("parent.stat")).unwrap();
    assert_eq!(
        start,
        starttime_in(&stat).to_string(),
        "the step's AMPLIHACK_PARENT_RECIPE_STARTTIME is the outer run's start time"
    );

    // The behaviour: the outer run dies without teardown, and the nested run
    // stops itself.
    parent.kill().expect("SIGKILL the outer recipe run");
    wait_child("the outer recipe run to be reaped", &mut parent);
    cleanup.parent_pid = None;
    wait_until("the nested run's exit code", || rc_file.is_file());
    let rc = fs::read_to_string(&rc_file).unwrap();
    let nested_err = fs::read_to_string(records.join("nested.stderr")).unwrap();
    assert_eq!(rc.trim(), "75", "the nested run exits 75\n{nested_err}");
    assert!(
        nested_err.contains(&format!(
            "orphaned: parent recipe run {parent_pid} exited; stopping"
        )),
        "the nested run names the orphan stop:\n{nested_err}"
    );
}
