// crates/amplihack-cli/src/commands/recipe/run/tests_teardown.rs
//
// Issue #964 — TDD (write-tests-first) contract for DETERMINISTIC TEARDOWN of
// the recipe-runner subprocess tree.
//
// Observed bug: a smart-orchestrator (recipe-runner) failure LEAKS recursive
// descendant subprocesses. Today `terminate_recipe_runner` only runs on the
// TIMEOUT path and sends an immediate `SIGKILL` to the process group. That
// means:
//   1. On any NON-timeout failure / early-exit the descendant tree is never
//      reaped — orphaned children keep running and amplify host load.
//   2. Even on the timeout path children get no chance to shut down cleanly
//      (no `SIGTERM` grace window), so any child-side cleanup is skipped.
//   3. There is no operator-tunable grace window.
//
// The tests below specify the intended behaviour. They exercise the public
// entry point `execute::execute_recipe_via_rust` with a stub runner installed
// via `RECIPE_RUNNER_RS_PATH`, so they COMPILE against today's code and FAIL at
// runtime (true TDD red). They must PASS once deterministic teardown lands:
//
//   * teardown fires on EVERY failure / early-exit path (not only timeout),
//   * teardown is a three-phase reaper: SIGTERM -> configurable grace -> SIGKILL,
//   * the grace window is configurable via `AMPLIHACK_TEARDOWN_GRACE_SECS`,
//   * teardown is scoped to the runner's own process group and never signals
//     unrelated / parent processes.
//
// Unix-only: the leak and the process-group reaping are Unix semantics.

#![cfg(unix)]

use super::*;

mod execute {
    pub(super) use super::super::execute::{
        new_tree_id, resolve_claimed_depth, terminate_recipe_runner_with,
    };
    pub(super) use super::execute_recipe_via_rust_for_test as execute_recipe_via_rust;
}
use crate::test_support::EnvGuard;
use std::collections::BTreeMap;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

/// Environment-variable name the deterministic-teardown feature must honour to
/// configure the SIGTERM -> SIGKILL grace window (in whole seconds).
const TEARDOWN_GRACE_ENV: &str = "AMPLIHACK_TEARDOWN_GRACE_SECS";
const RUNNER_TIMEOUT_ENV: &str = "AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS";
const RUNNER_PATH_ENV: &str = "RECIPE_RUNNER_RS_PATH";
/// Issue #1326: the session-tree store is durable now ($HOME/.amplihack). Tests
/// must redirect it or they write into the developer's real home directory.
const SESSION_TREE_DIR_ENV: &str = "AMPLIHACK_SESSION_TREE_DIR";

/// Write an executable `/bin/sh` stub at `path` (mode 0755) through
/// `write_executable`, which never holds a write descriptor on it in this
/// process, so a sibling test's fork cannot make its exec fail with ETXTBSY.
fn write_stub(path: &Path, body: &str) {
    crate::test_support::write_executable(path, body);
}

/// Makes a FIFO at `path`.
fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("failed to run mkfifo");
    assert!(status.success(), "mkfifo {} failed", path.display());
}

/// Owns both FIFO participants and joins the executor before the caller's
/// environment guards or temporary directory can drop. Constructed before
/// readiness checks, including pidfd setup, can fail.
#[cfg(target_os = "linux")]
struct OrphanExecution {
    // Drop joins the worker while the `go` descriptors below are still open.
    worker: crate::test_support::unix_process::FifoWorker<bool>,
    go_writer: std::fs::File,
    _go_reader: std::fs::File,
}

#[cfg(target_os = "linux")]
impl OrphanExecution {
    fn release_runner(&mut self) {
        use std::io::Write;
        self.go_writer.write_all(b"run\n").expect("release runner");
    }
}

#[cfg(target_os = "linux")]
impl Drop for OrphanExecution {
    fn drop(&mut self) {
        use std::io::Write;
        // A blank release takes the shell's cleanup path. The worker guard
        // then releases `hold` and joins; the shell waits for its child first.
        // These writes also work if neither shell has reached its read yet.
        if let Err(e) = self.go_writer.write_all(b"\n") {
            eprintln!("releasing the orphan probe runner failed: {e}");
        }
    }
}

/// A runner that starts an orphan in its own process group and then exits 1
/// once the test writes `run` to `go`. The orphan writes its pid to `pid`
/// and blocks in a shell builtin read on `hold` (no external reader process).
/// Until the execution guard drops, no release is written to `hold`, so the
/// orphan ends only by a signal; it writes `marker` only during cleanup.
/// Its lifetime is observed through a Linux pidfd; portable signal and grace
/// coverage lives in the tests below without this probe.
#[cfg(target_os = "linux")]
struct OrphanProbe {
    runner: PathBuf,
    recipe: PathBuf,
    pid: PathBuf,
    hold: PathBuf,
    go: PathBuf,
    marker: PathBuf,
}

