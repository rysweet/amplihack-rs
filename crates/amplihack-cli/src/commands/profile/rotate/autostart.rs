//! Rotator autostart: `amplihack claude`, `launch` and `profile run` start
//! the background rotator when auto-switch is on and none is running, and
//! the `rotate autostart [on|off]` setting that turns it off. See "Rotate
//! autostart" in docs/reference/profile-command.md for the steps in `run`.
//!
//! A launch is never failed, slowed by a wait or given stdout output by
//! this: every refusal returns silently, the state lock is tried once
//! without waiting, and failures go to `rotate.err` only.

use super::TuneFlags;
use super::change_log;
use super::daemon::{self, DaemonState, Spawner, launch_core, start_refusal, state_at};
use super::ledger::RotateState;
use super::log::RotateLog;
use super::owner;
use super::settings;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    State, check_root_private, euid, now, read_bounded_nofollow, write_private_atomic,
};
use crate::commands::profile::store::ProfileStore;
use crate::user_config::TEST_HOME_GUARD_ENV;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

pub const ENV: &str = "AMPLIHACK_ROTATE_AUTOSTART";
/// Set by `amplihack recipe run` for its steps: a launch inside a recipe run
/// never starts the rotator (issue #1803).
pub(super) const RECIPE_RUN_ENV: &str = "AMPLIHACK_RECIPE_RUN_ID";
/// The setting's file name in `<root>/.auto-switch.d/rotate/`.
const FILE: &str = "autostart";
const MAX_SETTING: u64 = 64;

/// Where the effective autostart state comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    Default,
    Setting,
    Env,
    Corrupt,
}

impl Src {
    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Setting => "setting",
            Self::Env => "env",
            Self::Corrupt => "corrupt",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Autostart {
    pub on: bool,
    pub src: Src,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvValue {
    Unset,
    On,
    Off,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileValue {
    Missing,
    On,
    Off,
    Corrupt,
}

/// `0`/`off` and `1`/`on`, case-insensitive; empty is unset.
pub fn parse_env(raw: Option<&OsStr>) -> EnvValue {
    let Some(raw) = raw.filter(|v| !v.is_empty()) else {
        return EnvValue::Unset;
    };
    match raw.to_str().map(str::to_ascii_lowercase).as_deref() {
        Some("1" | "on") => EnvValue::On,
        Some("0" | "off") => EnvValue::Off,
        _ => EnvValue::Invalid,
    }
}

/// The setting file, read without following a symlink and only up to 64
/// bytes. Anything but a file of yours holding `on` or `off` and an
/// optional newline is `Corrupt`.
pub fn read_setting(root: &Path) -> FileValue {
    let path = RotateState::new(root).dir.join(FILE);
    match read_bounded_nofollow(&path, MAX_SETTING) {
        Ok(None) => FileValue::Missing,
        Ok(Some((raw, meta))) if meta.uid() == euid() => match raw.as_slice() {
            b"on" | b"on\n" => FileValue::On,
            b"off" | b"off\n" => FileValue::Off,
            _ => FileValue::Corrupt,
        },
        _ => FileValue::Corrupt,
    }
}

/// The variable wins; an invalid one is ignored. A missing file means on,
/// a corrupt one off.
pub fn effective_from(env: EnvValue, file: FileValue) -> Autostart {
    let (on, src) = match (env, file) {
        (EnvValue::On, _) => (true, Src::Env),
        (EnvValue::Off, _) => (false, Src::Env),
        (_, FileValue::Missing) => (true, Src::Default),
        (_, FileValue::On) => (true, Src::Setting),
        (_, FileValue::Off) => (false, Src::Setting),
        (_, FileValue::Corrupt) => (false, Src::Corrupt),
    };
    Autostart { on, src }
}

/// The effective state for `root`. An invalid variable warns once per
/// process, without repeating its value.
pub fn effective(root: &Path, env: &dyn Fn(&str) -> Option<OsString>) -> Autostart {
    let value = parse_env(env(ENV).as_deref());
    if value == EnvValue::Invalid {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            eprintln!("warning: ignored {ENV} value (use on, off, 1 or 0); the setting decides");
        });
    }
    effective_from(value, read_setting(root))
}

pub fn status_line(a: Autostart) -> String {
    let word = if a.on { "on" } else { "off" };
    format!("autostart: {word} ({})", a.src.as_str())
}

/// The change log's name for a setting file value.
fn label(v: FileValue) -> &'static str {
    match v {
        FileValue::Missing => "unset",
        FileValue::On => "on",
        FileValue::Off => "off",
        FileValue::Corrupt => "corrupt",
    }
}

