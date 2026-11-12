//! Issue #2486 (refs #2472): a run's runtime folder is not removed while a
//! process still uses it, and a step never starts with a missing TMPDIR.
//!
//! - A child holding the run's TMPDIR when the run ends keeps the folder.
//!   `amplihack` says so once on stderr, and a later `amplihack cleanup`
//!   removes it after the child has exited.
//! - A step started with a missing inherited TMPDIR gets its own run's
//!   TMPDIR (normal runs), or the run is refused with the path named (dry
//!   runs, which have no run folder of their own).
//!
//! The run is held open with two FIFOs, not with time: the stub runner
//! writes its TMPDIR to `out.fifo`, then blocks reading `go.fifo` until the
//! test has started and registered the holder. Every process is reaped with
//! `wait()` / `wait_with_output()`; nothing sleeps, polls or times out.
//! `amplihack` reads the fixture's fake process table, into which the test
//! links only the holder it started.

use super::fixture::*;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

const OUT_FIFO_ENV: &str = "ISSUE_2486_OUT_FIFO";
const GO_FIFO_ENV: &str = "ISSUE_2486_GO_FIFO";
/// Written to `out.fifo` by the test itself when `amplihack` exits before
/// the stub reported, so the reader never blocks on a run that is gone.
const EXITED: &str = "__amplihack_exited__";

fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path for the call's duration.
    let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(
        rc,
        0,
        "mkfifo {}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
}