#[cfg(target_os = "linux")]
impl OrphanProbe {
    fn new(dir: &Path) -> Self {
        let probe = Self {
            runner: dir.join("recipe-runner-rs"),
            recipe: dir.join("recipe.yaml"),
            pid: dir.join("orphan.pid"),
            hold: dir.join("hold"),
            go: dir.join("go"),
            marker: dir.join("orphan-marker"),
        };
        mkfifo(&probe.hold);
        mkfifo(&probe.go);
        write_stub(
            &probe.runner,
            &format!(
                "#!/bin/sh\ncase \"$1\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\n
                 echo $$ > '{}'
                 /bin/sh -c 'echo $$ > \"$1\"; read _hold < \"$2\"; echo leaked > \"$3\"'                  orphan '{}' '{}' '{}' &
                 read _go < '{}'
                 if [ \"$_go\" != run ]; then wait; fi
                 exit 1
",
                dir.join("runner.pid").display(),
                probe.pid.display(),
                probe.hold.display(),
                probe.marker.display(),
                probe.go.display(),
            ),
        );
        std::fs::write(&probe.recipe, "name: orphan-probe\nsteps: []\n")
            .expect("failed to write recipe");
        probe
    }

    fn execute(&self, working_dir: &Path) -> OrphanExecution {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        // Establish both ends before starting the executor: its runner may
        // not have spawned yet when a failed readiness check unwinds.
        let go_reader = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.go)
            .expect("open go reader");
        let go_writer = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.go)
            .expect("open go writer");
        let (recipe, working_dir) = (self.recipe.clone(), working_dir.to_path_buf());
        let worker =
            crate::test_support::unix_process::FifoWorker::spawn(&self.hold, 1, move || {
                execute::execute_recipe_via_rust(
                    &recipe,
                    &BTreeMap::new(),
                    false,
                    true,
                    &working_dir,
                    &[],
                    None,
                )
                .is_err()
            });
        OrphanExecution {
            worker,
            go_writer,
            _go_reader: go_reader,
        }
    }

    /// Opens a pidfd for the orphan while it is known to be alive: the
    /// runner is still blocked on `go`, so no teardown has run yet.
    fn orphan(&self) -> crate::test_support::pidfd_wait::PidFd {
        self.orphan_with_ceiling(crate::test_support::wait::CEILING)
    }

    fn orphan_with_ceiling(
        &self,
        ceiling: std::time::Duration,
    ) -> crate::test_support::pidfd_wait::PidFd {
        use crate::test_support::pidfd_wait::{PidFd, parse_pid};
        crate::test_support::wait::wait_until_with_ceiling(
            "the orphan to write its pid",
            ceiling,
            || {
                std::fs::read_to_string(&self.pid)
                    .ok()
                    .is_some_and(|raw| parse_pid(&raw).is_ok())
            },
        );
        let raw = std::fs::read_to_string(&self.pid).expect("orphan pid file");
        let pid = parse_pid(&raw).expect("orphan pid");
        PidFd::open(pid).expect("pidfd of the live orphan")
    }
}

