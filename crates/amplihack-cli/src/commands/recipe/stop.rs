//! `amplihack recipe stop <session|run_id>` (issue #1606): end a recipe run's
//! whole process tree.
//!
//! - A tmux session that `recipe run --detach` started (checked: a pane runs
//!   `amplihack recipe detached-child`): only that pane's process group gets
//!   SIGTERM, and only its tree is swept. The pane runs `recipe run`, which
//!   tears its runner's tree down on that signal (see `run/stop_signal.rs`).
//!   Whatever is left of that tree after the grace period gets SIGKILL, then
//!   the session is killed, which closes any other pane (a `tail -f` you
//!   split off) the way tmux always does, without a signal from here.
//! - A run id (the `run_id` of an `amplihack.recipe.log_pointer` line): every
//!   process started with `AMPLIHACK_RECIPE_RUN_ID=<run_id>` is found in
//!   `/proc`, and the same SIGTERM, grace, SIGKILL is applied to them, their
//!   descendants and their process groups on Linux. On Darwin, a private
//!   launch record and authenticated live Unix peer identify the exact owner;
//!   native start identities attribute descendants without group expansion.
//!
//! The tree is found once, before any signal: by parent links and process
//! groups, so a descendant that started its own session is still in it. Each
//! process is recorded with its start time and is signalled only while that
//! still matches. The check and signal are separate syscalls on Darwin;
//! known replacement identities are refused. This process and its ancestors
//! are never signalled.

use std::collections::{BTreeMap, BTreeSet};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

/// How long the tree gets to exit after SIGTERM. `recipe run` itself waits up
/// to its teardown grace (5 s by default) before it SIGKILLs the runner's
/// group, so this is longer than that.
const STOP_GRACE: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(100);

/// What a detached run's pane runs; a session without it is not ours.
const DETACHED_CHILD: &str = "recipe detached-child";

/// Processes to make sure of, each with the start time it had when found.
pub(crate) type Tree = BTreeMap<i32, u64>;

/// Run `amplihack recipe stop <target>`.
pub fn run_stop(target: &str) -> Result<()> {
    if target.is_empty() || target.starts_with('-') {
        bail!("recipe stop: expected a tmux session name or a run id");
    }
    if tmux_has_session(target) {
        return stop_session(target);
    }
    stop_run_id(target)
}

fn tmux(args: &[&str]) -> Option<std::process::Output> {
    Command::new("tmux")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()
}

/// `=name` makes tmux match the session name exactly, not as a prefix.
fn exact(target: &str) -> String {
    format!("={target}")
}

fn tmux_has_session(target: &str) -> bool {
    tmux(&["has-session", "-t", &exact(target)]).is_some_and(|out| out.status.success())
}

fn stop_session(session: &str) -> Result<()> {
    let listing = tmux(&[
        "list-panes",
        "-s",
        "-t",
        &exact(session),
        "-F",
        "#{pane_pid} #{pane_start_command}",
    ])
    .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    .unwrap_or_default();
    // Only the panes running the detached run: a pane the user split off in
    // the same session is not signalled or swept.
    let panes: BTreeSet<i32> = listing
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(_, command)| command.contains(DETACHED_CHILD))
        .filter_map(|(pid, _)| pid.trim().parse().ok())
        .collect();
    if panes.is_empty() {
        bail!(
            "recipe stop: tmux session {session} was not started by `amplihack recipe run \
             --detach`; refusing to stop it"
        );
    }
    let tree = snapshot_tree(&panes);
    // A pane this command itself runs in is left to `kill-session` below.
    #[cfg(target_os = "macos")]
    terminate(&tree);
    #[cfg(not(target_os = "macos"))]
    for pane in panes.iter().filter(|pid| tree.contains_key(pid)) {
        signal_group(*pane, libc::SIGTERM);
    }
    let killed = sweep(&tree, STOP_GRACE);
    let _ = tmux(&["kill-session", "-t", &exact(session)]);
    report(&format!("session {session}"), tree.len(), killed);
    Ok(())
}

#[cfg(target_os = "linux")]
fn stop_run_id(run_id: &str) -> Result<()> {
    use super::process_tree::{snapshot, with_env};
    let procs = snapshot();
    let roots = with_env(&procs, "AMPLIHACK_RECIPE_RUN_ID", run_id);
    let tree = snapshot_tree(&roots);
    if tree.is_empty() {
        bail!("recipe stop: no tmux session or running recipe run named {run_id}");
    }
    terminate(&tree);
    let killed = sweep(&tree, STOP_GRACE);
    report(&format!("run {run_id}"), tree.len(), killed);
    Ok(())
}

