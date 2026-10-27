//! `rotate::autostart`: launches start the rotator when auto-switch is on.
//!
//! Every test uses a temporary HOME and a temporary root with mode 0700, a
//! fake `/proc` and a fake spawner. Nothing here calls the real `spawn()`:
//! the fake writes `/proc/<pid>/cmdline` so `state_at` sees a rotator, and
//! its pids are above Linux's `PID_MAX_LIMIT` (4194304), so the `SIGTERM`
//! sent when a pid file cannot be written can never reach a real process.
//!
//! Contract:
//! - `autostart::ENV` is `AMPLIHACK_ROTATE_AUTOSTART`; the setting is
//!   `<root>/.auto-switch.d/rotate/autostart`.
//! - `parse_env(Option<&OsStr>) -> EnvValue::{Unset, On, Off, Invalid}`;
//!   `0`/`off`/`1`/`on`, case-insensitive; empty is unset.
//! - `read_setting(root: &Path) -> FileValue::{Missing, On, Off, Corrupt}`.
//! - `effective_from(EnvValue, FileValue) -> Autostart { on, src }`, with
//!   `src` one of `Src::{Default, Setting, Env, Corrupt}`.
//! - `effective(root, env: &dyn Fn(&str) -> Option<OsString>) -> Autostart`.
//! - `status_line(Autostart) -> String`: `autostart: on (default)`.
//! - `set(&ProfileStore, on: bool) -> Result<(), ProfileError>` and
//!   `cli(&ProfileStore, Option<&str>) -> Result<(), ProfileError>`.
//! - `run(root, docker_flag, Deps) -> Outcome::{Off, Skipped(&str), Running,
//!   Busy, Started(u32), Failed}`, with the gates in the order of "Rotate
//!   autostart" in docs/reference/profile-command.md.
//! - `autostart_on_launch(docker_flag: bool)` returns `()`.

use super::autostart::{
    self, Autostart, Deps, ENV, EnvValue, FileValue, Outcome, Src, effective, effective_from,
    parse_env, read_setting, set, status_line,
};
use super::cycle::prototype_guard;
use super::ledger::RotateState;
use super::settings::Settings;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, euid};
use crate::commands::profile::root_config::{ROTATE_ROOT_FILE, config_dir};
use crate::commands::profile::store::{ProfileStore, ROOT_ENV};
use crate::test_support::{EnvGuard, env_lock};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

const EXE: &str = "amplihack-autostart-test";
/// Above `PID_MAX_LIMIT`: never a real process.
const FAKE_PID: u32 = 4_200_000;
const STARTED: &str = "amplihack: started Claude login rotator (pid 4200000); 'amplihack profile rotate autostart off' to disable\n";

fn mode(path: &Path, m: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(m)).unwrap();
}

fn private_file(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    mode(path, 0o600);
}

fn argv(parts: &[&str]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in parts {
        v.extend_from_slice(p.as_bytes());
        v.push(0);
    }
    v
}

/// A fake rotator process: what `state_at` checks under `/proc`.
fn fake_process(proc_root: &Path, pid: u32) {
    let d = proc_root.join(pid.to_string());
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("cmdline"),
        argv(&[
            &format!("/usr/bin/{EXE}"),
            "profile",
            "rotate",
            "watch",
            "--spawned",
        ]),
    )
    .unwrap();
}

struct Fx {
    tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    proc_root: PathBuf,
    rs: RotateState,
    spawns: Arc<AtomicU32>,
    seen: Arc<Mutex<Vec<(Settings, PathBuf)>>>,
}

/// Inputs a test changes; the rest of `Deps` comes from the fixture.
struct Opts {
    env: Vec<(&'static str, &'static str)>,
    docker_flag: bool,
    docker_env: bool,
    auto_switch: Option<bool>,
    euid: u32,
    proc_root: Option<PathBuf>,
    spawn_err: Option<&'static str>,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            env: Vec::new(),
            docker_flag: false,
            docker_env: false,
            auto_switch: None,
            euid: euid().max(1),
            proc_root: None,
            spawn_err: None,
        }
    }
}

