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
//! - A run id (the `run_id` of an `amplihack.recipe.log_pointer` line,
//!   issue #2175): `stop` reads the run's record (see `run_record.rs`) under
//!   the work root it resolves, and signals only the two processes it names
//!   and the runner's group, re-checking each start time before every
//!   signal. It never opens `/proc` as a folder or reads another process's
//!   `environ`, refuses to stop a run it descends from (its own, or an
//!   outer run when it runs under a nested `recipe run`: it walks its own
//!   parent chain, see `inside_run`), and never signals its own pid or its
//!   parent's. A missing, untrusted or stale record is
//!   "not found" and nothing is signalled; a lookup that fails to read gives
//!   the reason under that message. Signal order and accepted limits are in
//!   `docs/reference/recipe-command.md`. Linux only.
//!
//! For a session, the tree is found once, before any signal: by parent
//! links and process groups, so a descendant that started its own session is
//! still in it. Each process is recorded with its start time and is signalled
//! only while that still matches, so a pid the kernel has reused is never
//! hit. This process and its ancestors are never signalled.

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
    use super::run_record::{find, validate_run_id};
    use crate::commands::cleanup::work_root::resolve_from_env;
    let not_found =
        || anyhow::anyhow!("recipe stop: no tmux session or running recipe run named {run_id:?}");
    let inside = || anyhow::anyhow!("recipe stop: refusing to stop run {run_id:?} from inside it");
    if !validate_run_id(run_id) {
        bail!("recipe stop: invalid run id {run_id:?}");
    }
    if std::env::var("AMPLIHACK_RECIPE_RUN_ID").is_ok_and(|own| own == run_id) {
        return Err(inside());
    }
    // A work root that cannot be resolved holds no record: "not found",
    // with the reason underneath.
    let work_root = resolve_from_env().map_err(|error| {
        error
            .context("cannot resolve the work root")
            .context(not_found().to_string())
    })?;
    // A lookup that could not read the work root also refuses: "not found",
    // with the reason underneath.
    let record = find(&work_root.path, run_id)
        .map_err(|error| error.context(not_found().to_string()))?
        .ok_or_else(not_found)?;
    let tree = live_targets(&record);
    if tree.is_empty() {
        return Err(not_found());
    }
    let group = (record.runner.pgid, record.runner.start_ticks);
    if inside_group(group.0) || inside_run(&record) {
        return Err(inside());
    }
    terminate(&tree);
    signal_group_if_leader(group, libc::SIGTERM);
    // The group gets SIGKILL before its leader, while the leader can still
    // match its recorded start time. Only the two recorded pids are waited
    // on: once the runner has exited, the group gets no SIGKILL from here,
    // and members that ignored SIGTERM are left to `recipe run`'s own
    // teardown (`snapshot_group` in `run/execute.rs`), which got SIGTERM
    // above.
    let survivors = wait_for_exit(&tree, STOP_GRACE);
    signal_group_if_leader(group, libc::SIGKILL);
    kill_survivors(&tree, &survivors);
    let killed = survivors.len();
    report(&format!("run {run_id}"), tree.len(), killed);
    Ok(())
}

/// The processes `record` names whose start time still matches (issue
/// #2175): never this process or its parent. Reads only those pids' `stat`.
#[cfg(target_os = "linux")]
pub(super) fn live_targets(record: &super::run_record::Record) -> Tree {
    // SAFETY: getpid and getppid have no preconditions and cannot fail.
    let protected = unsafe { [libc::getpid(), libc::getppid()] };
    [
        (record.recipe_run.pid, record.recipe_run.start_ticks),
        (record.runner.pid, record.runner.start_ticks),
    ]
    .into_iter()
    .filter(|(pid, start)| !protected.contains(pid) && same_process(*pid, *start))
    .collect()
}

/// This process's session or process group is `pgid`: it was started from
/// inside that run, so stopping the group would stop it too.
#[cfg(target_os = "linux")]
fn inside_group(pgid: i32) -> bool {
    // SAFETY: getsid(0) and getpgid(0) only query this process.
    let (sid, pg) = unsafe { (libc::getsid(0), libc::getpgid(0)) };
    sid == pgid || pg == pgid
}

/// This process descends from the run `record` names: one of its ancestors
/// (or itself) is the recorded `recipe run` or runner with the recorded start
/// time, or is in the runner's process group. This is what catches a step of
/// a nested `recipe run` asked to stop an outer run: the nested run has its
/// own `AMPLIHACK_RECIPE_RUN_ID` and its runner its own session, but the
/// outer runner is still on its parent chain. Reads only this process's own
/// ancestry (see `process_tree::own_ancestry`).
#[cfg(target_os = "linux")]
pub(super) fn inside_run(record: &super::run_record::Record) -> bool {
    let recorded = [
        (record.recipe_run.pid, record.recipe_run.start_ticks),
        (record.runner.pid, record.runner.start_ticks),
    ];
    super::process_tree::own_ancestry()
        .iter()
        .any(|p| recorded.contains(&(p.pid, p.start)) || p.pgid == record.runner.pgid)
}

/// Signals the process group `group.0` only while its leader is still the
/// process recorded with start time `group.1`, so a reused group id is
/// never hit.
#[cfg(target_os = "linux")]
fn signal_group_if_leader(group: (i32, u64), signal: libc::c_int) {
    let (pgid, start) = group;
    if same_process(pgid, start) {
        signal_group(pgid, signal);
    }
}

#[cfg(not(target_os = "linux"))]
fn stop_run_id(run_id: &str) -> Result<()> {
    bail!(
        "recipe stop: no tmux session named {run_id}; stopping by run id needs /proc and is \
         supported on Linux only"
    )
}

/// `roots`, everything descended from them and every member of their process
/// groups, with start times; never this process or its ancestors. Off Linux
/// only the roots, by pid.
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
    #[cfg(not(target_os = "linux"))]
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
    let survivors = wait_for_exit(tree, grace);
    kill_survivors(tree, &survivors);
    survivors.len()
}

/// Wait up to `grace` for every process in `tree` to exit. Returns the pids
/// still running.
fn wait_for_exit(tree: &Tree, grace: Duration) -> Vec<i32> {
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
    alive()
}

/// SIGKILL each of `survivors` that is still the process `tree` recorded.
fn kill_survivors(tree: &Tree, survivors: &[i32]) {
    for pid in survivors {
        if tree
            .get(pid)
            .is_some_and(|start| same_process(*pid, *start))
        {
            signal_pid(*pid, libc::SIGKILL);
        }
    }
}

/// `pid` is alive, not a zombie, and still the process recorded with `start`.
fn same_process(pid: i32, start: u64) -> bool {
    #[cfg(target_os = "linux")]
    {
        super::process_tree::start_time(pid) == Some(start)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = start;
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }
}

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
