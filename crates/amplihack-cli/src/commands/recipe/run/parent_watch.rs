//! Issue #1976 (D8, D9): a nested `amplihack recipe run` stops itself when
//! the parent `recipe run` that started it exits.
//!
//! Every `recipe run` exports its own pid and start time (field 22 of
//! `/proc/<pid>/stat`) to its runner child as [`PARENT_PID_ENV`] and
//! [`PARENT_STARTTIME_ENV`]. A run that inherits both opens a pidfd on that
//! parent and blocks in `poll()` with no timeout on a detached thread. When
//! the parent exits, the thread sets the orphan stop in `stop_signal`, and
//! the existing wait loop tears the runner's process group down.
//!
//! The start time guards against a reused pid. A top-level run, including a
//! `--detach` pane root (detach removes both variables), watches nothing.
//! Linux only; on other platforms [`start`] does nothing.

/// The parent `recipe run`'s pid, exported to the runner child.
pub(crate) const PARENT_PID_ENV: &str = "AMPLIHACK_PARENT_RECIPE_PID";
/// The parent's start time in clock ticks since boot (field 22 of its stat).
pub(crate) const PARENT_STARTTIME_ENV: &str = "AMPLIHACK_PARENT_RECIPE_STARTTIME";

/// The variables that tie a run to the `recipe run` above it: the parent it
/// watches and the auto-drive state directory it may write `orphaned.json`
/// into. A run started outside that tree must not inherit them: a `--detach`
/// pane drops them (`detach::PANE_ENV_REMOVED`), and so does a command behind
/// `$(amplihack agent-binary --shell)`, because a tmux server started from a
/// step keeps the step's values in its global environment and would hand
/// them to every later session (crusty review of #2112).
pub(crate) const PARENT_WATCH_ENV: [&str; 3] = [
    PARENT_PID_ENV,
    PARENT_STARTTIME_ENV,
    super::orphan_record::AUTODRIVE_STATE_DIR_ENV,
];

/// Field 22 (`starttime`) of a `/proc/<pid>/stat` line. Fields are counted
/// after the LAST `)`, so a process name holding `)` and spaces cannot shift
/// them (C-S2).
pub(crate) fn proc_starttime(stat_text: &str) -> Option<u64> {
    let (_, rest) = stat_text.rsplit_once(')')?;
    // After `)`, field 3 (state) comes first, so field 22 is index 19.
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// The parent variables, parsed strictly (C-S1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParentEnv {
    /// Neither variable is set: a top-level run.
    Absent,
    /// Both are set and valid.
    Parsed { pid: i32, starttime: u64 },
    /// One is missing or a value is bad. Warned and treated as top level.
    Invalid(String),
}

pub(crate) fn parse_parent_env(pid: Option<&str>, starttime: Option<&str>) -> ParentEnv {
    match (pid, starttime) {
        (None, None) => ParentEnv::Absent,
        (Some(_), None) => ParentEnv::Invalid(format!("{PARENT_STARTTIME_ENV} is not set")),
        (None, Some(_)) => ParentEnv::Invalid(format!("{PARENT_PID_ENV} is not set")),
        (Some(pid), Some(starttime)) => {
            let pid = match pid.trim().parse::<i32>() {
                Ok(pid) if pid > 1 => pid,
                _ => {
                    return ParentEnv::Invalid(format!(
                        "{PARENT_PID_ENV} is not a process id greater than 1"
                    ));
                }
            };
            match starttime.trim().parse::<u64>() {
                Ok(starttime) => ParentEnv::Parsed { pid, starttime },
                Err(_) => ParentEnv::Invalid(format!(
                    "{PARENT_STARTTIME_ENV} is not a non-negative number"
                )),
            }
        }
    }
}

/// What [`inspect_parent`] found.
#[derive(Debug)]
pub(crate) enum ParentState {
    /// The parent lives; the pidfd becomes readable when it exits.
    #[cfg(target_os = "linux")]
    Alive(std::os::fd::OwnedFd),
    /// The parent has exited, or the pid now names another process.
    Gone,
    /// The parent cannot be watched (no pidfd support, or another error).
    Unwatchable(String),
}