/// Auto-switch on, autostart unset, no rotator.
fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    mode(tmp.path(), 0o700);
    let root = tmp.path().join("root");
    let home = tmp.path().join("home");
    let proc_root = tmp.path().join("proc");
    for d in [&root, &home, &proc_root] {
        fs::create_dir(d).unwrap();
        mode(d, 0o700);
    }
    let root = root.canonicalize().unwrap();
    private_file(&root.join(".auto-switch"), b"enabled=on\n");
    Fx {
        rs: RotateState::new(&root),
        tmp,
        root,
        home,
        proc_root,
        spawns: Arc::new(AtomicU32::new(0)),
        seen: Arc::new(Mutex::new(Vec::new())),
    }
}

impl Fx {
    fn setting(&self, body: &[u8]) {
        private_file(&self.rs.dir.join("autostart"), body);
        mode(&self.rs.dir, 0o700);
        mode(self.rs.dir.parent().unwrap(), 0o700);
    }

    fn running(&self, pid: u32) {
        self.rs.ensure().unwrap();
        private_file(&self.rs.pid, format!("{pid}\n").as_bytes());
        fake_process(&self.proc_root, pid);
    }

    fn spawned(&self) -> u32 {
        self.spawns.load(Ordering::SeqCst)
    }

    fn rotate_err(&self) -> String {
        fs::read_to_string(&self.rs.err).unwrap_or_default()
    }

    fn state_dir_exists(&self) -> bool {
        self.root.join(".auto-switch.d").exists()
    }

    fn run(&self) -> (Outcome, String) {
        self.run_with(Opts::default())
    }

    fn run_with(&self, o: Opts) -> (Outcome, String) {
        let proc_root = o
            .proc_root
            .clone()
            .unwrap_or_else(|| self.proc_root.clone());
        run_at(
            &self.root,
            &self.home,
            &proc_root,
            &self.proc_root,
            self.spawns.clone(),
            self.seen.clone(),
            o,
        )
    }
}

/// One `autostart::run` with injected dependencies. `fake_proc` is where the
/// fake spawner records its process; `proc_root` is what `run` checks.
fn run_at(
    root: &Path,
    home: &Path,
    proc_root: &Path,
    fake_proc: &Path,
    spawns: Arc<AtomicU32>,
    seen: Arc<Mutex<Vec<(Settings, PathBuf)>>>,
    o: Opts,
) -> (Outcome, String) {
    let vars = o.env.clone();
    let env = move |k: &str| -> Option<OsString> {
        vars.iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| OsString::from(v))
    };
    let auto_switch = o.auto_switch;
    let auto_switch_on = move |r: &Path| -> bool {
        auto_switch.unwrap_or_else(|| {
            fs::read_to_string(r.join(".auto-switch")).is_ok_and(|t| t.contains("enabled=on"))
        })
    };
    let docker = o.docker_env;
    let docker_env = move || docker;
    let spawn_err = o.spawn_err;
    let fake_proc = fake_proc.to_path_buf();
    let spawn = move |_rs: &RotateState, s: &Settings, r: &Path| -> Result<u32, ProfileError> {
        let n = spawns.fetch_add(1, Ordering::SeqCst);
        seen.lock().unwrap().push((*s, r.to_path_buf()));
        if let Some(e) = spawn_err {
            return Err(ProfileError::RotateDaemon(e.into()));
        }
        let pid = FAKE_PID + n;
        fake_process(&fake_proc, pid);
        Ok(pid)
    };
    let mut err: Vec<u8> = Vec::new();
    let outcome = autostart::run(
        root,
        o.docker_flag,
        Deps {
            env: &env,
            auto_switch_on: &auto_switch_on,
            docker_env: &docker_env,
            proc_root,
            exe_name: OsStr::new(EXE),
            spawn: &spawn,
            home,
            euid: o.euid,
            err: &mut err,
        },
    );
    (outcome, String::from_utf8(err).unwrap())
}

// --- Starting ------------------------------------------------------------

