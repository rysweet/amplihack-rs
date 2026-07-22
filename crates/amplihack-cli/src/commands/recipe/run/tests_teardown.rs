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
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// Environment-variable name the deterministic-teardown feature must honour to
/// configure the SIGTERM -> SIGKILL grace window (in whole seconds).
const TEARDOWN_GRACE_ENV: &str = "AMPLIHACK_TEARDOWN_GRACE_SECS";
const RUNNER_TIMEOUT_ENV: &str = "AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS";
const RUNNER_PATH_ENV: &str = "RECIPE_RUNNER_RS_PATH";

/// Minimal RAII guard that snapshots an env var and restores it on drop so a
/// two-phase test cannot leak state into sibling tests. Env is process-global;
/// callers must additionally hold `home_env_lock()` for serialization.
struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }

    fn apply(&self, value: impl AsRef<std::ffi::OsStr>) {
        unsafe { std::env::set_var(self.key, value) };
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// Write an executable `/bin/sh` stub at `path` and chmod it 0755.
fn write_stub(path: &Path, body: &str) {
    std::fs::write(path, body).expect("failed to write runner stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner stub");
}

/// True if a process with `pid` still exists (signal 0 probe).
fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

// -------------------------------------------------------------------------
// Test 1 — teardown fires on the FAILURE (non-timeout) path.
//
// A runner that spawns a detached descendant and then exits NON-ZERO must have
// that descendant reaped. The descendant writes a marker after a short delay;
// if teardown works the marker never appears.
//
// RED today: the failure path never reaps the tree, so the marker appears.
// -------------------------------------------------------------------------
#[test]
fn test_failure_path_reaps_orphaned_descendants() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let marker = temp.path().join("orphan-marker");

    // Spawn a descendant that writes the marker after 2s, then FAIL (exit 1).
    // This is a failure, NOT a timeout.
    write_stub(
        &runner,
        &format!(
            "#!/bin/sh\n(/bin/sleep 2; echo leaked > '{}') &\nexit 1\n",
            marker.display()
        ),
    );

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: fail-leak-probe\nsteps: []\n").expect("failed to write recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    // Ensure a fast failure is not misread as a timeout.
    let _timeout_env = EnvVarGuard::unset(RUNNER_TIMEOUT_ENV);

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        true,
        temp.path(),
        &[],
        None,
    );

    result.expect_err("non-zero runner exit must surface as an error");

    // Give the descendant time to write its marker if it was NOT reaped.
    std::thread::sleep(Duration::from_millis(2_300));
    assert!(
        !marker.exists(),
        "failure-path teardown must reap orphaned recipe-runner descendants \
         (issue #964): descendant survived and wrote {}",
        marker.display()
    );
}

// -------------------------------------------------------------------------
// Test 2 — teardown is GRACEFUL: SIGTERM is delivered before SIGKILL.
//
// The runner traps SIGTERM, records that it received it, and exits cleanly.
// With a graceful three-phase reaper the trap runs (marker written). With the
// current immediate-SIGKILL teardown the trap never runs.
//
// Uses the timeout path (which is the only path that reaps today) so the ONLY
// thing under test is SIGTERM-before-SIGKILL, isolating it from Test 1.
//
// RED today: immediate SIGKILL means the SIGTERM trap never runs -> no marker.
// -------------------------------------------------------------------------
#[test]
fn test_timeout_teardown_sends_sigterm_before_sigkill() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let marker = temp.path().join("graceful-marker");

    write_stub(
        &runner,
        &format!(
            "#!/bin/sh\ntrap 'echo graceful > \"{}\"; exit 0' TERM\n/bin/sleep 30\n",
            marker.display()
        ),
    );

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: graceful-probe\nsteps: []\n").expect("failed to write recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _timeout_env = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, "1");
    // Explicit, generous grace so the trapped child can finish cleanly.
    let _grace_env = EnvVarGuard::set(TEARDOWN_GRACE_ENV, "5");

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        true,
        temp.path(),
        &[],
        None,
    );

    result.expect_err("hung recipe-runner must time out");

    // Allow the SIGTERM trap to run and write the marker within the grace window.
    std::thread::sleep(Duration::from_millis(2_000));
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
// Test 3 — the grace window is CONFIGURABLE via AMPLIHACK_TEARDOWN_GRACE_SECS.
//
// The trapped runner needs ~1s of work to shut down cleanly.
//   * grace = 0  -> escalate to SIGKILL immediately -> clean-exit marker ABSENT.
//   * grace = 4  -> allow the ~1s cleanup to finish -> clean-exit marker PRESENT.
//
// RED today: no grace env var is honoured and SIGKILL is immediate, so the
// grace = 4 phase never produces the marker.
// -------------------------------------------------------------------------
#[test]
fn test_teardown_grace_window_is_configurable() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let _timeout_env = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, "1");
    let grace_env = EnvVarGuard::set(TEARDOWN_GRACE_ENV, "0");

    // Runs one timeout->teardown cycle against a runner whose SIGTERM handler
    // needs ~1s to finish; returns whether the clean-exit marker was written.
    let run_once = |grace: &str| -> bool {
        grace_env.apply(grace);
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let runner = temp.path().join("recipe-runner-rs");
        let marker = temp.path().join("clean-exit-marker");
        write_stub(
            &runner,
            &format!(
                "#!/bin/sh\ntrap '/bin/sleep 1; echo done > \"{}\"; exit 0' TERM\n/bin/sleep 30\n",
                marker.display()
            ),
        );
        let recipe = temp.path().join("recipe.yaml");
        std::fs::write(&recipe, "name: grace-probe\nsteps: []\n").expect("failed to write recipe");

        let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
        let result = execute::execute_recipe_via_rust(
            &recipe,
            &BTreeMap::new(),
            false,
            true,
            temp.path(),
            &[],
            None,
        );
        result.expect_err("hung recipe-runner must time out");
        // Wait comfortably past the 1s cleanup for either outcome to settle.
        std::thread::sleep(Duration::from_millis(1_800));
        marker.exists()
    };

    let wrote_with_zero_grace = run_once("0");
    let wrote_with_ample_grace = run_once("4");

    assert!(
        !wrote_with_zero_grace,
        "grace = 0 must escalate to SIGKILL immediately; the ~1s cleanup must \
         NOT complete (issue #964)"
    );
    assert!(
        wrote_with_ample_grace,
        "grace = 4 must give the runner its ~1s cleanup window before SIGKILL \
         via AMPLIHACK_TEARDOWN_GRACE_SECS (issue #964)"
    );
}

