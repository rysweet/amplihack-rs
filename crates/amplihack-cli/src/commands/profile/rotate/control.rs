//! `rotate status` and `rotate stop`: read the pid file and the ledger, and
//! signal only a verified rotator. Also the ledger side of `profile
//! overflow list` and `clear` (issue #1606).

use super::daemon::{DaemonState, current_state, daemon_err};
use super::identify::{self, LiveId};
use super::ledger::{LastCycle, Phase, RotateLedger, RotateState, active_blocks};
use super::log::{msg, timestamp};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::{
    self,
    kit::{State, now},
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::overflow_names;
use crate::commands::profile::pointer;
use crate::commands::profile::root_cmd::{pinned_line, root_line};
use crate::commands::profile::root_config::{ROTATE_ROOT_FILE, RootSource, config_dir, read_saved};
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::sanitize_display;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

const STOP_WAIT: Duration = Duration::from_secs(10);
/// How long `profile overflow clear` waits for the rotate state lock.
const CLEAR_LOCK_WAIT: Duration = Duration::from_secs(10);

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
    // One read, so the `overflow:`, `on credits:` and `default:` lines agree.
    let names = overflow_names(store.root());
    let overflow = overflow_line(&names, ledger.as_ref().map(Option::as_ref), now());
    let last_tag = ledger
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .and_then(|l| l.last_cycle.as_ref())
        .and_then(|c| c.overflow.clone());
    let pin = super::pin::read(&rs);
    let on_overflow = on_overflow_line(&store, &rs, home, &names, last_tag.as_deref(), &pin);
    let api_key = api_key_line(&store, home, ledger.as_ref().ok().and_then(Option::as_ref));
    let live = match &ledger {
        Ok(l) => l
            .as_ref()
            .and_then(|l| l.live.clone())
            .unwrap_or_else(|| "-".into()),
        Err(_) => "- (ledger unreadable)".into(),
    };
    let ledger = ledger.ok().flatten();
    let last = match ledger.as_ref().and_then(|l| l.last_cycle.as_ref()) {
        Some(LastCycle {
            at,
            outcome,
            overflow,
            ..
        }) => {
            let tag = overflow.as_ref().map_or_else(String::new, |t| {
                format!(" overflow={}", sanitize_display(t, false))
            });
            format!(
                "{} {}{tag}",
                timestamp(*at),
                sanitize_display(outcome, false)
            )
        }
        None => "-".into(),
    };
    let auto = super::autostart_line(store.root());
    println!("rotate: {word}\n{auto}\npid: {pid}\nlive: {live}\nlast cycle: {last}");
    println!("overflow: {overflow}\napi key: {api_key}\non overflow: {on_overflow}");
    if let Some(line) = super::platform_live_line(store.root()) {
        println!("{line}");
    }
    // Issue #1818: profiles whose stored reading has no session window. Not
    // labelled "no session limit": the stored reading cannot tell a plan
    // with none from a session window that was not recorded.
    if let Some(line) = no_session_window_line(&store) {
        println!("{line}");
    }
    // `.default` bypasses the rotator: say so when it names an overflow profile.
    if let Ok(Some(d)) = pointer::read(store.root())
        && names.contains(&d)
    {
        println!("default: {d} (overflow; launches use it directly)");
    }
    // Issue #1691: a hold leaves the switch as the last cycle, so say it.
    match pin {
        super::pin::PinState::None => {}
        super::pin::PinState::Pinned(n) => println!("hold: {n}"),
        super::pin::PinState::Missing(n) => println!("hold: {n} (profile missing; ignored)"),
        super::pin::PinState::Unreadable => println!("hold: unreadable"),
    }
    for line in ledger.as_ref().map(status_alerts).unwrap_or_default() {
        println!("{line}");
    }
    match rs.load_journal() {
        Ok(Some(j)) if j.phase == Phase::Inconsistent => println!("journal: inconsistent"),
        Ok(Some(j)) => println!("journal: pending (phase {})", j.phase.as_str()),
        Ok(None) => {}
        Err(_) => println!("journal: damaged"),
    }
    println!("log: {}", rs.log.display());
}