/// Exercise a real readiness timeout before release_runner(), and also drop
/// immediately after spawning (before either participant need be ready).
/// Cleanup must release both shells without help from product termination.
#[cfg(target_os = "linux")]
#[test]
fn test_orphan_probe_readiness_failure_cleans_up_runner_and_worker() {
    use crate::test_support::pidfd_wait::{PidFd, parse_pid, wait_pid_gone};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::time::Duration;

    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();
    for wait_for_readiness in [true, false] {
        let temp = tempfile::tempdir().expect("create probe directory");
        let mut probe = OrphanProbe::new(temp.path());
        let mut env = EnvGuard::capture(&[]);
        env.assign(RUNNER_PATH_ENV, &probe.runner);
        env.assign(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
        env.clear(RUNNER_TIMEOUT_ENV);
        let mut observed = None;
        let failure = catch_unwind(AssertUnwindSafe(|| {
            let _execution = probe.execute(temp.path());
            if wait_for_readiness {
                // Retain observations only; cleanup must work without a
                // KillOnDrop and before readiness gives the caller a pidfd.
                let orphan = probe.orphan();
                let raw = std::fs::read_to_string(temp.path().join("runner.pid")).unwrap();
                let runner = PidFd::open(parse_pid(&raw).unwrap()).unwrap();
                observed = Some((runner, orphan));
            }
            // This nonexistent path forces the actual readiness wait to fail
            // deterministically, even if the shells have already started.
            probe.pid = temp.path().join("missing.pid");
            probe.orphan_with_ceiling(Duration::ZERO);
        }));
        let message = failure.expect_err("readiness must fail");
        let message = message.downcast_ref::<String>().expect("timeout message");
        assert!(
            message.contains("the orphan to write its pid: timed out"),
            "{message}"
        );
        assert!(
            probe.marker.exists(),
            "unwinding must release the orphan and join the executor before returning"
        );
        if let Some((runner, orphan)) = observed {
            wait_pid_gone(&runner, "readiness cleanup to reap the runner");
            wait_pid_gone(&orphan, "readiness cleanup to release the orphan");
        }
        // Environment guards and tempdir remain alive until after cleanup.
        assert_eq!(
            std::env::var_os(RUNNER_PATH_ENV).as_deref(),
            Some(probe.runner.as_os_str())
        );
    }
}

// -------------------------------------------------------------------------
// Test 1 — teardown fires on the FAILURE (non-timeout) path.
//
// A runner that spawns a detached descendant and then exits NON-ZERO must have
// that descendant reaped. The descendant writes a marker after a short delay;
// if teardown works the marker never appears.
// -------------------------------------------------------------------------
#[cfg(target_os = "linux")]
#[test]
fn test_failure_path_reaps_orphaned_descendants() {
    use crate::test_support::pidfd_wait::{KillOnDrop, wait_pid_gone};
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    // The runner starts an orphan and then FAILS (exit 1), not a timeout.
    let probe = OrphanProbe::new(temp.path());

    let mut env = EnvGuard::capture(&[]);
    env.assign(RUNNER_PATH_ENV, &probe.runner);
    env.assign(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    // Ensure a fast failure is not misread as a timeout.
    env.clear(RUNNER_TIMEOUT_ENV);

    let mut execution = probe.execute(temp.path());
    let orphan = KillOnDrop(probe.orphan());
    execution.release_runner();
    let failed = execution.worker.wait();
    assert!(failed, "non-zero runner exit must surface as an error");

    // The orphan blocks until signalled, so it ends only if the teardown
    // reaped it (issue #964); a surviving orphan fails here as hung.
    wait_pid_gone(
        &orphan.0,
        "failure-path teardown to reap the orphaned recipe-runner descendant",
    );
    assert!(
        !probe.marker.exists(),
        "failure-path teardown must reap orphaned recipe-runner descendants \
         (issue #964): descendant survived and wrote {}",
        probe.marker.display()
    );
}

// -------------------------------------------------------------------------
// Test 2 — teardown is GRACEFUL: SIGTERM is delivered before SIGKILL.
//
// The runner traps SIGTERM, records that it received it, and exits cleanly.
// The reaper sends SIGTERM first, so the trap runs and writes the marker. A
// teardown that sent SIGKILL first would leave no marker.
//
// Uses the timeout path so the ONLY thing under test is SIGTERM-before-SIGKILL,
// isolating it from Test 1.
// -------------------------------------------------------------------------
#[test]
fn test_timeout_teardown_sends_sigterm_before_sigkill() {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let marker = temp.path().join("graceful-marker");
    let ready = temp.path().join("trap-ready");
    let completed = temp.path().join("trap-completed");
    mkfifo(&completed);
    let mut completion = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&completed)
        .expect("open TERM completion event reader");

    // The runner blocks on a builtin FIFO read. The supervisor releases it
    // only during drop, so passing still requires the timeout teardown.
    let hold = temp.path().join("hold");
    mkfifo(&hold);
    write_stub(
        &runner,
        &format!(
            "#!/bin/sh\ncase \"$1\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\ntrap 'echo graceful > \"{}\"; printf x > \"{}\"; exit 0' TERM\necho ready > \"{}\"\nread _hold < \"{}\"\n",
            marker.display(),
            completed.display(),
            ready.display(),
            hold.display()
        ),
    );

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: graceful-probe\nsteps: []\n").expect("failed to write recipe");

    let mut env = EnvGuard::capture(&[]);
    env.assign(RUNNER_PATH_ENV, &runner);
    env.assign(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    env.assign(RUNNER_TIMEOUT_ENV, "1");
    // Grace stays frozen in virtual time until the child exits. Scheduling
    // delays cannot exhaust it; grace expiration is covered by the policy
    // test below, while CEILING only fails a missing completion event.
    env.assign(TEARDOWN_GRACE_ENV, "1");

    let dir = temp.path().to_path_buf();
    let mut worker = super::test_runner::RunnerWorker::spawn(Some((&hold, 1)), true, move || {
        execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), false, true, &dir, &[], None)
            .is_err()
    });
    worker.time_out_after_ready("the runner to install its TERM trap", || ready.exists());
    crate::test_support::wait::wait_until("the TERM handler completion event", || {
        let event = completion.read(&mut [0; 1]);
        match event {
            Ok(1) => true,
            Ok(0) => false,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                false
            }
            other => panic!("read TERM handler completion event: {other:?}"),
        }
    });
    assert!(worker.wait(), "hung recipe-runner must time out");

    // The teardown reaps the runner before it returns, and the runner writes
    // the marker in its SIGTERM trap before it exits: no wait is needed.
    assert!(
        marker.exists(),
        "graceful teardown must deliver SIGTERM before SIGKILL so the runner \
         can shut down cleanly (issue #964): no SIGTERM handler ran"
    );
    let contents = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        contents.contains("graceful"),
        "SIGTERM handler must have executed; marker contents: {contents:?}"
    );
}