/// Writes the setting, private and atomically, after checking the root and
/// creating the state directory. A write that changes the value appends a
/// `change:` line to the rotate log (issue #1803).
pub fn set(store: &ProfileStore, on: bool) -> Result<(), ProfileError> {
    let rs = RotateState::new(store.root());
    rs.ensure()?;
    // Read without a lock: two `rotate autostart` commands at the same moment
    // can log the wrong old value. The change log is best effort, and the
    // setting itself is written atomically.
    let old = read_setting(store.root());
    let body: &[u8] = if on { b"on\n" } else { b"off\n" };
    write_private_atomic(&rs.dir, FILE, body).map_err(ProfileError::Io)?;
    let new = if on { FileValue::On } else { FileValue::Off };
    if old != new {
        change_log::record(store.root(), "rotate-autostart", label(old), label(new));
    }
    Ok(())
}

/// `rotate autostart [on|off]`: prints the state, or writes the setting and
/// prints it, warning when the variable overrides it here.
pub fn cli(store: &ProfileStore, value: Option<&str>) -> Result<(), ProfileError> {
    let env = |k: &str| std::env::var_os(k);
    let on = match value {
        None => {
            println!("{}", status_line(effective(store.root(), &env)));
            return Ok(());
        }
        Some("on") => true,
        Some("off") => false,
        Some(_) => {
            return Err(ProfileError::RotateConfig(
                "autostart takes on or off".into(),
            ));
        }
    };
    set(store, on)?;
    println!(
        "{}",
        status_line(Autostart {
            on,
            src: Src::Setting
        })
    );
    let raw = env(ENV);
    if matches!(parse_env(raw.as_deref()), EnvValue::On | EnvValue::Off)
        && let Some(raw) = raw
    {
        eprintln!(
            "warning: {ENV}={} overrides it in this environment",
            raw.to_string_lossy()
        );
    }
    Ok(())
}

/// What `run` reads and starts, injected so tests never touch the real
/// environment, `/proc` or `spawn()`.
pub struct Deps<'a> {
    pub env: &'a dyn Fn(&str) -> Option<OsString>,
    pub auto_switch_on: &'a dyn Fn(&Path) -> bool,
    pub docker_env: &'a dyn Fn() -> bool,
    pub proc_root: &'a Path,
    pub exe_name: &'a OsStr,
    pub spawn: &'a Spawner,
    pub home: &'a Path,
    pub euid: u32,
    /// The launch's stderr: the one `started` line goes here.
    pub err: &'a mut dyn Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Auto-switch or autostart is off.
    Off,
    /// A gate refused: `test`, `recipe-run`, `root`, `root-user`, `docker`,
    /// `unverifiable` or `refused`.
    Skipped(&'static str),
    /// A verified rotator is running.
    Running,
    /// A rotation owner marker exists (issue #1803); `running` is whether
    /// its owner is a live, verified rotator.
    Deferred {
        running: bool,
    },
    /// Another process holds the state lock.
    Busy,
    Started(u32),
    /// Logged to `rotate.err` when it can be written.
    Failed,
}

/// Appends one timed, sanitised line to `rotate.err`. Best effort: a launch
/// has nowhere else to report it, and must not fail or print because of it.
fn note(rs: &RotateState, msg: &str) {
    if let Ok(mut log) = RotateLog::open(&rs.err, &rs.err) {
        let _ = log.write(now(), msg);
    }
}

fn failed(rs: &RotateState, e: &ProfileError) -> Outcome {
    note(rs, &format!("autostart: failed: {e}"));
    Outcome::Failed
}