/// The `alert:` lines of the ledger's hand-change guidance, then a
/// `warning:` line for each profile in `lost_login` (issue #1693). Shows
/// what the last cycle decided; compares no accounts.
pub fn status_alerts(l: &RotateLedger) -> Vec<String> {
    let alerts = l.guidance.iter().flatten().map(|g| {
        let text = g.strip_prefix("ALERT: ").unwrap_or(g);
        format!("alert: {}", sanitize_display(text, false))
    });
    let warnings = l
        .lost_login
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .map(|n| {
            let line = msg::lost_login(&n);
            let text = line.strip_prefix("ALERT: ").unwrap_or(&line);
            format!("warning: {}", sanitize_display(text, false))
        });
    alerts.chain(warnings).collect()
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

/// The `no session window:` line, or `None` when no current profile's stored
/// reading has a week window and no session window. Read-only.
fn no_session_window_line(store: &ProfileStore) -> Option<String> {
    let names = store.list().unwrap_or_default();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let free = auto_switch::no_session_window_profiles(store.root(), &names);
    if free.is_empty() {
        return None;
    }
    let free: Vec<String> = free.iter().map(|n| sanitize_display(n, false)).collect();
    Some(format!("no session window: {}", free.join(", ")))
}

/// The `overflow:` value: the marked profiles `names` in list order, each
/// with its active spend block, or `-`. An unreadable ledger (`Err`) adds
/// `(blocks unknown)` rather than showing no blocks.
fn overflow_line(
    names: &[ProfileName],
    ledger: Result<Option<&RotateLedger>, &ProfileError>,
    now: i64,
) -> String {
    if names.is_empty() {
        return "-".into();
    }
    let (blocks, unknown) = match ledger {
        Ok(l) => (l.map(|l| active_blocks(l, now)).unwrap_or_default(), false),
        Err(_) => (BTreeMap::new(), true),
    };
    let line = names
        .iter()
        .map(|n| match blocks.get(n.as_str()) {
            Some(b) => format!("{n} (blocked until {})", timestamp(b.until)),
            None => n.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    if unknown {
        format!("{line} (blocks unknown)")
    } else {
        line
    }
}

/// The `on overflow:` value: `yes (<name>, held by switch)` when the pin
/// names the live overflow profile; `yes (<name>)` when the login identified in
/// `~/.claude` is an overflow profile and the rotator's last cycle recorded
/// `overflow=in` or `on` (it found no plan capacity), `no (<name> is
/// overflow; plan capacity remains)` when it is an overflow profile without
/// that tag, `no` for a plan profile, `-` when the live profile cannot be
/// identified. Runs no probe.
/// The `api key:` line (issue #1700): the API-key profile the tier uses,
/// with the active API-key block, or `-`.
fn api_key_line(
    store: &ProfileStore,
    home: Option<&Path>,
    ledger: Option<&RotateLedger>,
) -> String {
    let platforms = crate::commands::profile::kind::Ctx::load(store, home).platform_profiles(store);
    let Some(first) = platforms.first() else {
        return "-".into();
    };
    match ledger.and_then(|l| super::platform_block::active(l, now())) {
        Some(b) => format!(
            "{first} (blocked until {}, {})",
            timestamp(b.until),
            sanitize_display(&b.reason, false)
        ),
        None => first.to_string(),
    }
}

pub(super) fn on_overflow_line(
    store: &ProfileStore,
    rs: &RotateState,
    home: Option<&Path>,
    overflow: &[ProfileName],
    last_tag: Option<&str>,
    pin: &super::pin::PinState,
) -> String {
    let live = home.and_then(|h| identify::live(store, rs, h).ok());
    // An API-key login is always billed per token, whatever the last cycle.
    if let Some((LiveId::Known(n), _)) = &live
        && rs.load_ledger().ok().flatten().is_some_and(|l| {
            l.live_kind.as_deref() == Some(super::ledger::LIVE_KIND_PLATFORM)
                && l.live.as_deref() == Some(n.as_str())
        })
    {
        return format!("yes ({n})");
    }
    match live {
        Some((LiveId::Known(n), _)) if overflow.contains(&n) => {
            // A pin holds every cycle before the overflow policy runs, so a
            // `profile switch` onto an overflow profile spends credits until
            // `profile switch --none`, plan capacity or not.
            if *pin == super::pin::PinState::Pinned(n.clone()) {
                format!("yes ({n}, held by switch)")
            } else if matches!(last_tag, Some("in" | "on")) {
                format!("yes ({n})")
            } else {
                format!("no ({n} is overflow; plan capacity remains)")
            }
        }
        Some((LiveId::Known(_), _)) => "no".into(),
        _ => "-".into(),
    }
}

/// The active spend blocks under `root`, by profile name, with their end
/// time. Empty when there is no ledger. Takes no lock.
pub fn overflow_blocks(root: &Path, now: i64) -> Result<BTreeMap<String, i64>, ProfileError> {
    let l = RotateState::new(root).load_ledger()?;
    Ok(l.map(|l| {
        active_blocks(&l, now)
            .into_iter()
            .map(|(n, b)| (n, b.until))
            .collect()
    })
    .unwrap_or_default())
}

/// Removes the spend block for `name` under the rotate state lock, waiting
/// up to [`CLEAR_LOCK_WAIT`]. `Ok(false)` when there was none. Never creates
/// the ledger, and never writes it without the lock.
pub fn clear_overflow_block(root: &Path, name: &ProfileName) -> Result<bool, ProfileError> {
    let rs = RotateState::new(root);
    if !crate::commands::profile::auto_switch::kit::exists(&rs.ledger) {
        return Ok(false);
    }
    let state = State::new(root)?;
    let Some(_lock) = state.try_lock(CLEAR_LOCK_WAIT)? else {
        return Err(daemon_err("rotator busy, retry"));
    };
    let Some(mut l) = rs.load_ledger()? else {
        return Ok(false);
    };
    if l.overflow_blocked.remove(name.as_str()).is_none() {
        return Ok(false);
    }
    rs.save_ledger(&l)?;
    Ok(true)
}