// -------------------------------------------------------------------------
// Test 4 — teardown is SCOPED to the runner's process group only.
//
// The reaper signals the runner's process group (the runner is a session
// leader via setsid). It must NEVER broadcast to unrelated / parent processes.
// A sibling process spawned directly by the test lives in the TEST's process
// group, so it must survive while the runner's own orphan is reaped.
//
// RED today: the failure-path orphan is not reaped (marker appears). The
// sibling-survives half already holds; this test also guards against a future
// over-broad kill.
// -------------------------------------------------------------------------
#[test]
fn test_teardown_is_scoped_to_runner_process_group() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");

    // Sibling process in the TEST's process group (NOT the runner's session).
    let mut sibling = std::process::Command::new("/bin/sleep")
        .arg("8")
        .spawn()
        .expect("failed to spawn sibling process");
    let sibling_pid = sibling.id();

    let runner = temp.path().join("recipe-runner-rs");
    let marker = temp.path().join("orphan-marker");
    write_stub(
        &runner,
        &format!(
            "#!/bin/sh\n(/bin/sleep 2; echo leaked > '{}') &\nexit 1\n",
            marker.display()
        ),
    );
    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: scope-probe\nsteps: []\n").expect("failed to write recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _timeout_env = EnvVarGuard::unset(RUNNER_TIMEOUT_ENV);

    let started = Instant::now();
    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        true,
        temp.path(),
        &[],
        None,
    );
    result.expect_err("non-zero runner exit must surface as an error");

    // Runner's own orphan must be reaped.
    std::thread::sleep(Duration::from_millis(2_300));
    assert!(
        !marker.exists(),
        "teardown must reap the runner's orphaned descendant (issue #964)"
    );

    // The unrelated sibling (outside the runner's process group) must survive
    // the teardown, proving the reaper is pgid-scoped and not a broadcast.
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "test setup ran too long to validate sibling survival"
    );
    assert!(
        process_alive(sibling_pid),
        "teardown must be scoped to the runner's process group and must NOT \
         signal unrelated processes (issue #964)"
    );

    // Clean up the sibling deterministically.
    unsafe { libc::kill(sibling_pid as libc::pid_t, libc::SIGKILL) };
    let _ = sibling.wait();
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
            "#!/bin/sh\necho spawned > '{}'\nexit {}\n",
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

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _timeout_env = EnvVarGuard::unset(RUNNER_TIMEOUT_ENV);
    let _depth_env = EnvVarGuard::set(SESSION_DEPTH_ENV, session_depth);
    let _max_env = match max_depth {
        Some(v) => EnvVarGuard::set(MAX_DEPTH_ENV, v),
        None => EnvVarGuard::unset(MAX_DEPTH_ENV),
    };

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
//
// RED today: no guard exists, so the runner spawns and writes the marker.
// -------------------------------------------------------------------------
#[test]
fn test_depth_guard_bails_at_limit_before_spawn() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

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
//
// RED today: no guard exists, so the runner spawns regardless.
// -------------------------------------------------------------------------
#[test]
fn test_depth_guard_fails_closed_on_malformed_session_depth() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

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
//
// RED today: no guard exists (and no clamping is applied on the spawn path).
// -------------------------------------------------------------------------
#[test]
fn test_depth_guard_clamps_forged_max_depth_to_ceiling() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

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
    let output = std::process::Command::new("git")
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
    if std::process::Command::new("git")
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
//
// RED today: no snapshot/restore logic exists, so `core.bare` stays `true` and
// `git status` remains broken after the run.
// -------------------------------------------------------------------------
#[test]
fn test_caller_git_state_restored_after_terminal_failure() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

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
        "#!/bin/sh\ngit -C \"$5\" config core.bare true\nexit 1\n",
    );

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: git-corrupt-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _timeout_env = EnvVarGuard::unset(RUNNER_TIMEOUT_ENV);

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
            "#!/bin/sh\nmkdir -p '{}'\necho durable > '{}'\ngit -C \"$5\" config core.bare true\nexit 1\n",
            child_marker.parent().unwrap().display(),
            child_marker.display()
        ),
    );

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: child-worktree-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _timeout_env = EnvVarGuard::unset(RUNNER_TIMEOUT_ENV);

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