/// Opens `fifo` for writing without blocking and writes `line`. Fails
/// quietly when nobody has it open for reading.
fn poke(fifo: &Path, line: &str) {
    if let Ok(mut f) = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(fifo)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Releases the stub blocked on `go.fifo`. On drop (a failed assertion
/// before the release) it still unblocks a stub already waiting, so no
/// process is left hanging on the FIFO.
struct Gate {
    go: PathBuf,
    released: bool,
}

impl Gate {
    fn release(mut self) {
        // Blocking open: returns once the stub opens its read end.
        let mut f = fs::OpenOptions::new().write(true).open(&self.go).unwrap();
        writeln!(f, "go").unwrap();
        self.released = true;
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        if !self.released {
            poke(&self.go, "go");
        }
    }
}

/// A `cat` the test started; killed and reaped if the test fails first.
struct Holder(Child);

impl Holder {
    fn spawn(fx: &Fx, dir: &Path) -> Self {
        let child = Command::new(fx.bin.join("cat"))
            .current_dir(dir)
            .env_clear()
            .env("TMPDIR", dir)
            .env("TMP", dir)
            .env("TEMP", dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn holder");
        Self(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    /// Closes stdin and waits for the holder to exit.
    fn finish(mut self) {
        drop(self.0.stdin.take());
        let status = self.0.wait().expect("wait for holder");
        assert!(status.success(), "holder exited with {status}");
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A child holding the run's TMPDIR when the run ends keeps the runtime
/// folder; a sweep while it runs keeps it too; once it has exited, the next
/// `amplihack cleanup` removes it.
#[test]
fn child_holding_run_tmpdir_keeps_folder_until_it_exits_then_sweep_removes_it() {
    let fx = Fx::new();
    let out_fifo = fx.root.join("out.fifo");
    let go_fifo = fx.root.join("go.fifo");
    mkfifo(&out_fifo);
    mkfifo(&go_fifo);
    // O_RDWR: opening never blocks, and a writer always finds a reader.
    let out_end = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&out_fifo)
        .unwrap();
    let gate = Gate {
        go: go_fifo.clone(),
        released: false,
    };

    let runner = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$TMPDIR\" > \"${OUT_FIFO_ENV}\"\n\
         read _go < \"${GO_FIFO_ENV}\"\n\
         {RUNNER_SUCCESS_JSON}\n"
    );
    let child = fx
        .recipe_run_command(&runner, |c| {
            c.env(OUT_FIFO_ENV, &out_fifo)
                .env(GO_FIFO_ENV, &go_fifo)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        })
        .spawn()
        .expect("spawn amplihack recipe run");
    let waiter = {
        let out_fifo = out_fifo.clone();
        std::thread::spawn(move || {
            let out = child.wait_with_output().expect("wait for amplihack");
            poke(&out_fifo, EXITED);
            out
        })
    };

    let mut line = String::new();
    BufReader::new(&out_end).read_line(&mut line).unwrap();
    let line = line.trim_end_matches('\n');
    if line == EXITED {
        drop(gate);
        let out = waiter.join().unwrap();
        panic!("amplihack ended before the step ran: {}", describe(&out));
    }
    let tmpdir = PathBuf::from(line);
    let runtime = tmpdir.parent().unwrap().to_path_buf();
    assert!(
        runtime.starts_with(&fx.work) && tmpdir.ends_with("tmp"),
        "the step's TMPDIR {tmpdir:?} is the run's own under {:?}",
        fx.work
    );

    let holder = Holder::spawn(&fx, &tmpdir);
    fx.proc.link_own(holder.pid());
    gate.release();
    let out = waiter.join().unwrap();
    assert!(out.status.success(), "{}", describe(&out));

    assert!(
        tmpdir.is_dir(),
        "a runtime folder in use must be left in place: {}",
        describe(&out)
    );
    let stderr = stderr_of(&out);
    let notes: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("still in use"))
        .collect();
    assert_eq!(notes.len(), 1, "exactly one 'still in use' line: {stderr}");
    assert!(
        notes[0].contains(&runtime.display().to_string()),
        "the line names the runtime folder {runtime:?}: {}",
        notes[0]
    );

    // The run is recorded as ended, so a later sweep can judge it.
    let run_dir = fx.run_dirs().into_iter().next().expect("the run folder");
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(run_dir.join("run.json")).unwrap()).unwrap();
    assert!(
        report["ended_at"].as_str().is_some_and(|s| !s.is_empty()),
        "{report:#}"
    );

    // While the holder runs, a sweep checks again and keeps the folder.
    let busy = fx.cleanup_json(&["--apply", "--only", "run-tmp"]);
    assert!(tmpdir.is_dir(), "kept while in use: {busy:#}");
    let kept = entry_for(&busy, &runtime);
    assert_eq!(kept["action"], "skipped", "{busy:#}");
    assert_eq!(kept["reason"], "in use by a running process", "{busy:#}");

    holder.finish();

    let report = fx.cleanup_json(&["--apply", "--only", "run-tmp"]);
    assert_eq!(
        entry_for(&report, &runtime)["action"],
        "removed",
        "{report:#}"
    );
    assert!(!present(&runtime), "removed once the holder has exited");
    assert!(run_dir.join("run.json").is_file(), "run.json stays");
}

/// A run whose step leaves nothing behind still removes its runtime folder
/// at the end, and says nothing about it being in use.
#[test]
fn runtime_folder_nobody_uses_is_removed_at_run_end_without_a_note() {
    let fx = Fx::new();
    let runner = format!("#!/bin/sh\n{RECORD_ENV_SNIPPET}\n{RUNNER_SUCCESS_JSON}\n");
    let out = fx.recipe_run(&runner, |_| {});
    assert!(out.status.success(), "{}", describe(&out));
    let run_id = fx.recorded_env()["AMPLIHACK_RUN_ID"].clone();
    assert!(!expected_runtime_dir(&fx.work, &run_id).exists());
    assert!(
        !stderr_of(&out).contains("still in use"),
        "{}",
        describe(&out)
    );
}

fn with_tmpdir(c: &mut Command, tmpdir: &std::ffi::OsStr) {
    c.env("TMPDIR", tmpdir)
        .env("TMP", tmpdir)
        .env("TEMP", tmpdir);
}

/// A normal run started with a TMPDIR that no longer exists (left over from
/// an earlier run, as in #2472) gives the step its own run's TMPDIR, which
/// exists, and never re-creates the inherited one.
#[test]
fn scoped_step_with_missing_inherited_tmpdir_gets_its_own() {
    let fx = Fx::new();
    let gone = fx.root.join("tmp/r/58f4c9d67736/tmp");
    let runner = format!("#!/bin/sh\n{RECORD_ENV_SNIPPET}\n{RUNNER_SUCCESS_JSON}\n");
    let out = fx.recipe_run(&runner, |c| {
        with_tmpdir(c, gone.as_os_str());
        c.env("AMPLIHACK_CLEANUP_ON_EXIT", "0");
    });
    assert!(out.status.success(), "{}", describe(&out));

    let env = fx.recorded_env();
    let run_id = env["AMPLIHACK_RUN_ID"].clone();
    let own = expected_runtime_dir(&fx.work, &run_id).join("tmp");
    assert_eq!(env["TMPDIR"], own.display().to_string(), "{env:?}");
    assert_ne!(env["TMPDIR"], gone.display().to_string());
    assert_eq!(env["TMP"], env["TMPDIR"]);
    assert_eq!(env["TEMP"], env["TMPDIR"]);
    assert_eq!(
        env.get("TMPDIR_EXISTS").map(String::as_str),
        Some("1"),
        "the step's TMPDIR exists while it runs: {env:?}"
    );
    assert!(
        !present(&gone),
        "the inherited TMPDIR of another run is never re-created"
    );
}

fn dry_run(fx: &Fx, tmpdir: &std::ffi::OsStr) -> Output {
    let runner = format!("#!/bin/sh\n{RECORD_ENV_SNIPPET}\n{RUNNER_SUCCESS_JSON}\n");
    fx.recipe_run(&runner, |c| {
        c.arg("--dry-run");
        with_tmpdir(c, tmpdir);
    })
}

/// Control for the refusals below: a dry run with a usable TMPDIR starts
/// the step, so "the stub recorded nothing" is a real signal there.
#[test]
fn dry_run_with_existing_tmpdir_starts_the_step() {
    let fx = Fx::new();
    let out = dry_run(&fx, fx.tmp.as_os_str());
    assert!(out.status.success(), "{}", describe(&out));
    let env = fx.recorded_env();
    assert_eq!(env.get("TMPDIR_EXISTS").map(String::as_str), Some("1"));
}

/// A dry run has no run folder to fall back to, so a missing inherited
/// TMPDIR refuses the run, naming the path, before any step starts.
#[test]
fn dry_run_with_missing_inherited_tmpdir_is_refused_naming_path() {
    let fx = Fx::new();
    let gone = fx.root.join("tmp/gone");
    let out = dry_run(&fx, gone.as_os_str());
    assert!(!out.status.success(), "{}", describe(&out));
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&gone.display().to_string()),
        "names the path: {stderr}"
    );
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(stderr.contains("TMPDIR"), "{stderr}");
    assert!(
        !fx.records.join("env").exists(),
        "no step may start: {}",
        describe(&out)
    );
    assert!(!present(&gone), "the inherited TMPDIR is never created");
}

/// An inherited TMPDIR that is a file, not a folder, is refused the same way.
#[test]
fn dry_run_with_tmpdir_that_is_a_file_is_refused_naming_path() {
    let fx = Fx::new();
    let file = fx.root.join("tmp/not-a-folder");
    fs::write(&file, "x").unwrap();
    let out = dry_run(&fx, file.as_os_str());
    assert!(!out.status.success(), "{}", describe(&out));
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&file.display().to_string()),
        "names the path: {stderr}"
    );
    assert!(stderr.contains("not a directory"), "{stderr}");
    assert!(!fx.records.join("env").exists(), "no step may start");
    assert_eq!(fs::read_to_string(&file).unwrap(), "x");
}

/// A relative inherited TMPDIR is refused too, and nothing is created
/// relative to the working folder.
#[test]
fn dry_run_with_relative_tmpdir_is_refused_naming_path() {
    let fx = Fx::new();
    let out = dry_run(&fx, std::ffi::OsStr::new("relative/gone"));
    assert!(!out.status.success(), "{}", describe(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("relative/gone"), "names the path: {stderr}");
    assert!(stderr.contains("not an absolute directory"), "{stderr}");
    assert!(!fx.records.join("env").exists(), "no step may start");
    for base in [&fx.repo, &fx.root] {
        assert!(
            !present(&base.join("relative")),
            "nothing created in {base:?}"
        );
    }
}