/// Open a pidfd on `pid`, then read its stat and compare the start time
/// (C-S3). The pidfd is taken first, so the process it names is the one
/// whose start time is checked.
#[cfg(target_os = "linux")]
pub(crate) fn inspect_parent(pid: i32, starttime: u64) -> ParentState {
    use std::os::fd::{FromRawFd, OwnedFd};

    // SAFETY: pidfd_open takes a pid and a flags word and returns a new file
    // descriptor or -1; it touches no memory of this process. The fd it
    // returns has close-on-exec set by the kernel.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if raw < 0 {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ESRCH) => ParentState::Gone,
            _ => ParentState::Unwatchable(format!("pidfd_open({pid}) failed: {error}")),
        };
    }
    let Ok(raw) = i32::try_from(raw) else {
        return ParentState::Unwatchable(format!("pidfd_open({pid}) returned {raw}"));
    };
    // SAFETY: `raw` is a file descriptor pidfd_open just returned, owned by
    // nothing else; OwnedFd closes it exactly once.
    let pidfd = unsafe { OwnedFd::from_raw_fd(raw) };
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ParentState::Gone,
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => return ParentState::Gone,
        Err(error) => {
            return ParentState::Unwatchable(format!("cannot read /proc/{pid}/stat: {error}"));
        }
    };
    // A zombie has already exited.
    let state = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next());
    if matches!(state, Some("Z" | "X")) {
        return ParentState::Gone;
    }
    match proc_starttime(&stat) {
        Some(actual) if actual == starttime => ParentState::Alive(pidfd),
        Some(_) => ParentState::Gone,
        None => ParentState::Unwatchable(format!("cannot parse /proc/{pid}/stat")),
    }
}

/// This process's own start time, for the runner child's environment.
#[cfg(target_os = "linux")]
pub(crate) fn own_starttime() -> Option<u64> {
    proc_starttime(&std::fs::read_to_string("/proc/self/stat").ok()?)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn own_starttime() -> Option<u64> {
    None
}

/// Start watching the parent named by the environment, if any. Called once,
/// after `stop_signal::arm`. A parent already gone stops the run at once, so
/// the pre-spawn stop check keeps the runner from ever starting (C-S8).
#[cfg(target_os = "linux")]
pub(crate) fn start() {
    use super::stop_signal::{OrphanInfo, orphan_stop_line, request_orphan_stop};

    let pid = std::env::var(PARENT_PID_ENV).ok();
    let starttime = std::env::var(PARENT_STARTTIME_ENV).ok();
    let (pid, starttime) = match parse_parent_env(pid.as_deref(), starttime.as_deref()) {
        ParentEnv::Absent => return,
        ParentEnv::Parsed { pid, starttime } => (pid, starttime),
        ParentEnv::Invalid(why) => {
            warn(&format!(
                "ignoring {PARENT_PID_ENV}/{PARENT_STARTTIME_ENV}: {why}; this run will not \
                 watch a parent recipe run"
            ));
            return;
        }
    };
    match inspect_parent(pid, starttime) {
        ParentState::Gone => {
            eprintln!("{}", orphan_stop_line(pid));
            request_orphan_stop(OrphanInfo { parent_pid: pid });
        }
        ParentState::Unwatchable(why) => warn(&format!(
            "cannot watch parent recipe run {pid} ({PARENT_PID_ENV}): {why}; this run will \
             not stop if it exits"
        )),
        ParentState::Alive(pidfd) => {
            let spawned = std::thread::Builder::new()
                .name("recipe-parent-watch".to_string())
                .spawn(move || watch(pidfd, pid));
            if let Err(error) = spawned {
                warn(&format!(
                    "cannot start the watch on parent recipe run {pid} ({PARENT_PID_ENV}): \
                     {error}; this run will not stop if it exits"
                ));
            }
            // Detached: never joined. It ends with the process.
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn start() {}

/// Block until the parent exits, then request the orphan stop. A poll error
/// gives up with a warning and never stops a healthy run (C-S5).
#[cfg(target_os = "linux")]
fn watch(pidfd: std::os::fd::OwnedFd, pid: i32) {
    use super::stop_signal::{OrphanInfo, orphan_stop_line, request_orphan_stop};
    use std::os::fd::AsRawFd;

    let mut pollfd = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        pollfd.revents = 0;
        // SAFETY: `pollfd` is a valid, initialised pollfd that outlives the
        // call, and the count is 1. A timeout of -1 waits for the event.
        let ready = unsafe { libc::poll(&mut pollfd, 1, -1) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            warn(&format!(
                "the watch on parent recipe run {pid} failed: {error}; this run will not stop \
                 if it exits"
            ));
            return;
        }
        if pollfd.revents & libc::POLLNVAL != 0 {
            warn(&format!(
                "the watch on parent recipe run {pid} lost its pidfd; this run will not stop \
                 if it exits"
            ));
            return;
        }
        if pollfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            eprintln!("{}", orphan_stop_line(pid));
            request_orphan_stop(OrphanInfo { parent_pid: pid });
            return;
        }
    }
}

fn warn(message: &str) {
    tracing::warn!("{message}");
    eprintln!("warning: {message}");
}