/// The autostart steps, in the documented order. Steps 0 to 8 create
/// nothing; 9 to 12 run under the state lock, tried once without waiting.
///
/// Step 0 (#1690): a process a test started (`AMPLIHACK_TEST_HOME_GUARD`
/// non-empty) never autostarts a rotator. It returns `Skipped("test")` before
/// any gate, so it reads, creates and prints nothing; a daemon started from
/// a test's temp `HOME` would outlive the test. A guard value that is not
/// valid UTF-8 still counts, since only emptiness is checked.
///
/// Step 0b and the owner gate (#1803): inside a recipe run nothing is read;
/// while an owner marker exists the launch defers to it, before the
/// autostart setting, so an owner wins over `autostart on`.
pub fn run(root: &Path, docker_flag: bool, d: Deps<'_>) -> Outcome {
    if (d.env)(TEST_HOME_GUARD_ENV).is_some_and(|v| !v.is_empty()) {
        return Outcome::Skipped("test");
    }
    if (d.env)(RECIPE_RUN_ENV).is_some_and(|v| !v.is_empty()) {
        return Outcome::Skipped("recipe-run");
    }
    if !(d.auto_switch_on)(root) {
        return Outcome::Off;
    }
    // Before anything under the root is read.
    if check_root_private(root).is_err() {
        return Outcome::Skipped("root");
    }
    if d.euid == 0 {
        return Outcome::Skipped("root-user");
    }
    // The owner wins over the setting, which only shapes the owner warning.
    let autostart_on = effective(root, d.env).on;
    if let Some(running) = owner::defer(root, d.proc_root, d.exe_name, autostart_on, d.err) {
        return Outcome::Deferred { running };
    }
    if !autostart_on {
        return Outcome::Off;
    }
    if docker_flag || (d.docker_env)() {
        return Outcome::Skipped("docker");
    }
    let rs = RotateState::new(root);
    // A running or unverifiable rotator ends it, before the lock and again
    // under it.
    let blocked = || match state_at(&rs, d.proc_root, d.exe_name) {
        DaemonState::Running(_) => Some(Outcome::Running),
        DaemonState::Unverifiable => Some(Outcome::Skipped("unverifiable")),
        DaemonState::Stopped | DaemonState::Stale => None,
    };
    if let Some(o) = blocked() {
        return o;
    }
    if start_refusal(root).is_some() {
        return Outcome::Skipped("refused");
    }
    if let Err(e) = rs.ensure() {
        return failed(&rs, &e);
    }
    let lock = match State::new(root).and_then(|s| s.try_lock(Duration::ZERO)) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Outcome::Busy,
        Err(e) => return failed(&rs, &e),
    };
    if let Some(o) = blocked() {
        return o;
    }
    let s = match settings::resolve(&TuneFlags::default(), None, d.env) {
        Ok(s) => s,
        Err(e) => return failed(&rs, &e),
    };
    let (pid, warnings) = match launch_core(&rs, root, d.home, &s, d.spawn) {
        Ok(started) => started,
        Err(e) => return failed(&rs, &e),
    };
    drop(lock);
    let exe = std::env::current_exe().map_or_else(|_| "-".into(), |p| p.display().to_string());
    note(&rs, &format!("autostart: started pid {pid} exe {exe}"));
    for e in warnings {
        note(
            &rs,
            &format!("autostart: warning: could not record the rotator's root: {e}"),
        );
    }
    // The launch goes on whether or not its stderr takes the line.
    let _ = writeln!(
        d.err,
        "amplihack: started Claude login rotator (pid {pid}); 'amplihack profile rotate autostart off' to disable"
    );
    Outcome::Started(pid)
}

/// The refusing spawner's fixed error text. Tests assert on it so they know
/// `run` reached the spawner rather than failing at an earlier step.
#[cfg(test)]
pub(super) const REFUSED_SPAWN: &str = "a unit test never starts a real rotator";

/// Unit tests get this in place of `daemon::spawn` (#1690), so an in-crate
/// test can never start a real rotator, whatever its environment holds.
#[cfg(test)]
fn refuse_spawn(
    _: &RotateState,
    _: &super::settings::Settings,
    _: &Path,
) -> Result<u32, ProfileError> {
    Err(ProfileError::RotateConfig(REFUSED_SPAWN.into()))
}

/// The launch hook: runs `run` with this process's environment, `/proc`
/// and the real spawner (a refusing one in unit tests). Never fails and
/// never writes to stdout.
pub fn autostart_on_launch(docker_flag: bool) {
    use crate::commands::profile::root_config::{home_from_env, store_from_env};
    let (Ok((store, _)), Some(home), Some(exe_name)) =
        (store_from_env(), home_from_env(), daemon::exe_name())
    else {
        return;
    };
    let env = |k: &str| std::env::var_os(k);
    let auto_switch_on = |r: &Path| crate::commands::profile::auto_switch::kit::effective(r).on;
    let docker_env = crate::commands::profile::docker_env_active;
    let mut err = std::io::stderr();
    #[cfg(not(test))]
    let spawn: &Spawner = &daemon::spawn;
    #[cfg(test)]
    let spawn: &Spawner = &refuse_spawn;
    run(
        store.root(),
        docker_flag,
        Deps {
            env: &env,
            auto_switch_on: &auto_switch_on,
            docker_env: &docker_env,
            proc_root: Path::new("/proc"),
            exe_name: &exe_name,
            spawn,
            home: &home,
            euid: euid(),
            err: &mut err,
        },
    );
}

/// The `autostart:` status line for `root`, for `rotate status` and
/// `auto-switch status`, with the owner deferral when there is one.
pub(in crate::commands::profile) fn autostart_line(root: &std::path::Path) -> String {
    let line = status_line(effective(root, &|k| std::env::var_os(k)));
    format!("{line}{}", owner::deferral_suffix(root))
}