#[cfg(target_os = "macos")]
fn stop_run_id(run_id: &str) -> Result<()> {
    let (mut tree, (owner, start), stream) = super::run_control::request_stop(run_id)?;
    // The authenticated owner uses the normal cancellation path; frozen
    // descendants remain ours even after their parents exit or they setsid.
    terminate(&tree);
    let mut killed = sweep(&tree, STOP_GRACE);
    let final_tree = super::run_control::final_tree(&stream)?;
    terminate(&final_tree);
    killed += sweep(&final_tree, Duration::from_secs(1));
    tree.extend(final_tree);
    tree.insert(owner, start);
    verify_terminal(&tree, Duration::from_secs(5))?;
    report(&format!("run {run_id}"), tree.len(), killed);
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn verify_terminal(tree: &Tree, grace: Duration) -> Result<()> {
    let deadline = Instant::now() + grace;
    loop {
        let mut alive = Vec::new();
        for (pid, start) in tree {
            if super::darwin_process::inspect(*pid)?.is_some_and(|p| p.start == *start) {
                alive.push(*pid);
            }
        }
        if alive.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            bail!("recipe stop: still has live owned processes: {alive:?}");
        }
        thread::sleep(POLL);
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn stop_run_id(run_id: &str) -> Result<()> {
    bail!(
        "recipe stop: no tmux session named {run_id}; stopping by run id is supported \
         on Linux and macOS only"
    )
}

/// `roots`, everything descended from them and every member of their process
/// groups on Linux, with start times; never this process or its ancestors.
/// Darwin follows only positively linked descendants, with native identities.
pub(crate) fn snapshot_tree(roots: &BTreeSet<i32>) -> Tree {
    #[cfg(target_os = "linux")]
    {
        use super::process_tree::{self_and_ancestors, snapshot, tree};
        let procs = snapshot();
        let protected = self_and_ancestors(&procs);
        let found = tree(&procs, roots);
        procs
            .iter()
            .filter(|p| found.contains(&p.pid) && !protected.contains(&p.pid))
            .map(|p| (p.pid, p.start))
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        super::darwin_process::tree(roots).unwrap_or_default()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        roots.iter().map(|pid| (*pid, 0)).collect()
    }
}

/// The members of process group `pgid` and everything under them, for a
/// group whose leader has already been reaped (see `group_tree`): never the
/// leader's pid itself, which the kernel may have given to another process.
/// Empty off Linux.
pub(crate) fn snapshot_group(pgid: i32) -> Tree {
    #[cfg(target_os = "linux")]
    {
        use super::process_tree::{group_tree, self_and_ancestors, snapshot};
        let procs = snapshot();
        let protected = self_and_ancestors(&procs);
        let found = group_tree(&procs, pgid);
        procs
            .iter()
            .filter(|p| found.contains(&p.pid) && !protected.contains(&p.pid))
            .map(|p| (p.pid, p.start))
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pgid;
        Tree::new()
    }
}

/// SIGTERM every process of `tree` that is still the one recorded.
pub(crate) fn terminate(tree: &Tree) {
    for (pid, start) in tree {
        if same_process(*pid, *start) {
            signal_pid(*pid, libc::SIGTERM);
        }
    }
}

/// Wait up to `grace` for every process in `tree` to exit, then SIGKILL the
/// ones still running. Returns how many had to be killed.
pub(crate) fn sweep(tree: &Tree, grace: Duration) -> usize {
    let alive = || {
        tree.iter()
            .filter(|(pid, start)| same_process(**pid, **start))
            .map(|(pid, _)| *pid)
            .collect::<Vec<_>>()
    };
    let started = Instant::now();
    while started.elapsed() < grace && !alive().is_empty() {
        thread::sleep(POLL);
    }
    let survivors = alive();
    let mut killed = 0;
    for pid in &survivors {
        // Liveness may have changed since survivors was collected. In
        // particular, never send KILL to a reused PID from that cached list.
        if tree
            .get(pid)
            .is_some_and(|start| same_process(*pid, *start))
        {
            signal_pid(*pid, libc::SIGKILL);
            killed += 1;
        }
    }
    killed
}

/// `pid` is alive, not a zombie, and still the process recorded with `start`.
fn same_process(pid: i32, start: u64) -> bool {
    #[cfg(target_os = "linux")]
    {
        super::process_tree::start_time(pid) == Some(start)
    }
    #[cfg(target_os = "macos")]
    {
        super::darwin_process::same(pid, start)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = start;
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }
}

#[cfg(not(target_os = "macos"))]
fn signal_group(pgid: i32, signal: libc::c_int) {
    if pgid > 1 {
        // SAFETY: kill has no memory-safety preconditions.
        unsafe { libc::kill(-pgid, signal) };
    }
}

fn signal_pid(pid: i32, signal: libc::c_int) {
    if pid > 1 {
        // SAFETY: kill has no memory-safety preconditions.
        unsafe { libc::kill(pid, signal) };
    }
}

fn report(what: &str, processes: usize, killed: usize) {
    if killed == 0 {
        println!("stopped {what} ({processes} processes)");
    } else {
        println!("stopped {what} ({processes} processes; {killed} needed SIGKILL)");
    }
}
