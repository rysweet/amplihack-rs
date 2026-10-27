//! The background rotator (D15, D18): `start` spawns `rotate watch` in a
//! new session and `watch` runs cycles until SIGTERM or SIGINT. The pid file
//! and process checks used by `status` and `stop` (in `control.rs`) live
//! here.

use super::cycle::{self, Ctx, CycleOutcome, LOCK_WAIT, Mode, prototype_guard};
use super::ledger::{LastCycle, RotateState};
use super::log::{Sink, msg, open_or_warn};
use super::settings::Settings;
use super::swap::{self, RecoverOutcome};
use crate::commands::profile::auto_switch::kit::{
    Source, State, check_root_private, effective, euid, now, read_bounded_nofollow,
    recover_auto_switch, write_private_atomic,
};
use crate::commands::profile::root_config::{ROTATE_ROOT_FILE, config_dir, save};
use crate::commands::profile::store::{ProfileStore, ROOT_ENV};
use crate::commands::profile::usage_cmd::{PROBE_CANCEL, sanitize_display};
use crate::commands::profile::{PROFILE_ENV, ProfileError};
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const MAX_PID_FILE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonState {
    Running(u32),
    Stopped,
    Stale,
    Unverifiable,
}

pub(super) fn daemon_err(msg: impl Into<String>) -> ProfileError {
    ProfileError::RotateDaemon(msg.into())
}

/// A valid pid file: at most 32 bytes of ASCII digits (one trailing newline
/// allowed) for a pid greater than 1 that is not `own`.
pub fn parse_pid(bytes: &[u8], own: u32) -> Option<u32> {
    if bytes.len() > MAX_PID_FILE {
        return None;
    }
    let digits = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let pid: u32 = std::str::from_utf8(digits).ok()?.parse().ok()?;
    (pid > 1 && pid != own).then_some(pid)
}

/// NUL-separated argv holding `profile`, `rotate` and `watch`, whose
/// `argv[0]` has the file name `exe_name`.
pub fn is_rotator(cmdline: &[u8], exe_name: &OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let mut args = cmdline.split(|&b| b == 0).filter(|a| !a.is_empty());
    let Some(argv0) = args.next() else {
        return false;
    };
    if Path::new(OsStr::from_bytes(argv0)).file_name() != Some(exe_name) {
        return false;
    }
    let rest: Vec<&[u8]> = args.collect();
    [&b"profile"[..], b"rotate", b"watch"]
        .iter()
        .all(|w| rest.contains(w))
}

/// Whether `pid` is a rotator owned by this user, per `proc_root`.
fn verified(proc_root: &Path, pid: u32, exe_name: &OsStr) -> bool {
    let dir = proc_root.join(pid.to_string());
    let owned = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.uid() == euid());
    owned
        && matches!(read_bounded_nofollow(&dir.join("cmdline"), 64 * 1024),
            Ok(Some((raw, _))) if is_rotator(&raw, exe_name))
}

/// The daemon's state from the pid file and `proc_root`. Read-only.
pub fn state_at(rs: &RotateState, proc_root: &Path, exe_name: &OsStr) -> DaemonState {
    let raw = match read_bounded_nofollow(&rs.pid, MAX_PID_FILE as u64) {
        Ok(None) => return DaemonState::Stopped,
        Ok(Some((raw, _))) => raw,
        Err(_) => return DaemonState::Stale,
    };
    if !proc_root.is_dir() {
        return DaemonState::Unverifiable;
    }
    match parse_pid(&raw, std::process::id()) {
        Some(pid) if verified(proc_root, pid, exe_name) => DaemonState::Running(pid),
        _ => DaemonState::Stale,
    }
}

pub(super) fn exe_name() -> Option<std::ffi::OsString> {
    std::env::current_exe()
        .ok()?
        .file_name()
        .map(OsStr::to_os_string)
}

pub(super) fn current_state(rs: &RotateState) -> DaemonState {
    match exe_name() {
        Some(name) => state_at(rs, Path::new("/proc"), &name),
        None => DaemonState::Unverifiable,
    }
}

/// Why `start` refuses before it takes the lock, if it does: the external
/// rotator holds its lock, or the root is not private. Creates nothing.
pub(super) fn start_refusal(root: &Path) -> Option<ProfileError> {
    if let Err(e) = prototype_guard(root) {
        return Some(e);
    }
    check_root_private(root)
        .err()
        .map(|e| ProfileError::RotateRefused(format!("profile root {e}")))
}