#[test]
fn nothing_running_starts_one_rotator_and_prints_one_line() {
    let f = fx();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Started(FAKE_PID));
    assert_eq!(f.spawned(), 1);
    assert_eq!(err, STARTED, "exactly one stderr line");
    assert_eq!(
        fs::read_to_string(&f.rs.pid).unwrap(),
        format!("{FAKE_PID}\n")
    );
    let pid_mode = fs::metadata(&f.rs.pid).unwrap().permissions().mode() & 0o777;
    assert_eq!(pid_mode, 0o600);
    let log = f.rotate_err();
    assert_eq!(log.lines().count(), 1, "{log:?}");
    assert!(
        log.contains(&format!("autostart: started pid {FAKE_PID} exe ")),
        "{log:?}"
    );
}

#[test]
fn a_started_rotator_records_its_root_and_gets_the_root_and_settings() {
    let f = fx();
    let (o, _) = f.run_with(Opts {
        env: vec![("AMPLIHACK_ROTATE_INTERVAL", "60")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Started(FAKE_PID));
    assert!(
        f.rs.dir.join("root").exists(),
        "root record next to the pid"
    );
    assert!(config_dir(&f.home).join(ROTATE_ROOT_FILE).exists());
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0.interval, 60, "the launch's tuning variables");
    assert_eq!(seen[0].0.hold, 3600);
    assert_eq!(seen[0].1, f.root);
}

#[test]
fn a_second_launch_finds_the_rotator_running_and_says_nothing() {
    let f = fx();
    assert_eq!(f.run().0, Outcome::Started(FAKE_PID));
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Running);
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 1);
}

#[test]
fn a_live_verified_rotator_is_left_alone() {
    let f = fx();
    f.running(48213);
    let before = fs::read(&f.rs.pid).unwrap();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Running);
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
    assert_eq!(fs::read(&f.rs.pid).unwrap(), before);
    assert_eq!(f.rotate_err(), "");
}

#[test]
fn a_stale_pid_file_is_replaced() {
    let f = fx();
    f.rs.ensure().unwrap();
    private_file(&f.rs.pid, b"4199999\n");
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Started(FAKE_PID));
    assert_eq!(err, STARTED);
}

