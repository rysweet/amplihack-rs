//! Issue #2175: unit tests of the run record.
//!
//! Included from `run_record.rs` as
//! `#[cfg(test)] #[path = "run_record_tests.rs"] mod tests;`.
//!
//! The contract these tests set:
//!
//! - `VERSION = 1`, `MAX_RECORD = 4096` (bytes), `MAX_RUN_ID = 128` (bytes).
//! - `validate_run_id(&str) -> bool`: 1 to 128 bytes of `[A-Za-z0-9._-]`,
//!   not starting with `.`.
//! - `write(run_dir, run_id, runner_pid: u32) -> Result<Guard>` writes
//!   `<run_dir>/procs/<run_id>.json` (dir 0700, file 0600):
//!   `{"version":1,"run_id","recipe_run":{"pid","start_ticks"},
//!   "runner":{"pid","pgid","start_ticks"}}`, with `recipe_run` this process,
//!   `pgid == runner pid`, start times read from `/proc`. A `procs` symlink
//!   is refused. Dropping the `Guard` removes the file.
//! - `find(work_root, run_id) -> Option<Record>` opens
//!   `<work_root>/runs/<entry>/procs/<run_id>.json` for each real directory
//!   entry, never through a symlink, at most 4096 bytes, owned by this user
//!   with no group or other write bit, no unknown fields, version 1, the same
//!   run id, both pids > 1 and `runner.pgid == runner.pid`.
//! - `stop::live_targets(&Record) -> Tree`: the recorded pids whose start
//!   time still matches, never this process or its parent.
//!
//! Temp directories only; nothing here signals a process.

use super::*;
use crate::commands::cleanup::run_dir::pid_start_ticks;
use crate::commands::recipe::stop::live_targets;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const ID: &str = "5b60657b-76ef-4f49-8a22-8b89ed75f43e";

/// A temp work root with one run directory, `runs/run-a`.
struct Root {
    _tmp: tempfile::TempDir,
    work: PathBuf,
}