// -------------------------------------------------------------------------
// Test 3 — configurable grace, with identical cleanup under both settings.
//
// The first two cases use identical one-second cleanup after TERM; only the
// configured grace changes. The last case checks escalation when cleanup takes
// longer than the grace. This runs on every Unix target; only the pidfd-based
// orphan probes require Linux.
// -------------------------------------------------------------------------
#[test]
fn test_teardown_grace_window_is_configurable() {
    use crate::test_support::virtual_clock::VirtualClock;
    use std::time::Duration;

    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut env = EnvGuard::capture(&[]);
    for (setting, cleanup_duration, expected_elapsed, cleaned) in [
        ("0", 1, 0, false),
        ("4", 1, 1, true),
        // An uncooperative process must receive KILL at the configured limit.
        ("4", 10, 4, false),
    ] {
        env.assign(TEARDOWN_GRACE_ENV, setting);
        let clock = VirtualClock::new();
        let events = std::cell::RefCell::new(Vec::new());
        let terminated = std::cell::Cell::new(false);
        let cleanup_done = std::cell::Cell::new(false);
        execute::terminate_recipe_runner_with(
            &clock,
            || {
                assert!(terminated.get(), "TERM must precede the grace wait");
                if clock.elapsed() >= Duration::from_secs(cleanup_duration) {
                    cleanup_done.set(true);
                    events.borrow_mut().push("cleanup");
                    false
                } else {
                    true
                }
            },
            |signal| match signal {
                libc::SIGTERM => {
                    terminated.set(true);
                    events.borrow_mut().push("TERM");
                }
                libc::SIGKILL => events.borrow_mut().push("KILL"),
                other => panic!("unexpected signal {other}"),
            },
        );
        assert_eq!(cleanup_done.get(), cleaned, "configured grace {setting}");
        assert_eq!(
            clock.elapsed(),
            Duration::from_secs(expected_elapsed),
            "virtual elapsed time for configured grace {setting}"
        );
        assert_eq!(
            *events.borrow(),
            if cleaned {
                vec!["TERM", "cleanup", "KILL"]
            } else {
                vec!["TERM", "KILL"]
            }
        );
    }
}

