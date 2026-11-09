//! Issue #2018: a headless recipe step stops when the process that started
//! it exits.
//!
//! A launch is a headless step when `AMPLIHACK_RECIPE_RUN_ID` is non-empty
//! and stdin and stdout are not both terminals. On Linux such a launcher
//! binds itself to its parent with `PR_SET_PDEATHSIG`, so the kernel sends
//! it SIGTERM when the recipe runner (or the usage-limit supervisor) exits,
//! and the launcher then ends its agent. The agent's own binding is set in
//! `ManagedChild::spawn_bound_to_parent`.
//!
//! The expected parent is read by [`note_startup_parent`], the first
//! statement of the binary's `main`, before the update notice, self-heal or
//! asset repair can take any time. The binding later compares the parent
//! after `prctl` with that early reading, so a runner that died while the
//! launcher was starting up is detected even though the launcher has
//! already been reparented (prctl(2) describes this race).
//!
//! The decision and the binding are made once per process, on the main
//! thread, and cached: the dispatch arms call [`bind_launcher_if_headless`]
//! before the usage-limit supervisor runs, and `run_launch_core` calls it
//! again to cover every other way in (`rustyclawd`, `profile run`,
//! `--supervise`). This module knows nothing about step records.

use std::io::IsTerminal;
use std::sync::OnceLock;

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

/// What [`bind_launcher_if_headless`] decided for this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// This launch is a headless recipe step.
    pub headless: bool,
    /// The expected parent: `getppid()` as read by [`note_startup_parent`]
    /// at the start of `main`, or at the binding when nothing read it
    /// earlier. 0 when the parent is outside this process's PID namespace.
    pub parent_pid: i32,
    /// The parent's start time, read at the same moment. `None` off Linux,
    /// for an interactive launch, or when it could not be read.
    pub parent_start_ticks: Option<u64>,
}

/// True when `run_id` is non-empty and stdin and stdout are not both
/// terminals. `AMPLIHACK_NONINTERACTIVE` plays no part.
pub fn is_headless_step(run_id: Option<&str>, stdin_tty: bool, stdout_tty: bool) -> bool {
    run_id.is_some_and(|id| !id.is_empty()) && !(stdin_tty && stdout_tty)
}

/// D1: the exit status of a headless launcher stopped by `first`, the first
/// of SIGTERM, SIGINT or SIGHUP it received: `128 + N`, or 143 when the
/// signal is unknown or not one of the three.
pub fn headless_signal_exit_code(first: Option<i32>) -> i32 {
    match first {
        Some(sig @ (SIGTERM | SIGINT | SIGHUP)) => 128 + sig,
        _ => 128 + SIGTERM,
    }
}

static STARTUP_PARENT: OnceLock<i32> = OnceLock::new();

/// Records `getppid()` as this process's expected parent. The binary calls
/// it as the first statement of `main`; later calls change nothing. An
/// entry point that never calls it reads the parent at the binding instead,
/// which leaves the window between its start and the binding unchecked.
pub fn note_startup_parent() {
    // SAFETY: getppid has no preconditions and cannot fail.
    STARTUP_PARENT.get_or_init(|| unsafe { libc::getppid() });
}

/// The parent [`note_startup_parent`] recorded, if it ran.
fn startup_parent() -> Option<i32> {
    STARTUP_PARENT.get().copied()
}

/// Debug builds only: when `AMPLIHACK_TEST_STARTUP_GATE` names a FIFO, wait
/// here until a writer opens it and closes it again. The binary calls this
/// between the self-heal and dispatch, so a test can hold a launcher inside
/// the startup window and end its runner there. Release builds hold neither
/// the name nor the wait.
#[cfg(debug_assertions)]
pub fn startup_test_gate() {
    const GATE_ENV: &str = "AMPLIHACK_TEST_STARTUP_GATE";
    let Some(path) = std::env::var_os(GATE_ENV).filter(|value| !value.is_empty()) else {
        return;
    };
    if let Ok(mut gate) = std::fs::File::open(&path) {
        let _ = std::io::Read::read_to_end(&mut gate, &mut Vec::new());
    }
}