#[test]
fn sixteen_concurrent_launches_start_exactly_one_rotator() {
    let f = fx();
    let barrier = Arc::new(Barrier::new(16));
    let outcomes: Vec<(Outcome, String)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let barrier = barrier.clone();
                let f = &f;
                s.spawn(move || {
                    barrier.wait();
                    f.run()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(f.spawned(), 1, "{outcomes:?}");
    let started: Vec<_> = outcomes
        .iter()
        .filter(|(o, _)| matches!(o, Outcome::Started(_)))
        .collect();
    assert_eq!(started.len(), 1, "{outcomes:?}");
    assert_eq!(started[0].1, STARTED);
    for (o, err) in &outcomes {
        if !matches!(o, Outcome::Started(_)) {
            assert!(matches!(o, Outcome::Busy | Outcome::Running), "{o:?}");
            assert_eq!(err, "", "the others return silently");
        }
    }
}

// --- Gates, in order -------------------------------------------------------

#[test]
fn auto_switch_off_does_nothing_and_creates_nothing() {
    let f = fx();
    fs::remove_file(f.root.join(".auto-switch")).unwrap();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Off);
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
    assert!(!f.state_dir_exists(), "no .auto-switch.d");
}

#[test]
fn auto_switch_off_comes_before_every_other_gate() {
    let f = fx();
    mode(&f.root, 0o757);
    let (o, err) = f.run_with(Opts {
        auto_switch: Some(false),
        docker_flag: true,
        euid: 0,
        ..Opts::default()
    });
    mode(&f.root, 0o700);
    assert_eq!(o, Outcome::Off);
    assert_eq!(err, "");
    assert!(!f.state_dir_exists());
}

#[test]
fn a_root_writable_by_others_is_skipped_without_reading_the_setting() {
    let f = fx();
    f.setting(b"off\n");
    mode(&f.root, 0o757);
    let (o, err) = f.run();
    mode(&f.root, 0o700);
    assert_eq!(o, Outcome::Skipped("root"), "step 2 comes before step 4");
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
    assert!(!f.root.join(".auto-switch.d").join("lock").exists());
}

#[test]
fn a_missing_root_is_skipped() {
    let f = fx();
    let gone = f.tmp.path().join("no-such-root");
    let (o, err) = run_at(
        &gone,
        &f.home,
        &f.proc_root,
        &f.proc_root,
        f.spawns.clone(),
        f.seen.clone(),
        Opts {
            auto_switch: Some(true),
            ..Opts::default()
        },
    );
    assert_eq!(o, Outcome::Skipped("root"));
    assert_eq!(err, "");
    assert!(!gone.exists(), "nothing created");
}

#[test]
fn a_symlinked_root_is_skipped() {
    let f = fx();
    let link = f.tmp.path().join("link");
    std::os::unix::fs::symlink(&f.root, &link).unwrap();
    let (o, _) = run_at(
        &link,
        &f.home,
        &f.proc_root,
        &f.proc_root,
        f.spawns.clone(),
        f.seen.clone(),
        Opts::default(),
    );
    assert_eq!(o, Outcome::Skipped("root"));
    assert_eq!(f.spawned(), 0);
}

#[test]
fn root_never_autostarts() {
    let f = fx();
    f.setting(b"off\n");
    let (o, err) = f.run_with(Opts {
        euid: 0,
        ..Opts::default()
    });
    assert_eq!(
        o,
        Outcome::Skipped("root-user"),
        "step 3 comes before step 4"
    );
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
}

#[test]
fn the_autostart_setting_off_does_nothing() {
    let f = fx();
    f.setting(b"off\n");
    let (o, err) = f.run_with(Opts {
        docker_flag: true,
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Off, "step 4 comes before step 5");
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
    assert!(!f.root.join(".auto-switch.d").join("lock").exists());
}

#[test]
fn the_env_var_turns_autostart_off() {
    for v in ["0", "off", "OFF", "Off"] {
        let f = fx();
        let (o, err) = f.run_with(Opts {
            env: vec![(ENV, v)],
            ..Opts::default()
        });
        assert_eq!(o, Outcome::Off, "{ENV}={v}");
        assert_eq!(err, "");
        assert_eq!(f.spawned(), 0);
        assert!(!f.state_dir_exists(), "{ENV}={v}");
    }
}

#[test]
fn the_env_var_wins_over_the_setting() {
    let f = fx();
    f.setting(b"off\n");
    let (o, _) = f.run_with(Opts {
        env: vec![(ENV, "on")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Started(FAKE_PID));
}

#[test]
fn an_invalid_env_var_falls_back_to_the_setting() {
    let f = fx();
    f.setting(b"off\n");
    let (o, _) = f.run_with(Opts {
        env: vec![(ENV, "maybe")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Off);
    let g = fx();
    let (o, _) = g.run_with(Opts {
        env: vec![(ENV, "maybe")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Started(FAKE_PID), "missing setting means on");
}

#[test]
fn a_corrupt_setting_counts_as_off() {
    let f = fx();
    f.setting(b"yes\n");
    let (o, _) = f.run();
    assert_eq!(o, Outcome::Off);
    assert_eq!(f.spawned(), 0);
}

#[test]
fn docker_skips_autostart() {
    for (flag, env) in [(true, false), (false, true)] {
        let f = fx();
        let (o, err) = f.run_with(Opts {
            docker_flag: flag,
            docker_env: env,
            ..Opts::default()
        });
        assert_eq!(o, Outcome::Skipped("docker"), "flag={flag} env={env}");
        assert_eq!(err, "");
        assert_eq!(f.spawned(), 0);
        assert!(!f.state_dir_exists());
    }
}

#[test]
fn docker_is_checked_before_the_daemon_state() {
    let f = fx();
    f.running(48213);
    let (o, _) = f.run_with(Opts {
        docker_flag: true,
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Skipped("docker"));
}

#[test]
fn an_unverifiable_rotator_is_skipped() {
    let f = fx();
    f.rs.ensure().unwrap();
    private_file(&f.rs.pid, b"48213\n");
    let (o, err) = f.run_with(Opts {
        proc_root: Some(f.tmp.path().join("no-proc")),
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Skipped("unverifiable"));
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
}

#[test]
fn a_held_prototype_lock_is_refused_as_start_refuses_it() {
    let f = fx();
    let dir = f.root.join(".rotator");
    fs::create_dir(&dir).unwrap();
    let holder = fs::File::create(dir.join("lock")).unwrap();
    // SAFETY: flock on a descriptor this test owns.
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(prototype_guard(&f.root).is_err(), "start refuses too");
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Skipped("refused"));
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
    assert!(!f.state_dir_exists(), "steps 1-7 create nothing");
}

#[test]
fn a_prototype_lock_that_is_not_a_file_is_refused() {
    let f = fx();
    fs::create_dir_all(f.root.join(".rotator").join("lock")).unwrap();
    let (o, _) = f.run();
    assert_eq!(o, Outcome::Skipped("refused"));
    assert_eq!(f.spawned(), 0);
}

#[test]
fn a_busy_state_lock_returns_at_once() {
    let f = fx();
    let state = State::new(&f.root).unwrap();
    let _held = state.try_lock(Duration::ZERO).unwrap().unwrap();
    let t = std::time::Instant::now();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Busy);
    assert!(t.elapsed() < Duration::from_millis(500), "zero-wait lock");
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
}

// --- Failures: never printed, never propagated ------------------------------

#[test]
fn invalid_tuning_variables_fail_without_spawning() {
    let f = fx();
    let (o, err) = f.run_with(Opts {
        env: vec![("AMPLIHACK_ROTATE_HOLD", "abc")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Failed);
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0, "no silent fallback to the defaults");
    let log = f.rotate_err();
    assert!(log.contains("autostart: failed: "), "{log:?}");
    assert!(log.contains("AMPLIHACK_ROTATE_HOLD"), "{log:?}");
    // The lock was released: the next launch with valid settings starts it.
    assert_eq!(f.run().0, Outcome::Started(FAKE_PID));
}

#[test]
fn a_spawner_error_fails_with_one_sanitised_line() {
    let f = fx();
    let (o, err) = f.run_with(Opts {
        spawn_err: Some("bad\npath\x1b[31m here"),
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Failed);
    assert_eq!(err, "", "nothing printed");
    assert_eq!(f.spawned(), 1);
    assert!(!f.rs.pid.exists());
    let log = f.rotate_err();
    assert_eq!(log.lines().count(), 1, "{log:?}");
    assert!(log.contains("autostart: failed: "), "{log:?}");
    assert!(!log.contains('\x1b'), "{log:?}");
    assert!(log.ends_with('\n'));
}

#[test]
fn an_unwritable_pid_file_fails() {
    let f = fx();
    f.rs.ensure().unwrap();
    fs::create_dir(&f.rs.pid).unwrap();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Failed);
    assert_eq!(err, "");
    assert!(f.rs.pid.is_dir(), "left as it was");
}

#[test]
fn a_read_only_state_dir_fails_quietly() {
    if euid() == 0 {
        return; // root ignores the mode bits
    }
    let f = fx();
    f.rs.ensure().unwrap();
    mode(&f.rs.dir, 0o500);
    let (o, err) = f.run();
    mode(&f.rs.dir, 0o700);
    assert_eq!(o, Outcome::Failed);
    assert_eq!(err, "");
    assert!(!f.rs.pid.exists());
}

#[test]
fn a_symlinked_state_lock_fails() {
    let f = fx();
    let dir = f.root.join(".auto-switch.d");
    fs::create_dir(&dir).unwrap();
    mode(&dir, 0o700);
    std::os::unix::fs::symlink("/dev/null", dir.join("lock")).unwrap();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Failed);
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
}

// --- The setting -------------------------------------------------------------

#[test]
fn env_values() {
    assert_eq!(parse_env(None), EnvValue::Unset);
    assert_eq!(parse_env(Some(OsStr::new(""))), EnvValue::Unset);
    for v in ["0", "off", "OFF", "oFf"] {
        assert_eq!(parse_env(Some(OsStr::new(v))), EnvValue::Off, "{v}");
    }
    for v in ["1", "on", "ON", "On"] {
        assert_eq!(parse_env(Some(OsStr::new(v))), EnvValue::On, "{v}");
    }
    for v in ["2", "yes", "false", " on", "on\n", "of"] {
        assert_eq!(parse_env(Some(OsStr::new(v))), EnvValue::Invalid, "{v:?}");
    }
}

#[test]
fn the_precedence_table() {
    use EnvValue as E;
    use FileValue as F;
    let a = |on, src| Autostart { on, src };
    for file in [F::Missing, F::On, F::Off, F::Corrupt] {
        assert_eq!(effective_from(E::On, file), a(true, Src::Env), "{file:?}");
        assert_eq!(effective_from(E::Off, file), a(false, Src::Env), "{file:?}");
    }
    for env in [E::Unset, E::Invalid] {
        assert_eq!(effective_from(env, F::Missing), a(true, Src::Default));
        assert_eq!(effective_from(env, F::On), a(true, Src::Setting));
        assert_eq!(effective_from(env, F::Off), a(false, Src::Setting));
        assert_eq!(effective_from(env, F::Corrupt), a(false, Src::Corrupt));
    }
}

#[test]
fn status_lines() {
    let a = |on, src| status_line(Autostart { on, src });
    assert_eq!(a(true, Src::Default), "autostart: on (default)");
    assert_eq!(a(true, Src::Setting), "autostart: on (setting)");
    assert_eq!(a(false, Src::Setting), "autostart: off (setting)");
    assert_eq!(a(true, Src::Env), "autostart: on (env)");
    assert_eq!(a(false, Src::Env), "autostart: off (env)");
    assert_eq!(a(false, Src::Corrupt), "autostart: off (corrupt)");
}

#[test]
fn effective_reads_the_env_then_the_setting() {
    let f = fx();
    let none = |_: &str| None;
    assert_eq!(
        status_line(effective(&f.root, &none)),
        "autostart: on (default)"
    );
    f.setting(b"off\n");
    assert_eq!(
        status_line(effective(&f.root, &none)),
        "autostart: off (setting)"
    );
    for v in ["0", "off"] {
        let env = move |k: &str| (k == ENV).then(|| OsString::from(v));
        assert_eq!(
            status_line(effective(&f.root, &env)),
            "autostart: off (env)",
            "{v}"
        );
    }
}

#[test]
fn setting_files() {
    let f = fx();
    assert_eq!(read_setting(&f.root), FileValue::Missing);
    for (body, want) in [
        (&b"on\n"[..], FileValue::On),
        (b"on", FileValue::On),
        (b"off\n", FileValue::Off),
        (b"off", FileValue::Off),
        (b"", FileValue::Corrupt),
        (b"yes\n", FileValue::Corrupt),
        (b"on\n\n", FileValue::Corrupt),
        (b" off\n", FileValue::Corrupt),
        (b"off\r\n", FileValue::Corrupt),
        (&[b'o'; 65][..], FileValue::Corrupt),
    ] {
        f.setting(body);
        assert_eq!(
            read_setting(&f.root),
            want,
            "{:?}",
            String::from_utf8_lossy(body)
        );
    }
}

#[test]
fn a_symlinked_setting_is_corrupt_and_not_followed() {
    let f = fx();
    f.rs.ensure().unwrap();
    let target = f.tmp.path().join("elsewhere");
    private_file(&target, b"on\n");
    std::os::unix::fs::symlink(&target, f.rs.dir.join("autostart")).unwrap();
    assert_eq!(read_setting(&f.root), FileValue::Corrupt);
    assert_eq!(f.run().0, Outcome::Off);
}

#[test]
fn a_setting_that_is_a_directory_is_corrupt() {
    let f = fx();
    f.rs.ensure().unwrap();
    fs::create_dir(f.rs.dir.join("autostart")).unwrap();
    assert_eq!(read_setting(&f.root), FileValue::Corrupt);
}

#[test]
fn a_setting_owned_by_another_user_is_corrupt() {
    if euid() != 0 {
        return; // only root can give a file away
    }
    let f = fx();
    f.setting(b"on\n");
    let path = f.rs.dir.join("autostart");
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: chown of a path this test created, with a valid C string.
    assert_eq!(unsafe { libc::chown(c.as_ptr(), 65534, 65534) }, 0);
    assert_eq!(read_setting(&f.root), FileValue::Corrupt);
}

#[test]
fn set_writes_a_private_file_and_round_trips() {
    let f = fx();
    let store = ProfileStore::new(f.root.clone());
    set(&store, false).unwrap();
    let path = f.rs.dir.join("autostart");
    assert_eq!(fs::read_to_string(&path).unwrap(), "off\n");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(read_setting(&f.root), FileValue::Off);
    set(&store, true).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "on\n");
    assert_eq!(read_setting(&f.root), FileValue::On);
}

#[test]
fn set_refuses_a_root_writable_by_others() {
    let f = fx();
    mode(&f.root, 0o757);
    let r = set(&ProfileStore::new(f.root.clone()), false);
    mode(&f.root, 0o700);
    assert!(r.is_err());
    assert!(!f.state_dir_exists());
}

#[test]
fn cli_without_a_value_changes_nothing() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::unset([ENV]);
    let f = fx();
    autostart::cli(&ProfileStore::new(f.root.clone()), None).unwrap();
    assert!(!f.state_dir_exists(), "read-only");
    autostart::cli(&ProfileStore::new(f.root.clone()), Some("off")).unwrap();
    assert_eq!(read_setting(&f.root), FileValue::Off);
    autostart::cli(&ProfileStore::new(f.root.clone()), Some("on")).unwrap();
    assert_eq!(read_setting(&f.root), FileValue::On);
}

// --- The launch entry point (real environment, under the env lock) ---------

/// Runs `autostart_on_launch` against a temp HOME and root. Every case
/// also sets an invalid `AMPLIHACK_ROTATE_HOLD`, so even a wrong gate fails
/// at step 10 and can never reach the real `spawn()`.
fn on_launch(vars: [(&str, &str); 2], docker: bool) -> Fx {
    let f = fx();
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _restore = EnvGuard::capture(&["HOME", ROOT_ENV, ENV, "AMPLIHACK_CLAUDE_AUTO_SWITCH"]);
    let _set = EnvGuard::set([
        ("HOME", f.home.to_str().unwrap()),
        (ROOT_ENV, f.root.to_str().unwrap()),
        vars[0],
        vars[1],
    ]);
    let _hold = EnvGuard::set([("AMPLIHACK_ROTATE_HOLD", "abc")]);
    super::autostart_on_launch(docker);
    f
}

#[test]
fn on_launch_with_auto_switch_off_by_env_creates_nothing() {
    let f = on_launch([("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0"), (ENV, "1")], false);
    assert!(!f.state_dir_exists());
}

#[test]
fn on_launch_with_autostart_off_by_env_creates_nothing() {
    for v in ["0", "off"] {
        let f = on_launch([("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1"), (ENV, v)], false);
        assert!(!f.state_dir_exists(), "{ENV}={v}");
    }
}

#[test]
fn on_launch_under_docker_creates_nothing() {
    let f = on_launch([("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1"), (ENV, "1")], true);
    assert!(!f.state_dir_exists());
}

// --- Issue #1690: a test process never starts a real rotator ---------------
//
// Stop B: `run` returns `Skipped("test")` as its very first step when
// `AMPLIHACK_TEST_HOME_GUARD` (`user_config::TEST_HOME_GUARD_ENV`) is
// non-empty, before any gate reads or creates anything.
// Stop C: in `cfg(test)` builds `autostart_on_launch` hands `run` a spawner
// that refuses, so an in-crate test can never reach `daemon::spawn`.

use crate::user_config::TEST_HOME_GUARD_ENV;

#[test]
fn guard_set_skips_before_any_gate() {
    let f = fx();
    let (o, err) = f.run_with(Opts {
        env: vec![(TEST_HOME_GUARD_ENV, "1")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Skipped("test"));
    assert_eq!(f.spawned(), 0, "never spawns under the test guard");
    assert!(!f.state_dir_exists(), "creates nothing under the root");
    assert_eq!(err, "", "prints nothing to the launch's stderr");
    assert_eq!(f.rotate_err(), "", "logs nothing");
}

#[test]
fn guard_skip_comes_before_the_auto_switch_gate() {
    let f = fx();
    let (o, _) = f.run_with(Opts {
        env: vec![(TEST_HOME_GUARD_ENV, "1")],
        auto_switch: Some(false),
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Skipped("test"), "step 0, not Off");
}

#[test]
fn guard_skip_comes_before_the_root_and_root_user_gates() {
    let f = fx();
    mode(&f.root, 0o757);
    let (o, _) = f.run_with(Opts {
        env: vec![(TEST_HOME_GUARD_ENV, "1")],
        euid: 0,
        ..Opts::default()
    });
    mode(&f.root, 0o700);
    assert_eq!(o, Outcome::Skipped("test"));
    assert_eq!(f.spawned(), 0);
}

#[test]
fn guard_skip_wins_over_autostart_forced_on() {
    for v in ["1", "on"] {
        let f = fx();
        let (o, err) = f.run_with(Opts {
            env: vec![(TEST_HOME_GUARD_ENV, "1"), (ENV, v)],
            ..Opts::default()
        });
        assert_eq!(o, Outcome::Skipped("test"), "{ENV}={v}");
        assert_eq!(f.spawned(), 0, "{ENV}={v}");
        assert!(!f.state_dir_exists(), "{ENV}={v}");
        assert_eq!(err, "");
    }
}

#[test]
fn empty_guard_still_starts() {
    let f = fx();
    let (o, err) = f.run_with(Opts {
        env: vec![(TEST_HOME_GUARD_ENV, "")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Started(FAKE_PID), "an empty guard is unset");
    assert_eq!(f.spawned(), 1);
    assert_eq!(err, STARTED);
}

/// Runs `autostart_on_launch` with every gate open, the guard unset and a
/// valid hold, so the only thing between it and a spawner is Stop C.
///
/// If Stop C were removed this would reach the real `daemon::spawn`, which
/// execs `current_exe()`: the libtest harness binary. libtest rejects the
/// unknown `--interval`/`--spawned` options and exits at once, so no process
/// is left behind.
fn on_launch_reaching_the_spawner() -> Fx {
    let f = fx();
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _restore = EnvGuard::capture(&[
        "HOME",
        ROOT_ENV,
        ENV,
        "AMPLIHACK_CLAUDE_AUTO_SWITCH",
        "AMPLIHACK_ROTATE_HOLD",
        TEST_HOME_GUARD_ENV,
    ]);
    let _unset = EnvGuard::unset([TEST_HOME_GUARD_ENV, "AMPLIHACK_ROTATE_HOLD"]);
    let _set = EnvGuard::set([
        ("HOME", f.home.to_str().unwrap()),
        (ROOT_ENV, f.root.to_str().unwrap()),
        ("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1"),
        (ENV, "1"),
    ]);
    super::autostart_on_launch(false);
    f
}

#[test]
fn on_launch_in_a_test_build_refuses_to_spawn() {
    let f = on_launch_reaching_the_spawner();
    let log = f.rotate_err();
    assert!(
        log.contains("autostart: failed"),
        "the cfg(test) spawner refuses and run() logs the failure: {log:?}"
    );
    assert!(
        !log.contains("autostart: started"),
        "no real rotator was started: {log:?}"
    );
    assert!(!f.rs.pid.exists(), "no pid file for a refused spawn");
}

#[test]
fn the_refusing_spawner_reveals_no_path() {
    let f = on_launch_reaching_the_spawner();
    let log = f.rotate_err();
    let tmp = f.tmp.path().to_str().unwrap();
    // Without this the absence check below would pass on an empty log.
    assert!(log.contains("autostart: failed"), "{log:?}");
    assert!(
        !log.contains(tmp),
        "fixed error text, no root or home path: {log:?}"
    );
}

#[test]
fn on_launch_with_the_guard_set_creates_nothing() {
    let f = fx();
    {
        let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _restore = EnvGuard::capture(&[
            "HOME",
            ROOT_ENV,
            ENV,
            "AMPLIHACK_CLAUDE_AUTO_SWITCH",
            TEST_HOME_GUARD_ENV,
        ]);
        let _set = EnvGuard::set([
            ("HOME", f.home.to_str().unwrap()),
            (ROOT_ENV, f.root.to_str().unwrap()),
            ("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1"),
            (ENV, "1"),
            (TEST_HOME_GUARD_ENV, "1"),
        ]);
        let _hold = EnvGuard::set([("AMPLIHACK_ROTATE_HOLD", "abc")]);
        super::autostart_on_launch(false);
    }
    assert!(!f.state_dir_exists(), "Stop B ends it before ensure()");
}