// -------------------------------------------------------------------------
// Test 4 — teardown is SCOPED to the runner's process group only.
//
// The reaper signals the runner's process group (the runner is a session
// leader via setsid). It must NEVER broadcast to unrelated / parent processes.
// A sibling process spawned directly by the test lives in the TEST's process
// group, so it must survive while the runner's own orphan is reaped. The
// test also guards against a future over-broad kill.
// -------------------------------------------------------------------------
#[cfg(target_os = "linux")]
#[test]
fn test_teardown_is_scoped_to_runner_process_group() {
    use crate::test_support::pidfd_wait::{KillOnDrop, wait_pid_gone};
    use std::os::unix::process::ExitStatusExt;
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();
    let temp = tempfile::tempdir().expect("failed to create temp dir");

    // Sibling process in the TEST's process group (NOT the runner's session).
    // It blocks on its stdin, so it exits only when the test closes it or a
    // signal reaches it; its exit status tells the two apart.
    let mut sibling = std::process::Command::new("/bin/cat")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn sibling process");
    struct Reap<'a>(&'a mut std::process::Child);
    impl Drop for Reap<'_> {
        fn drop(&mut self) {
            drop(self.0.stdin.take());
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let sibling = Reap(&mut sibling);

    let probe = OrphanProbe::new(temp.path());
    let mut env = EnvGuard::capture(&[]);
    env.assign(RUNNER_PATH_ENV, &probe.runner);
    env.assign(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    env.clear(RUNNER_TIMEOUT_ENV);

    let mut execution = probe.execute(temp.path());
    let orphan = KillOnDrop(probe.orphan());
    execution.release_runner();
    let failed = execution.worker.wait();
    assert!(failed, "non-zero runner exit must surface as an error");

    // Runner's own orphan must be reaped.
    wait_pid_gone(
        &orphan.0,
        "teardown to reap the runner's orphaned descendant (issue #964)",
    );
    assert!(
        !probe.marker.exists(),
        "teardown must reap the runner's orphaned descendant (issue #964)"
    );

    // The unrelated sibling (outside the runner's process group) must
    // survive the teardown, proving the reaper is pgid-scoped and not a
    // broadcast: closing its stdin ends it with status 0, where any signal
    // the teardown sent would show as a signal death.
    drop(sibling.0.stdin.take());
    let status = crate::test_support::wait::wait_child("the sibling cat", sibling.0);
    assert!(
        status.success() && status.signal().is_none(),
        "teardown must be scoped to the runner's process group and must NOT \
         signal unrelated processes (issue #964): sibling ended with {status:?}"
    );
}

// =========================================================================
// Issue #964 — TDD (write-tests-first) contract for the FAIL-CLOSED RECURSION
// DEPTH GUARD and the CALLER GIT-STATE RESTORE on terminal failure.
//
// These specify the two confirmed *missing* behaviours (only env propagation
// and happy-path teardown exist today):
//
//   A. A recursion-depth guard that BAILS BEFORE SPAWN when the current session
//      depth has reached the (clamped) maximum — so a failing orchestration can
//      never recursively re-spawn descendants past the limit. Contract:
//        * bail when AMPLIHACK_SESSION_DEPTH >= AMPLIHACK_MAX_DEPTH,
//        * FAIL-CLOSED: a malformed / non-numeric AMPLIHACK_SESSION_DEPTH is
//          treated as "at the limit" (bail), never as depth 0,
//        * a forged, over-large AMPLIHACK_MAX_DEPTH is clamped to the hard
//          ceiling (MAX_DEPTH_CEILING = 32) before the comparison,
//        * below the limit the runner still spawns normally (no over-blocking).
//
//   B. Best-effort restoration of the CALLER checkout's git configuration on
//      any terminal failure: if the runner corrupts the caller's checkout
//      (e.g. flips `core.bare=true`, reproducing the observed `git status`
//      breakage), the pre-run value is snapshotted before spawn and restored
//      after teardown, while durable child worktrees are preserved.
//
// All tests drive the PUBLIC entry point `execute::execute_recipe_via_rust`
// with a stub runner installed via `RECIPE_RUNNER_RS_PATH`, so they COMPILE
// against today's code and FAIL at runtime (true TDD red). They PASS once the
// depth guard and git-restore logic land.
//
// Unix-only: matches the rest of this teardown suite.

/// Env var carrying the current session's recursion depth (root = 0).
const SESSION_DEPTH_ENV: &str = "AMPLIHACK_SESSION_DEPTH";
/// Env var carrying the maximum allowed recursion depth.
const MAX_DEPTH_ENV: &str = "AMPLIHACK_MAX_DEPTH";

/// Write a stub that records it was spawned (marker file) and then exits with
/// `exit_code`. Used to detect whether the depth guard blocked the spawn.
fn write_marker_stub(path: &Path, marker: &Path, exit_code: u8) {
    write_stub(
        path,
        &format!(
            "#!/bin/sh\ncase \"$1\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\necho spawned > '{}'\nexit {}\n",
            marker.display(),
            exit_code
        ),
    );
}

/// Drive `execute_recipe_via_rust` once with the given depth env values and a
/// spawn-marker stub. Returns whether the stub actually ran (marker present).
/// The overall result is intentionally ignored: whether the call errors because
/// the guard bailed OR because the stub produced no JSON is irrelevant — the
/// only observable under test is "did we spawn the runner at all?".
fn depth_guard_spawned(session_depth: &str, max_depth: Option<&str>) -> bool {
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let marker = temp.path().join("spawn-marker");
    write_marker_stub(&runner, &marker, 1);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: depth-probe\nsteps: []\n").expect("failed to write recipe");

    let mut env = EnvGuard::capture(&[]);
    env.assign(RUNNER_PATH_ENV, &runner);
    env.assign(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    env.clear(RUNNER_TIMEOUT_ENV);
    // Issue #1326: a genuine nested run names a tree that has sealed a ceiling.
    // Pin both so the probe does not depend on the ambient process ancestry of
    // whoever runs the suite (a developer inside an `amplihack` session has an
    // orchestrator ancestor; CI does not).
    env.assign("AMPLIHACK_TREE_ID", "probe");
    crate::commands::session_tree::state::ensure_sealed(
        "probe",
        max_depth.and_then(|v| v.parse().ok()).unwrap_or(3),
    )
    .expect("seal probe tree");
    env.assign(SESSION_DEPTH_ENV, session_depth);
    match max_depth {
        Some(v) => env.assign(MAX_DEPTH_ENV, v),
        None => env.clear(MAX_DEPTH_ENV),
    }

    let _ = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        true,
        temp.path(),
        &[],
        None,
    );

    marker.exists()
}

// -------------------------------------------------------------------------
// Test 5 — depth AT the limit BAILS BEFORE SPAWN.
//
// SESSION_DEPTH == MAX_DEPTH must be refused: no descendant runner is spawned.
// -------------------------------------------------------------------------
#[test]
fn test_depth_guard_bails_at_limit_before_spawn() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();

    let spawned = depth_guard_spawned("3", Some("3"));
    assert!(
        !spawned,
        "recursion-depth guard must bail BEFORE spawning when session depth has \
         reached the maximum (issue #964): runner was spawned at depth == max"
    );
}