/// Spawns a rotator with `spawn`, writes `rotate.pid` and records the root.
/// The caller holds the state lock and has seen the daemon `Stopped` or
/// `Stale`. Returns the pid and any failure to record the root, which does
/// not stop the rotator.
pub(super) fn launch_core(
    rs: &RotateState,
    root: &Path,
    home: &Path,
    s: &Settings,
    spawn: &Spawner,
) -> Result<(u32, Vec<ProfileError>), ProfileError> {
    let pid = spawn(rs, s, root)?;
    if let Err(e) = write_private_atomic(&rs.dir, "rotate.pid", format!("{pid}\n").as_bytes()) {
        // Without a pid file `stop` could never reach it. It is our own
        // unreaped child, so the pid cannot have been reused.
        if let Ok(pid) = libc::pid_t::try_from(pid) {
            // SAFETY: kill has no memory-safety preconditions.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        return Err(e.into());
    }
    Ok((pid, record_root(rs, home, root)))
}

/// `start`: prototype check, recovery under the lock, then `watch` as a
/// detached process. Returns at once.
pub fn start(store: &ProfileStore, home: &Path, s: &Settings) -> Result<(), ProfileError> {
    let rs = RotateState::new(store.root());
    prototype_guard(store.root())?;
    rs.ensure()?;
    let state = State::new(store.root())?;
    // The lock is held until the pid file is written, so two `start`s
    // cannot both find no rotator and spawn one each.
    let Some(_lock) = state.try_lock(LOCK_WAIT)? else {
        return Err(daemon_err("lock busy"));
    };
    recover_auto_switch(&state)?;
    match swap::recover(store, &rs, home)? {
        RecoverOutcome::Inconsistent(r) => return Err(ProfileError::RotateInconsistent(r)),
        RecoverOutcome::RolledBack { from, to } => {
            println!("{}", msg::recovered(false, &from, &to))
        }
        RecoverOutcome::RolledForward { from, to } => {
            println!("{}", msg::recovered(true, &from, &to))
        }
        RecoverOutcome::None => {}
    }
    if paused(store.root()) {
        eprintln!(
            "warning: auto-switch is off, so the rotator stays idle; turn it on with `amplihack profile auto-switch on`"
        );
    }
    match current_state(&rs) {
        DaemonState::Running(pid) => {
            return Err(daemon_err(format!(
                "a rotator is already running (pid {pid})"
            )));
        }
        DaemonState::Unverifiable => {
            return Err(daemon_err("cannot verify a running rotator on this host"));
        }
        DaemonState::Stopped | DaemonState::Stale => {}
    }
    let (pid, warnings) = launch_core(&rs, store.root(), home, s, &spawn)?;
    println!(
        "rotate: started (pid {pid}), checking every {}s",
        s.interval
    );
    println!(
        "log: {}",
        sanitize_display(&rs.log.display().to_string(), false)
    );
    for e in warnings {
        eprintln!("warning: could not record the rotator's root: {e}");
    }
    println!(
        "root: {}",
        sanitize_display(&store.root().display().to_string(), false)
    );
    Ok(())
}

/// Records the root a rotator started with, next to its pid file and in
/// `~/.amplihack/rotate-root`, so `status` and `stop` find it from a shell
/// whose root resolves elsewhere. The rotator runs either way; the failures
/// are returned for the caller to report.
fn record_root(rs: &RotateState, home: &Path, root: &Path) -> Vec<ProfileError> {
    [rs.dir.join("root"), config_dir(home).join(ROTATE_ROOT_FILE)]
        .iter()
        .filter_map(|file| save(file, root).err())
        .collect()
}

/// What starts the background process: [`spawn`], or a fake in tests.
pub(super) type Spawner = dyn Fn(&RotateState, &Settings, &Path) -> Result<u32, ProfileError>;

/// Starts `<this exe> profile rotate watch <flags> --spawned` in a new
/// session with SIGHUP ignored, stdin from `/dev/null` and its stdout and
/// stderr appended to `rotate.err`, so the log keeps its grammar. No shell;
/// the child gets no `AMPLIHACK_ROTATE*` variable and no profile selection
/// (`CLAUDE_CONFIG_DIR`, `AMPLIHACK_CLAUDE_PROFILE`) from the launch that
/// started it, and is pinned to `root` whatever the variable or the saved
/// root say later (issue #1592).
pub(super) fn spawn(rs: &RotateState, s: &Settings, root: &Path) -> Result<u32, ProfileError> {
    use std::os::unix::process::CommandExt;
    let errors = super::log::RotateLog::open(&rs.err, &rs.err)?;
    let (out, err) = (errors.file()?, errors.file()?);
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.args(["profile", "rotate", "watch"])
        .args(s.to_args(true))
        .arg("--spawned")
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("AMPLIHACK_ROTATE") {
            cmd.env_remove(k);
        }
    }
    cmd.env_remove("CLAUDE_CONFIG_DIR").env_remove(PROFILE_ENV);
    cmd.env(ROOT_ENV, root);
    // SAFETY: the closure runs in the child between fork and exec and calls
    // only setsid and signal, which are async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    Ok(cmd.spawn()?.id())
}