impl Root {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().canonicalize().unwrap().join("work");
        let root = Self { _tmp: tmp, work };
        fs::create_dir_all(root.run_dir("run-a")).unwrap();
        root
    }

    fn run_dir(&self, name: &str) -> PathBuf {
        self.work.join("runs").join(name)
    }

    fn file(&self, run: &str, run_id: &str) -> PathBuf {
        self.run_dir(run)
            .join("procs")
            .join(format!("{run_id}.json"))
    }

    /// Writes `body` as a 0600 record of `run_id` in run directory `run`.
    fn plant(&self, run: &str, run_id: &str, body: &str) -> PathBuf {
        let path = self.file(run, run_id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    fn find(&self, run_id: &str) -> Option<Record> {
        find(&self.work, run_id)
    }
}

/// A `sleep` child of this test, killed and reaped when dropped.
struct Sleeper(Child);

impl Sleeper {
    fn start() -> Self {
        Self(
            Command::new("sleep")
                .arg("300")
                .stdin(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn pid(&self) -> i32 {
        self.0.id() as i32
    }

    fn start_ticks(&self) -> u64 {
        pid_start_ticks(self.0.id()).expect("the sleep's start time")
    }

    fn running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn me() -> u32 {
    std::process::id()
}

fn my_start() -> u64 {
    pid_start_ticks(me()).expect("this process's start time")
}

fn body(version: i64, run_id: &str, recipe_run: (i64, u64), runner: (i64, i64, u64)) -> String {
    format!(
        r#"{{"version":{version},"run_id":"{run_id}","recipe_run":{{"pid":{},"start_ticks":{}}},"runner":{{"pid":{},"pgid":{},"start_ticks":{}}}}}"#,
        recipe_run.0, recipe_run.1, runner.0, runner.1, runner.2
    )
}

/// A record that `find` accepts: two live sleeps with their real start times.
fn good(run_id: &str, a: &Sleeper, b: &Sleeper) -> String {
    body(
        1,
        run_id,
        (i64::from(a.pid()), a.start_ticks()),
        (i64::from(b.pid()), i64::from(b.pid()), b.start_ticks()),
    )
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn record(recipe_run: (i32, u64), runner: (i32, u64)) -> Record {
    Record {
        version: VERSION,
        run_id: ID.to_string(),
        recipe_run: Proc {
            pid: recipe_run.0,
            start_ticks: recipe_run.1,
        },
        runner: Runner {
            pid: runner.0,
            pgid: runner.0,
            start_ticks: runner.1,
        },
    }
}

// --- the run id ---------------------------------------------------------------

#[test]
fn a_uuid_and_the_allowed_characters_are_valid_ids() {
    assert_eq!(MAX_RUN_ID, 128);
    for id in [
        ID,
        "a",
        "A_b-9",
        "a.b",
        "no-such-run",
        &"x".repeat(MAX_RUN_ID),
    ] {
        assert!(validate_run_id(id), "{id:?} must be accepted");
    }
}

#[test]
fn ids_that_could_name_another_path_are_invalid() {
    let long = "x".repeat(MAX_RUN_ID + 1);
    for id in [
        "",
        "../x",
        "a/b",
        "/abs",
        ".",
        "..",
        ".hidden",
        "a\0b",
        "a b",
        "a\n",
        "é",
        "a\\b",
        "a\u{1b}[31m",
        &long,
    ] {
        assert!(!validate_run_id(id), "{id:?} must be refused");
    }
}

// --- write and the guard ------------------------------------------------------

#[test]
fn a_written_record_names_this_process_and_the_runner() {
    let root = Root::new();
    let runner = Sleeper::start();
    let _guard = write(&root.run_dir("run-a"), ID, runner.0.id()).unwrap();
    let found = root.find(ID).expect("the record just written");
    assert_eq!(found.version, VERSION);
    assert_eq!(VERSION, 1);
    assert_eq!(found.run_id, ID);
    assert_eq!(found.recipe_run.pid, me() as i32);
    assert_eq!(found.recipe_run.start_ticks, my_start());
    assert_eq!(found.runner.pid, runner.pid());
    assert_eq!(found.runner.pgid, runner.pid());
    assert_eq!(found.runner.start_ticks, runner.start_ticks());
}

#[test]
fn the_file_holds_exactly_the_record_fields() {
    let root = Root::new();
    let runner = Sleeper::start();
    let _guard = write(&root.run_dir("run-a"), ID, runner.0.id()).unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(root.file("run-a", ID)).unwrap()).unwrap();
    let keys =
        |v: &serde_json::Value| -> Vec<String> { v.as_object().unwrap().keys().cloned().collect() };
    assert_eq!(keys(&value), ["recipe_run", "run_id", "runner", "version"]);
    assert_eq!(keys(&value["recipe_run"]), ["pid", "start_ticks"]);
    assert_eq!(keys(&value["runner"]), ["pgid", "pid", "start_ticks"]);
}

#[test]
fn the_record_is_private() {
    let root = Root::new();
    let runner = Sleeper::start();
    let _guard = write(&root.run_dir("run-a"), ID, runner.0.id()).unwrap();
    assert_eq!(mode(&root.run_dir("run-a").join("procs")), 0o700);
    assert_eq!(mode(&root.file("run-a", ID)), 0o600);
}

#[test]
fn dropping_the_guard_removes_the_record() {
    let root = Root::new();
    let runner = Sleeper::start();
    let guard = write(&root.run_dir("run-a"), ID, runner.0.id()).unwrap();
    assert!(root.file("run-a", ID).is_file());
    drop(guard);
    assert!(!root.file("run-a", ID).exists());
    assert!(root.find(ID).is_none());
}

#[test]
fn a_guard_whose_record_is_already_gone_drops_quietly() {
    let root = Root::new();
    let runner = Sleeper::start();
    let guard = write(&root.run_dir("run-a"), ID, runner.0.id()).unwrap();
    fs::remove_file(root.file("run-a", ID)).unwrap();
    drop(guard);
}

#[test]
fn each_attempt_has_its_own_record() {
    let root = Root::new();
    let runner = Sleeper::start();
    let first = write(&root.run_dir("run-a"), ID, runner.0.id()).unwrap();
    let second = write(&root.run_dir("run-a"), "second-attempt", runner.0.id()).unwrap();
    drop(first);
    assert!(root.find(ID).is_none());
    assert!(root.find("second-attempt").is_some());
    drop(second);
}

#[test]
fn write_refuses_an_invalid_id_and_creates_nothing() {
    let root = Root::new();
    let runner = Sleeper::start();
    assert!(write(&root.run_dir("run-a"), "../x", runner.0.id()).is_err());
    assert!(!root.run_dir("run-a").join("x.json").exists());
    assert!(!root.run_dir("run-a").join("procs").join("x.json").exists());
}

#[test]
fn write_fails_for_a_runner_that_is_gone() {
    let root = Root::new();
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(write(&root.run_dir("run-a"), ID, pid).is_err());
    assert!(!root.file("run-a", ID).exists());
}

#[test]
fn write_does_not_follow_a_procs_symlink() {
    let root = Root::new();
    let elsewhere = root.work.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    symlink(&elsewhere, root.run_dir("run-a").join("procs")).unwrap();
    let runner = Sleeper::start();
    assert!(write(&root.run_dir("run-a"), ID, runner.0.id()).is_err());
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

// --- find ---------------------------------------------------------------------

#[test]
fn find_looks_in_every_run_directory() {
    let root = Root::new();
    let (a, b) = (Sleeper::start(), Sleeper::start());
    fs::create_dir_all(root.run_dir("run-b")).unwrap();
    root.plant("run-b", ID, &good(ID, &a, &b));
    let found = root.find(ID).expect("the record in run-b");
    assert_eq!(found.runner.pid, b.pid());
}

#[test]
fn find_with_no_runs_directory_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(find(tmp.path(), ID).is_none());
}

#[test]
fn find_of_a_missing_record_is_none() {
    let root = Root::new();
    assert!(root.find(ID).is_none());
}

#[test]
fn find_of_an_invalid_id_reads_nothing() {
    let root = Root::new();
    let (a, b) = (Sleeper::start(), Sleeper::start());
    // Where `runs/run-a/procs/../x.json` would lead.
    let path = root.run_dir("run-a").join("x.json");
    fs::write(&path, good("../x", &a, &b)).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::create_dir_all(root.run_dir("run-a").join("procs")).unwrap();
    assert!(root.find("../x").is_none());
}

#[test]
fn find_refuses_a_symlinked_record() {
    let root = Root::new();
    let (a, b) = (Sleeper::start(), Sleeper::start());
    let real = root.plant("run-a", "elsewhere", &good(ID, &a, &b));
    symlink(&real, root.file("run-a", ID)).unwrap();
    assert!(root.find(ID).is_none());
}

#[test]
fn find_does_not_follow_a_procs_symlink() {
    let root = Root::new();
    let (a, b) = (Sleeper::start(), Sleeper::start());
    let procs = root.work.join("outside").join("procs");
    fs::create_dir_all(&procs).unwrap();
    let path = procs.join(format!("{ID}.json"));
    fs::write(&path, good(ID, &a, &b)).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&procs, root.run_dir("run-a").join("procs")).unwrap();
    assert!(root.find(ID).is_none());
}

#[test]
fn find_does_not_follow_a_run_directory_symlink() {
    let root = Root::new();
    let (a, b) = (Sleeper::start(), Sleeper::start());
    let outside = root.work.join("outside");
    let procs = outside.join("procs");
    fs::create_dir_all(&procs).unwrap();
    let path = procs.join(format!("{ID}.json"));
    fs::write(&path, good(ID, &a, &b)).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&outside, root.run_dir("run-link")).unwrap();
    assert!(root.find(ID).is_none());
}

#[test]
fn find_refuses_a_record_others_can_write() {
    let root = Root::new();
    let (a, b) = (Sleeper::start(), Sleeper::start());
    let path = root.plant("run-a", ID, &good(ID, &a, &b));
    for bits in [0o620, 0o602, 0o666] {
        fs::set_permissions(&path, fs::Permissions::from_mode(bits)).unwrap();
        assert!(root.find(ID).is_none(), "mode {bits:o} must be refused");
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        root.find(ID).is_some(),
        "a record others can only read is fine"
    );
}

#[test]
fn find_refuses_a_record_over_4096_bytes() {
    let root = Root::new();
    let (a, b) = (Sleeper::start(), Sleeper::start());
    let mut text = good(ID, &a, &b);
    let limit = usize::try_from(MAX_RECORD).unwrap();
    assert_eq!(limit, 4096);
    text.push_str(&" ".repeat(limit + 1 - text.len()));
    root.plant("run-a", ID, &text);
    assert!(root.find(ID).is_none());

    text.pop();
    root.plant("run-a", ID, &text);
    assert!(root.find(ID).is_some(), "exactly 4096 bytes is read");
}

#[test]
fn find_refuses_malformed_or_dangerous_records() {
    let root = Root::new();
    let (a, b) = (Sleeper::start(), Sleeper::start());
    let (ap, at) = (i64::from(a.pid()), a.start_ticks());
    let (bp, bt) = (i64::from(b.pid()), b.start_ticks());
    let cases: Vec<(&str, String)> = vec![
        ("not json", "not json".into()),
        ("empty", String::new()),
        (
            "unknown top-level field",
            good(ID, &a, &b).replacen('{', r#"{"cwd":"/","#, 1),
        ),
        (
            "unknown runner field",
            body(1, ID, (ap, at), (bp, bp, bt)).replacen(
                r#""runner":{"#,
                r#""runner":{"argv":[],"#,
                1,
            ),
        ),
        ("version 2", body(2, ID, (ap, at), (bp, bp, bt))),
        ("version 0", body(0, ID, (ap, at), (bp, bp, bt))),
        (
            "another run id",
            body(1, "another-run", (ap, at), (bp, bp, bt)),
        ),
        (
            "runner pgid differs",
            body(1, ID, (ap, at), (bp, bp + 1, bt)),
        ),
        ("runner pid -1", body(1, ID, (ap, at), (-1, -1, bt))),
        ("runner pid 0", body(1, ID, (ap, at), (0, 0, bt))),
        ("runner pid 1", body(1, ID, (ap, at), (1, 1, bt))),
        ("recipe run pid -1", body(1, ID, (-1, at), (bp, bp, bt))),
        ("recipe run pid 0", body(1, ID, (0, at), (bp, bp, bt))),
        ("recipe run pid 1", body(1, ID, (1, at), (bp, bp, bt))),
        (
            "pid out of range",
            body(1, ID, (ap, at), (1 << 40, 1 << 40, bt)),
        ),
        (
            "no runner",
            format!(
                r#"{{"version":1,"run_id":"{ID}","recipe_run":{{"pid":{ap},"start_ticks":{at}}}}}"#
            ),
        ),
    ];
    for (what, text) in cases {
        root.plant("run-a", ID, &text);
        assert!(root.find(ID).is_none(), "{what}: {text}");
    }
}

#[test]
fn find_changes_nothing_on_disk() {
    let root = Root::new();
    let (a, b) = (Sleeper::start(), Sleeper::start());
    let text = good("another-run", &a, &b);
    let path = root.plant("run-a", ID, &text);
    assert!(root.find(ID).is_none());
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

// --- the start-time check (D11) -----------------------------------------------

#[test]
fn a_pid_whose_start_time_differs_is_not_a_target() {
    let mut a = Sleeper::start();
    let mut b = Sleeper::start();
    let stale = record(
        (a.pid(), a.start_ticks() + 1),
        (b.pid(), b.start_ticks() + 1),
    );
    assert!(live_targets(&stale).is_empty());
    assert!(a.running() && b.running());

    let live = record((a.pid(), a.start_ticks()), (b.pid(), b.start_ticks()));
    let targets = live_targets(&live);
    assert_eq!(targets.get(&a.pid()), Some(&a.start_ticks()));
    assert_eq!(targets.get(&b.pid()), Some(&b.start_ticks()));
    assert_eq!(targets.len(), 2);
    assert!(
        a.running() && b.running(),
        "building targets signals nothing"
    );
}

#[test]
fn only_the_matching_pid_of_a_half_stale_record_is_a_target() {
    let a = Sleeper::start();
    let b = Sleeper::start();
    let half = record((a.pid(), a.start_ticks() + 1), (b.pid(), b.start_ticks()));
    let targets = live_targets(&half);
    assert_eq!(targets.keys().copied().collect::<Vec<_>>(), [b.pid()]);
}

#[test]
fn a_process_that_exited_is_not_a_target() {
    let mut gone = Command::new("true").spawn().unwrap();
    let pid = gone.id() as i32;
    gone.wait().unwrap();
    let b = Sleeper::start();
    let r = record((pid, 1), (b.pid(), b.start_ticks()));
    assert_eq!(
        live_targets(&r).keys().copied().collect::<Vec<_>>(),
        [b.pid()]
    );
}

#[test]
fn this_process_and_its_parent_are_never_targets() {
    let me = me() as i32;
    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() };
    let parent_start = pid_start_ticks(parent as u32).expect("parent start time");
    let r = record((me, my_start()), (parent, parent_start));
    assert!(live_targets(&r).is_empty());
}

#[test]
fn a_dry_run_workspace_has_no_run_directory() {
    use crate::commands::cleanup::run_dir::RunWorkspace;
    let workspace = RunWorkspace::dry_runtime().unwrap();
    assert!(workspace.run_dir().is_none());
}