// -------------------------------------------------------------------------
// Test 6 — depth guard is FAIL-CLOSED on a malformed session depth.
//
// A non-numeric AMPLIHACK_SESSION_DEPTH must be treated as "at the limit" and
// BAIL — never silently parsed as depth 0 (which would defeat the guard).
// -------------------------------------------------------------------------
#[test]
fn test_depth_guard_fails_closed_on_malformed_session_depth() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();

    let spawned = depth_guard_spawned("not-a-number", Some("3"));
    assert!(
        !spawned,
        "recursion-depth guard must FAIL-CLOSED on a malformed \
         AMPLIHACK_SESSION_DEPTH and bail, not default to depth 0 (issue #964)"
    );
}

// -------------------------------------------------------------------------
// Test 7 — a forged over-large MAX_DEPTH is CLAMPED to the hard ceiling.
//
// AMPLIHACK_MAX_DEPTH is attacker-influenceable; a huge value must be clamped
// to MAX_DEPTH_CEILING (32) before comparison. At depth == ceiling the guard
// must still bail even though the raw MAX_DEPTH is astronomically larger.
// -------------------------------------------------------------------------
#[test]
fn test_depth_guard_clamps_forged_max_depth_to_ceiling() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();

    let ceiling = crate::commands::session_tree::state::MAX_DEPTH_CEILING;
    let at_ceiling = ceiling.to_string();
    let spawned = depth_guard_spawned(&at_ceiling, Some("4294967295"));
    assert!(
        !spawned,
        "recursion-depth guard must clamp a forged AMPLIHACK_MAX_DEPTH to the \
         hard ceiling ({ceiling}) and bail at depth == ceiling (issue #964)"
    );
}

// -------------------------------------------------------------------------
// Test 8 — below the limit the guard does NOT over-block.
//
// A legitimate nested run (depth 0, max 3) must still spawn the runner. This
// guards against a regression where the depth guard is too aggressive.
//
// Passes today AND after the fix; it is a safety net, not a red test.
// -------------------------------------------------------------------------
#[test]
fn test_depth_guard_allows_spawn_below_limit() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();

    let spawned = depth_guard_spawned("0", Some("3"));
    assert!(
        spawned,
        "depth guard must NOT block a legitimate below-limit run (depth 0 < \
         max 3): the runner should have spawned (issue #964)"
    );
}

// -------------------------------------------------------------------------
// Git-state restore helpers.
// -------------------------------------------------------------------------

/// Run `git <args...>` inside `repo` and return trimmed stdout, or `None` if
/// git is unavailable or the command failed.
fn git_in(repo: &Path, args: &[&str]) -> Option<String> {
    let output = amplihack_git::command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Initialise a minimal, non-bare git repo at `repo`. Returns false (skip) if
/// git is not usable in this environment.
fn init_git_repo(repo: &Path) -> bool {
    if amplihack_git::command()
        .arg("-C")
        .arg(repo)
        .arg("init")
        .arg("--quiet")
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
    {
        return false;
    }
    // Deterministic identity so any future commit-based assertions are stable.
    let _ = git_in(repo, &["config", "user.email", "t@example.com"]);
    let _ = git_in(repo, &["config", "user.name", "Teardown Test"]);
    // Confirm we start from the healthy, non-bare state.
    matches!(
        git_in(repo, &["config", "--get", "core.bare"]).as_deref(),
        Some("false") | None
    )
}

// -------------------------------------------------------------------------
// Test 9 — caller git state is RESTORED after a terminal failure.
//
// Reproduces the observed #964 corruption: the runner flips the caller
// checkout's `core.bare` to `true` (which makes `git status` fail with "this
// operation must be run in a work tree") and then exits non-zero. After the
// failed run the caller's checkout must be usable again: `core.bare` restored
// and `git status` succeeds.
// -------------------------------------------------------------------------
#[test]
fn test_caller_git_state_restored_after_terminal_failure() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();

    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let repo = temp.path().join("caller-checkout");
    std::fs::create_dir_all(&repo).expect("failed to create caller checkout dir");
    if !init_git_repo(&repo) {
        eprintln!("skipping: git unavailable for caller-git-state restore test");
        return;
    }
    // `git status` must work before the run.
    assert!(
        git_in(&repo, &["status", "--porcelain"]).is_some(),
        "precondition: caller checkout must be a healthy work tree before the run"
    );

    let runner = temp.path().join("recipe-runner-rs");
    // The runner corrupts the CALLER checkout (passed as `-C` == working dir)
    // exactly like the observed leak, then fails. `$5` is the `-C` value in the
    // fixed argv layout: <recipe> --output-format json -C <working_dir> ...
    write_stub(
        &runner,
        "#!/bin/sh\ncase \"$1\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\ngit -C \"$5\" config core.bare true\nexit 1\n",
    );

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: git-corrupt-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let mut env = EnvGuard::capture(&[]);
    env.assign(RUNNER_PATH_ENV, &runner);
    env.assign(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    env.clear(RUNNER_TIMEOUT_ENV);

    let result =
        execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), false, true, &repo, &[], None);
    result.expect_err("non-zero runner exit must surface as an error");

    // The caller checkout must be restored to a usable state.
    let bare_after = git_in(&repo, &["config", "--get", "core.bare"]);
    assert!(
        matches!(bare_after.as_deref(), Some("false") | None),
        "terminal-failure cleanup must restore the caller checkout's core.bare \
         (issue #964): core.bare is still {bare_after:?} after the failed run"
    );
    assert!(
        git_in(&repo, &["status", "--porcelain"]).is_some(),
        "terminal-failure cleanup must leave the caller checkout usable \
         (`git status` must succeed) after the failed run (issue #964)"
    );
}

