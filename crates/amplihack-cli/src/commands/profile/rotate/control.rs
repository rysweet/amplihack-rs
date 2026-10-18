//! `rotate status` and `rotate stop`: read the pid file and the ledger, and
//! signal only a verified rotator.

use super::daemon::{DaemonState, current_state, daemon_err};
use super::ledger::{LastCycle, Phase, RotateState};
use super::log::timestamp;
use crate::commands::profile::ProfileError;
use crate::commands::profile::root_cmd::{pinned_line, root_line};
use crate::commands::profile::root_config::{ROTATE_ROOT_FILE, RootSource, config_dir, read_saved};
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::sanitize_display;
use std::path::Path;
use std::time::{Duration, Instant};

const STOP_WAIT: Duration = Duration::from_secs(10);

/// The store whose rotator `status` and `stop` act on, and whether it was
/// found through `~/.amplihack/rotate-root`. The record is followed only when
/// the rotator under the resolved root is not running and the one under the
/// recorded root is. An unreadable or stale record is ignored: it can only
/// choose which pid file is read, and `stop` verifies the process before it
/// signals anything. Also returns the state found for the chosen root, so
/// callers do not probe the pid file and `/proc` a second time.
pub fn locate(store: &ProfileStore, home: Option<&Path>) -> (ProfileStore, bool, DaemonState) {
    let state = current_state(&RotateState::new(store.root()));
    if !matches!(state, DaemonState::Running(_))
        && let Some(home) = home
        && let Ok(Some(root)) = read_saved(&config_dir(home).join(ROTATE_ROOT_FILE))
    {
        let recorded = current_state(&RotateState::new(&root));
        if matches!(recorded, DaemonState::Running(_)) {
            return (ProfileStore::new(root), true, recorded);
        }
    }
    (store.clone(), false, state)
}

/// `status`: `key: value` lines after the root; changes nothing.
pub fn status(store: &ProfileStore, home: Option<&Path>, source: RootSource) {
    let (store, pinned, state) = locate(store, home);
    if pinned {
        println!("{}", pinned_line(store.root()));
    } else {
        println!("{}", root_line(store.root(), source));
    }
    let rs = RotateState::new(store.root());
    let (word, pid) = match state {
        DaemonState::Running(pid) => ("running", pid.to_string()),
        DaemonState::Stopped => ("stopped", "-".into()),
        DaemonState::Stale => ("stale", "-".into()),
        DaemonState::Unverifiable => ("unverifiable", "-".into()),
    };
    let ledger = rs.load_ledger();
    let live = match &ledger {
        Ok(l) => l
            .as_ref()
            .and_then(|l| l.live.clone())
            .unwrap_or_else(|| "-".into()),
        Err(_) => "- (ledger unreadable)".into(),
    };
    let last = match ledger.ok().flatten().and_then(|l| l.last_cycle) {
        Some(LastCycle { at, outcome }) => {
            format!("{} {}", timestamp(at), sanitize_display(&outcome, false))
        }
        None => "-".into(),
    };
    println!("rotate: {word}\npid: {pid}\nlive: {live}\nlast cycle: {last}");
    match rs.load_journal() {
        Ok(Some(j)) if j.phase == Phase::Inconsistent => println!("journal: inconsistent"),
        Ok(Some(j)) => println!("journal: pending (phase {})", j.phase.as_str()),
        Ok(None) => {}
        Err(_) => println!("journal: damaged"),
    }
    println!("log: {}", rs.log.display());
}

/// `stop`: SIGTERM to a verified rotator only, then up to 10 seconds for it
/// to exit. A stale pid file is removed and never signalled. Leaves
/// `~/.amplihack/rotate-root` in place; the next `start` replaces it.
pub fn stop(store: &ProfileStore, home: Option<&Path>) -> Result<(), ProfileError> {
    let (store, _, state) = locate(store, home);
    let rs = RotateState::new(store.root());
    match state {
        DaemonState::Stopped => println!("rotate: not running"),
        DaemonState::Stale => {
            std::fs::remove_file(&rs.pid)?;
            println!("rotate: removed a stale pid file");
        }
        DaemonState::Unverifiable => {
            return Err(daemon_err(
                "cannot verify the rotator process on this host; nothing was signalled",
            ));
        }
        DaemonState::Running(pid) => {
            terminate(&rs, pid)?;
            let start = Instant::now();
            while matches!(current_state(&rs), DaemonState::Running(p) if p == pid) {
                if start.elapsed() >= STOP_WAIT {
                    return Err(daemon_err(format!(
                        "the rotator (pid {pid}) did not exit within 10 seconds"
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            match std::fs::remove_file(&rs.pid) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
            println!("rotate: stopped (pid {pid})");
        }
    }
    Ok(())
}

/// Sends SIGTERM to `pid`, verified again after a pidfd pins it on Linux.
fn terminate(rs: &RotateState, pid: u32) -> Result<(), ProfileError> {
    let gone = || {
        daemon_err(format!(
            "the rotator (pid {pid}) changed before it was signalled"
        ))
    };
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::{FromRawFd, OwnedFd};
        // SAFETY: pidfd_open takes a pid and flags and touches no memory.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        if fd >= 0 {
            // SAFETY: `fd` is a new descriptor owned by nothing else.
            let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
            if current_state(rs) != DaemonState::Running(pid) {
                return Err(gone());
            }
            use std::os::fd::AsRawFd;
            // SAFETY: a valid pidfd, a signal number, no siginfo and no flags.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    libc::SIGTERM,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            };
            return if rc == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error().into())
            };
        }
    }
    if current_state(rs) != DaemonState::Running(pid) {
        return Err(gone());
    }
    let pid = libc::pid_t::try_from(pid).map_err(|_| gone())?;
    // SAFETY: kill has no memory-safety preconditions; the pid was verified
    // as this user's rotator just above.
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}