/// `watch`: cycles every `interval` seconds until SIGTERM or SIGINT. Never
/// acts on a signal while holding the lock.
pub fn watch(
    store: &ProfileStore,
    home: PathBuf,
    s: Settings,
    spawned: bool,
) -> Result<(), ProfileError> {
    let rs = RotateState::new(store.root());
    let stop = PROBE_CANCEL.clone();
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, stop.clone())?;
    }
    if !spawned {
        rs.ensure()?;
        // As in `start`: checked and written under the lock.
        let state = State::new(store.root())?;
        let Some(_lock) = state.try_lock(LOCK_WAIT)? else {
            return Err(daemon_err("lock busy"));
        };
        if let DaemonState::Running(pid) = current_state(&rs) {
            return Err(daemon_err(format!(
                "a rotator is already running (pid {pid})"
            )));
        }
        write_private_atomic(
            &rs.dir,
            "rotate.pid",
            format!("{}\n", std::process::id()).as_bytes(),
        )?;
    }
    let mut ctx = Ctx::new(store.clone(), home, s);
    ctx.stop = Some(stop.clone());
    let mut idle = false;
    while !stop.load(Ordering::Relaxed) {
        if !paused(store.root()) {
            idle = false;
            match cycle::run_cycle(&mut ctx, Mode::Watch) {
                Ok(CycleOutcome::Stopped) => break,
                Ok(_) => {}
                Err(e) => log_line(&rs, &failure(&e)),
            }
        } else if !idle {
            idle = true;
            log_line(&rs, msg::IDLE);
            record_idle(&rs, store);
        }
        let until = Instant::now() + Duration::from_secs(s.interval);
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1).min(until - Instant::now()));
        }
    }
    remove_own_pid(&rs);
    Ok(())
}

/// Whether auto-switch has been turned off (by its settings file or
/// `AMPLIHACK_CLAUDE_AUTO_SWITCH=0`), or its settings file is damaged.
/// Without any setting, rotation runs.
fn paused(root: &Path) -> bool {
    let eff = effective(root);
    !eff.on && eff.source != Source::Default
}

/// The log line for a cycle that failed: a refused swap is an ALERT.
fn failure(e: &ProfileError) -> String {
    match e {
        ProfileError::RotateRefused(_) => format!("ALERT: {e}"),
        ProfileError::RotatePrototypeRunning => "skip: the external rotator is running".into(),
        _ => format!("skip: {e}"),
    }
}

fn log_line(rs: &RotateState, line: &str) {
    match rs.ensure() {
        Ok(()) => Sink::new(open_or_warn(&rs.log, &rs.log1), false, true).emit(now(), line),
        Err(e) => eprintln!("warning: rotate log unavailable ({e}): {line}"),
    }
}

/// Records an idle cycle, if the lock is free.
fn record_idle(rs: &RotateState, store: &ProfileStore) {
    let Ok(state) = State::new(store.root()) else {
        return;
    };
    if let Ok(Some(_lock)) = state.try_lock(LOCK_WAIT)
        && let Ok(l) = rs.load_ledger()
    {
        let mut l = l.unwrap_or_default();
        l.last_cycle = Some(LastCycle {
            at: now(),
            outcome: "idle".into(),
            overflow: None,
        });
        if let Err(e) = rs.save_ledger(&l) {
            log_line(rs, &format!("skip: could not record the idle cycle: {e}"));
        }
    }
}

/// Removes the pid file only if it still holds this process's pid.
fn remove_own_pid(rs: &RotateState) {
    let own = format!("{}\n", std::process::id());
    if matches!(read_bounded_nofollow(&rs.pid, MAX_PID_FILE as u64), Ok(Some((raw, _))) if raw == own.as_bytes())
        && let Err(e) = std::fs::remove_file(&rs.pid)
    {
        eprintln!("warning: could not remove {} ({e})", rs.pid.display());
    }
}