// -------------------------------------------------------------------------
// Test 10 — restore preserves DURABLE CHILD WORKTREES.
//
// The restore must be scoped to the caller checkout's own config and must never
// delete or unregister durable child worktrees produced by the run. A child
// worktree directory created under the run must still exist after the failed
// run, even as the caller's `core.bare` is restored.
//
// Passes today AND after the fix (restore never deletes) — a guard against an
// over-broad restore that clobbers child artifacts.
// -------------------------------------------------------------------------
#[test]
fn test_restore_preserves_durable_child_worktree() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();

    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let repo = temp.path().join("caller-checkout");
    std::fs::create_dir_all(&repo).expect("failed to create caller checkout dir");
    if !init_git_repo(&repo) {
        eprintln!("skipping: git unavailable for child-worktree preservation test");
        return;
    }

    let child_marker = repo.join("worktrees").join("child").join("KEEP");
    let runner = temp.path().join("recipe-runner-rs");
    // Create a durable child artifact, corrupt the caller, then fail.
    write_stub(
        &runner,
        &format!(
            "#!/bin/sh\ncase \"$1\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\nmkdir -p '{}'\necho durable > '{}'\ngit -C \"$5\" config core.bare true\nexit 1\n",
            child_marker.parent().unwrap().display(),
            child_marker.display()
        ),
    );

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: child-worktree-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let mut env = EnvGuard::capture(&[]);
    env.assign(RUNNER_PATH_ENV, &runner);
    env.assign(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    env.clear(RUNNER_TIMEOUT_ENV);

    let result =
        execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), false, true, &repo, &[], None);
    result.expect_err("non-zero runner exit must surface as an error");

    assert!(
        child_marker.exists(),
        "cleanup must PRESERVE durable child worktrees and never delete run \
         artifacts (issue #964): {} was removed",
        child_marker.display()
    );
}

// -------------------------------------------------------------------------
// Test 11 — caller git state is RESTORED on a STRUCTURED failure result.
//
// The more realistic corruption path: the runner does real work, corrupts the
// caller checkout's `core.bare`, then reports failure by emitting a structured
// JSON result with `"success": false` (rather than crashing with empty stdout).
// That surfaces as `Ok(RecipeRunResult { success: false, .. })`, NOT `Err`, yet
// it is still a terminal failure per issue #964 and must trigger the same
// caller-checkout restore.
//
// RED before the fix: restore fired only on `result.is_err()`, so an
// `Ok(success:false)` failure left `core.bare=true` and `git status` broken.
// -------------------------------------------------------------------------
#[test]
fn test_caller_git_state_restored_after_structured_failure_result() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_environment = fixture_environment();

    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let repo = temp.path().join("caller-checkout");
    std::fs::create_dir_all(&repo).expect("failed to create caller checkout dir");
    if !init_git_repo(&repo) {
        eprintln!("skipping: git unavailable for structured-failure restore test");
        return;
    }
    assert!(
        git_in(&repo, &["status", "--porcelain"]).is_some(),
        "precondition: caller checkout must be a healthy work tree before the run"
    );

    let runner = temp.path().join("recipe-runner-rs");
    // Corrupt the caller checkout, then report failure via STRUCTURED JSON on
    // stdout and exit 0. Valid JSON stdout makes this parse to
    // `Ok(RecipeRunResult { success: false })` — the non-`Err` terminal-failure
    // path. `$5` is the `-C` value in the fixed argv layout.
    write_stub(
        &runner,
        "#!/bin/sh\ncase \"$1\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\ngit -C \"$5\" config core.bare true\n\
         printf '{\"recipe_name\":\"structured-fail\",\"success\":false}\\n'\nexit 0\n",
    );

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: structured-fail-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let mut env = EnvGuard::capture(&[]);
    env.assign(RUNNER_PATH_ENV, &runner);
    env.assign(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    env.clear(RUNNER_TIMEOUT_ENV);

    let result =
        execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), false, true, &repo, &[], None);
    let run = result.expect("structured failure must surface as Ok(RecipeRunResult)");
    assert!(
        !run.success,
        "precondition: runner reported a structured failure (success == false)"
    );

    // Same restore guarantee as the `Err` path: caller checkout left usable.
    let bare_after = git_in(&repo, &["config", "--get", "core.bare"]);
    assert!(
        matches!(bare_after.as_deref(), Some("false") | None),
        "structured-failure cleanup must restore the caller checkout's core.bare \
         (issue #964): core.bare is still {bare_after:?} after the failed run"
    );
    assert!(
        git_in(&repo, &["status", "--porcelain"]).is_some(),
        "structured-failure cleanup must leave the caller checkout usable \
         (`git status` must succeed) after the failed run (issue #964)"
    );
}