/// Release builds: nothing.
#[cfg(not(debug_assertions))]
pub fn startup_test_gate() {}

/// Decides whether this launch is a headless step and, if it is, binds the
/// launcher to its parent. Idempotent: the first call decides and binds,
/// later calls return the same answer.
///
/// Must be called on the main thread, because the parent-death signal
/// follows the thread that set it up in the parent, and the agent is forked
/// from this process's main thread.
///
/// If the parent has already gone when the binding takes effect, this
/// prints one line and exits with status 1. It must therefore run before
/// any guard, lock, slot or child exists.
pub fn bind_launcher_if_headless() -> Binding {
    static BINDING: OnceLock<Binding> = OnceLock::new();
    *BINDING.get_or_init(decide_and_bind)
}

fn decide_and_bind() -> Binding {
    // SAFETY: getppid has no preconditions and cannot fail.
    let parent_pid = startup_parent().unwrap_or_else(|| unsafe { libc::getppid() });
    let interactive = Binding {
        headless: false,
        parent_pid,
        parent_start_ticks: None,
    };
    // The unit-test harness runs on named threads and may inherit
    // AMPLIHACK_RECIPE_RUN_ID from the recipe step running the tests, so it
    // is never bound. The integration tests cover the real binary.
    if cfg!(test) {
        return interactive;
    }
    let run_id = std::env::var_os(crate::commands::governor::RECIPE_RUN_ID_ENV);
    let run_id = run_id.as_ref().map(|value| value.to_string_lossy());
    let headless = is_headless_step(
        run_id.as_deref(),
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    );
    if !headless {
        return interactive;
    }
    debug_assert_eq!(
        std::thread::current().name(),
        Some("main"),
        "a headless launcher must bind on the main thread"
    );
    let binding = Binding {
        headless: true,
        parent_pid,
        parent_start_ticks: parent_start_ticks(parent_pid),
    };
    bind_to_parent(parent_pid);
    tracing::debug!(parent_pid, "headless recipe step bound to its parent");
    binding
}

#[cfg(target_os = "linux")]
fn parent_start_ticks(parent_pid: i32) -> Option<u64> {
    if parent_pid <= 0 {
        return None;
    }
    crate::commands::recipe::process_tree::start_time(parent_pid)
}

#[cfg(not(target_os = "linux"))]
fn parent_start_ticks(_parent_pid: i32) -> Option<u64> {
    None
}

/// Sets SIGTERM as this process's parent-death signal, then checks that the
/// parent read at startup is still the parent. A refusal by the kernel is a
/// warning; a parent that has gone ends this process with status 1.
#[cfg(target_os = "linux")]
fn bind_to_parent(parent_pid: i32) {
    // SAFETY: prctl(PR_SET_PDEATHSIG) only changes this process's own
    // parent-death signal.
    let rc = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong) };
    if rc == -1 {
        let error = std::io::Error::last_os_error();
        eprintln!(
            "amplihack: warning: cannot bind this headless step to its parent ({error}); \
             continuing without the binding"
        );
        return;
    }
    // SAFETY: getppid has no preconditions and cannot fail.
    if unsafe { libc::getppid() } != parent_pid {
        // Nothing has been started yet: no guard, lock, slot or child.
        eprintln!("amplihack: headless step parent exited before launch; not starting");
        std::process::exit(1);
    }
}

/// No parent-death signal off Linux: the step can outlive its runner, and
/// its record lets `recipe steps` report it.
#[cfg(not(target_os = "linux"))]
fn bind_to_parent(_parent_pid: i32) {}

#[cfg(test)]
#[path = "step_lifetime_tests.rs"]
mod tests;
