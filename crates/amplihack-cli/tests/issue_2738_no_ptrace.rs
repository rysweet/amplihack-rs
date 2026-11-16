//! Issue #2738: a real recipe run starts and finishes with nothing tracing it.
//!
//! #2290 added a required Linux observer that attached to the recipe runner
//! and every process below it with `ptrace`, so a host or container that
//! denies `ptrace` could not run a recipe at all, and a Git command the
//! observer could not attribute ended the run (#2737). The observer is gone.
//!
//! The kernel states the absence positively: `TracerPid:` in
//! `/proc/<pid>/status` is the pid of whatever is tracing that process, and
//! `0` when nothing is. This test starts a real `amplihack recipe run` under
//! the pinned `recipe-runner-rs`, waits for the step to signal that it is
//! running, and reads `TracerPid` for the run and for every process below it.
//! The tree is asserted non-empty first, so a run that never spawned anything
//! cannot pass the check vacuously.
//!
//! Only `Name:`, `PPid:` and `TracerPid:` are read. A failure prints a pid,
//! its tracer and its `comm`, never a command line or an environment, so a
//! failing run in CI cannot publish a subprocess's environment.
//!
//! The real runner is the receipt-checked pinned build from
//! `tests/support/pinned_runner.rs` (#2658), never a `PATH` lookup. The test
//! never skips: without a runner at the pin it fails. Linux only.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::wait::{wait_child, wait_until};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn real_runner() -> PathBuf {
    crate::pinned_runner::pinned_runner(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// The three `/proc/<pid>/status` fields this test reads. Nothing else about
/// a process is read or printed.
#[derive(Clone, Debug)]
struct Status {
    pid: i32,
    comm: String,
    ppid: i32,
    tracer: i32,
}

/// `None` when the process is gone or its status cannot be parsed; a process
/// that exits mid-scan is not part of the run's live tree.
fn status_of(pid: i32) -> Option<Status> {
    let text = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let field = |name: &str| -> Option<String> {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .map(|rest| rest.trim().to_owned())
    };
    Some(Status {
        pid,
        comm: field("Name:")?,
        ppid: field("PPid:")?.parse().ok()?,
        tracer: field("TracerPid:")?.parse().ok()?,
    })
}

fn live_processes() -> Vec<Status> {
    fs::read_dir("/proc")
        .expect("read /proc")
        .filter_map(|entry| {
            entry
                .ok()?
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
        })
        .filter_map(status_of)
        .collect()
}

/// `root` and every live process below it, parents before children.
fn tree_from(root: i32) -> Vec<Status> {
    let all = live_processes();
    let mut tree: Vec<Status> = all.iter().filter(|s| s.pid == root).cloned().collect();
    let mut index = 0;
    while index < tree.len() {
        let parent = tree[index].pid;
        for candidate in &all {
            if candidate.ppid == parent && !tree.iter().any(|s| s.pid == candidate.pid) {
                tree.push(candidate.clone());
            }
        }
        index += 1;
    }
    tree
}

fn describe(tree: &[Status]) -> String {
    tree.iter()
        .map(|s| format!("  pid {} ({}) TracerPid {}\n", s.pid, s.comm, s.tracer))
        .collect()
}

fn quoted(path: &Path) -> String {
    let text = path.to_str().expect("UTF-8 temp path");
    assert!(!text.contains('\''), "temp path holds a quote: {text}");
    format!("'{text}'")
}

fn recipe(name: &str, body: &str) -> String {
    let block: String = body.lines().map(|line| format!("      {line}\n")).collect();
    format!(
        "name: {name}\ndescription: issue 2738 no-tracer check\nversion: 1.0.0\n\
         context: {{}}\nsteps:\n  - id: step\n    type: bash\n    command: |\n{block}"
    )
}

/// On any exit, release a step still blocked on the FIFO and kill the run if
/// it is still alive, so a failed assertion leaves nothing running.
struct Cleanup {
    fifo: PathBuf,
    run_pid: Option<i32>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.fifo);
        if let Some(pid) = self.run_pid {
            // SAFETY: a process this test started and recorded.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

#[test]
fn a_real_recipe_run_and_every_process_below_it_have_no_tracer_and_the_run_finishes() {
    let runner = real_runner();
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
    let ready = records.join("ready");
    let recipe_path = root.join("recipe.yaml");
    fs::write(
        &recipe_path,
        recipe(
            "issue-2738-no-ptrace",
            &format!(
                ": > {ready}\nread line < {fifo}\n",
                ready = quoted(&ready),
                fifo = quoted(&fifo),
            ),
        ),
    )
    .unwrap();

    let path_env = format!(
        "{}:/usr/local/bin:/usr/bin:/bin",
        runner.parent().expect("runner has a directory").display()
    );
    let run_err = root.join("run.stderr");
    let mut cmd = crate::isolated_amplihack::isolated(&amplihack);
    cmd.args(["recipe", "run"])
        .arg(&recipe_path)
        .arg("-w")
        .arg(&root)
        .current_dir(&root)
        .env_clear()
        .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
        .env("TMPDIR", std::env::temp_dir())
        .env("PATH", &path_env)
        .env("HOME", &home)
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("AMPLIHACK_HOME", home.join(".amplihack"))
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
        .env("AMPLIHACK_SKIP_REFLECTION", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&run_err).unwrap());
    let mut run = cmd.spawn().expect("spawn the recipe run");
    let run_pid = run.id() as i32;
    let mut cleanup = Cleanup {
        fifo: fifo.clone(),
        run_pid: Some(run_pid),
    };

    wait_until("the run's step to start, or the run to end", || {
        ready.is_file() || matches!(run.try_wait(), Ok(Some(_)))
    });
    let stderr_of_run =
        || fs::read_to_string(&run_err).unwrap_or_else(|e| format!("<unreadable: {e}>"));
    assert!(
        ready.is_file(),
        "the run's step never started\nrun stderr:\n{}",
        stderr_of_run()
    );

    // The tree is read while the run is in flight, so the processes the
    // observer used to hold are exactly the ones checked here.
    let tree = tree_from(run_pid);
    assert!(
        tree.iter().any(|s| s.pid == run_pid),
        "the recipe run {run_pid} was not in the process table while its step ran"
    );
    let below: Vec<&Status> = tree.iter().filter(|s| s.pid != run_pid).collect();
    assert!(
        !below.is_empty(),
        "no process below the recipe run {run_pid}; the check would pass vacuously\n\
         run stderr:\n{}",
        stderr_of_run()
    );
    let traced: Vec<Status> = tree.iter().filter(|s| s.tracer != 0).cloned().collect();
    let traced_text = describe(&traced);
    let tree_text = describe(&tree);
    assert!(
        traced.is_empty(),
        "a recipe run must start with nothing tracing it, but {} of {} processes \
         have a tracer:\n{traced_text}whole tree:\n{tree_text}",
        traced.len(),
        tree.len(),
    );

    // The run's status is the runner's own exit: release the step and let it
    // finish on its own. A whole line, not just end-of-file: the step's `read`
    // reports failure at end-of-file, which would end the run with the step's
    // own exit 1 and say nothing about the tracer.
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut writer = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .expect("open the FIFO while the step reads it");
        writer.write_all(b"go\n").expect("release the step");
    }
    // Cleared before the wait, not after: every panic path inside `wait_child`
    // kills and reaps the child itself, so a `Drop` kill after one would signal
    // a pid the kernel has already freed.
    cleanup.run_pid = None;
    let status = wait_child("the recipe run to finish", &mut run);
    assert!(
        status.success(),
        "the recipe run ended with {status}\nrun stderr:\n{}",
        stderr_of_run()
    );
}