// --- issue #1326: the depth-claim decision, without fabricating a process tree ---

#[test]
fn uncorroborated_stale_depth_is_treated_as_a_new_root() {
    // A leftover AMPLIHACK_SESSION_DEPTH in a surviving shell must not wedge the
    // user. Nothing vouches for it, so it is discarded.
    assert_eq!(execute::resolve_claimed_depth(4, None, false), 0);
}

#[test]
fn corroborated_depth_is_believed_and_left_for_the_ceiling_check() {
    // A tree id, or a live orchestrator ancestor, makes the claim actionable. The
    // caller then refuses, because no sealed ceiling can be found for it.
    assert_eq!(execute::resolve_claimed_depth(4, None, true), 4);
}

#[test]
fn a_sealed_tree_always_makes_the_claim_actionable() {
    // Once a ceiling exists there is something to clamp against, so corroboration
    // is irrelevant and the depth stands either way.
    assert_eq!(execute::resolve_claimed_depth(2, Some(3), false), 2);
    assert_eq!(execute::resolve_claimed_depth(2, Some(3), true), 2);
}

#[test]
fn depth_zero_is_never_rewritten() {
    for corroborated in [false, true] {
        for sealed in [None, Some(0), Some(3)] {
            assert_eq!(execute::resolve_claimed_depth(0, sealed, corroborated), 0);
        }
    }
}

#[test]
fn generated_tree_ids_are_valid_and_distinct() {
    // `ensure_sealed` validates the id, and it is only ever generated on the root
    // path -- the least exercised one -- so a format change that broke
    // `validate_tree_id` would surface late and only for real users.
    use crate::commands::session_tree::state::validate_tree_id;
    let a = execute::new_tree_id();
    let b = execute::new_tree_id();
    validate_tree_id(&a).expect("generated tree id must be valid");
    validate_tree_id(&b).expect("generated tree id must be valid");
    assert_ne!(a, b, "two ids from one process must differ");
}

/// The environment a recipe fixture runs in: a temp `HOME` owned by this
/// fixture, a private work root with no end-of-run sweep (issue #2067; see
/// `test_support::IsolatedWorkRoot`), and the variables it changed. Field
/// order is drop order: the variables are restored first, then the work root
/// guard restores its variables and removes its folder, then the temp `HOME`
/// is removed, so no `amplihack-recipe-home-*` directory outlives the test.
struct FixtureEnvironment {
    _env: EnvGuard,
    _work_root: crate::test_support::IsolatedWorkRoot,
    _home: tempfile::TempDir,
}

/// The caller holds the shared environment lock until the fixture drops.
fn fixture_environment() -> FixtureEnvironment {
    // Issue #2019: the runner and anything it stages see a temp HOME, never
    // the developer's real ~/.amplihack.
    let home = tempfile::Builder::new()
        .prefix("amplihack-recipe-home-")
        .tempdir()
        .expect("create the recipe fixture HOME");
    let work_root = crate::test_support::IsolatedWorkRoot::new();
    let mut env = EnvGuard::capture(&[]);
    env.assign("HOME", home.path());
    // These assertions exercise generic runner/Claude staging, without a Codex stub.
    env.assign("AMPLIHACK_AGENT_BINARY", "claude");
    // A fixture starts its own run even inside an active recipe session (#1984).
    for key in crate::test_support::OUTER_RUN_IDENTITY_VARS {
        env.clear(key);
    }
    FixtureEnvironment {
        _env: env,
        _work_root: work_root,
        _home: home,
    }
}
