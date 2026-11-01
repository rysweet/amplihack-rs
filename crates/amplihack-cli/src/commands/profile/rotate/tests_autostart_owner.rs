//! Issue #1803: autostart defers to a rotation owner and never runs inside a
//! recipe run, and `exhausted::exhausted_while_off` names an exhausted live
//! login while auto-switch is off.
//!
//! Every `run` here gets injected `Deps` with a spawner that refuses with
//! `autostart::REFUSED_SPAWN`, a temporary root with mode 0700 and a fake
//! `/proc`. No real rotator, tmux server, HOME or profile root is touched.
//!
//! Contract:
//! - `Outcome::Deferred { running: bool }`: an owner marker exists; `running`
//!   is whether its owner is a live, verified rotator.
//! - Step 0 is the test guard (`Skipped("test")`), step 0b
//!   `AMPLIHACK_RECIPE_RUN_ID` non-empty (`Skipped("recipe-run")`), before
//!   anything is read. The owner gate comes after the root and euid checks
//!   and before the autostart setting, Docker and the daemon state.
//! - Deferring creates nothing; a stopped owner prints one line to `d.err`
//!   per call, a running one prints nothing.
//! - `autostart_line(root)` ends with ` (deferring to <unit>)`, ` (deferring
//!   to an unreadable owner marker)` or ` (skipped: root is not private)`.
//! - `exhausted::ALERT`, `exhausted::Reason::{Threshold { pct, threshold },
//!   LimitStop { resets_at }}`, `exhausted::Exhausted { profile, reason }`
//!   (its `Display` is `<profile>: <reason>`) and
//!   `exhausted::exhausted_while_off(root, now) -> Option<Exhausted>`.

use super::autostart::{self, Deps, ENV, Outcome, REFUSED_SPAWN};
use super::exhausted::{self, ALERT, Exhausted, Reason};
use super::ledger::{RotateLedger, RotateState};
use super::owner::{self, Marker};
use super::settings::Settings;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::euid;
use crate::test_support::{EnvGuard, env_lock};
use crate::user_config::TEST_HOME_GUARD_ENV;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const EXE: &str = "amplihack-owner-gate-test";
/// Above Linux's `PID_MAX_LIMIT` (4194304): never a real process.
const OWNER_PID: u32 = 4_300_100;
const RECIPE: &str = "AMPLIHACK_RECIPE_RUN_ID";
const DEAD_LINE: &str = "amplihack: rotation is owned by amplihack-rotator.service, which is not running; 'amplihack profile rotate owner clear' to let launches start the rotator\n";
const UNREADABLE_LINE: &str = "amplihack: the rotation owner marker is unreadable; 'amplihack profile rotate owner clear' to let launches start the rotator\n";

fn mode(path: &Path, m: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(m)).unwrap();
}

fn private_file(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    mode(path, 0o600);
}

fn nul_joined(parts: &[&str]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in parts {
        v.extend_from_slice(p.as_bytes());
        v.push(0);
    }
    v
}

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    proc_root: PathBuf,
    rs: RotateState,
    spawns: Arc<AtomicU32>,
}

struct Opts {
    env: Vec<(&'static str, &'static str)>,
    auto_switch: bool,
    docker: bool,
    euid: u32,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            env: Vec::new(),
            auto_switch: true,
            docker: false,
            euid: euid().max(1),
        }
    }
}

/// Auto-switch on, autostart unset, no rotator, no marker.
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
        _tmp: tmp,
        root,
        home,
        proc_root,
        spawns: Arc::new(AtomicU32::new(0)),
    }
}

impl Fx {
    fn marker(&self, unit: &str, pid: u32) {
        self.rs.ensure().unwrap();
        owner::write(
            &self.root,
            &Marker {
                unit: unit.into(),
                pid,
                since: Some(1_793_524_205),
            },
        )
        .unwrap();
    }

    fn raw_marker(&self, body: &[u8]) {
        self.rs.ensure().unwrap();
        private_file(&self.rs.dir.join(owner::FILE), body);
    }

    fn owner_process(&self, pid: u32) {
        let d = self.proc_root.join(pid.to_string());
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("cmdline"),
            nul_joined(&[
                &format!("/usr/bin/{EXE}"),
                "profile",
                "rotate",
                "watch",
                "--supervised",
            ]),
        )
        .unwrap();
    }

    fn rotate_err(&self) -> String {
        fs::read_to_string(&self.rs.err).unwrap_or_default()
    }

    /// The names in the rotate state directory, sorted.
    fn rotate_dir(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.rs.dir)
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    fn run(&self) -> (Outcome, String) {
        self.run_with(Opts::default())
    }

    fn run_with(&self, o: Opts) -> (Outcome, String) {
        let vars = o.env.clone();
        let env = move |k: &str| -> Option<OsString> {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        };
        let on = o.auto_switch;
        let auto_switch_on = move |_: &Path| on;
        let docker = o.docker;
        let docker_env = move || docker;
        let spawns = self.spawns.clone();
        let spawn = move |_: &RotateState, _: &Settings, _: &Path| -> Result<u32, ProfileError> {
            spawns.fetch_add(1, Ordering::SeqCst);
            Err(ProfileError::RotateConfig(REFUSED_SPAWN.into()))
        };
        let mut err: Vec<u8> = Vec::new();
        let outcome = autostart::run(
            &self.root,
            false,
            Deps {
                env: &env,
                auto_switch_on: &auto_switch_on,
                docker_env: &docker_env,
                proc_root: &self.proc_root,
                exe_name: OsStr::new(EXE),
                spawn: &spawn,
                home: &self.home,
                euid: o.euid,
                err: &mut err,
            },
        );
        (outcome, String::from_utf8(err).unwrap())
    }

    fn spawned(&self) -> u32 {
        self.spawns.load(Ordering::SeqCst)
    }
}

// --- Recipe runs ---------------------------------------------------------------

#[test]
fn a_recipe_run_skips_and_creates_nothing() {
    let f = fx();
    let (o, err) = f.run_with(Opts {
        env: vec![(RECIPE, "3f9c2a1e")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Skipped("recipe-run"));
    assert_eq!(f.spawned(), 0);
    assert_eq!(err, "", "prints nothing");
    assert!(
        !f.root.join(".auto-switch.d").exists(),
        "creates no rotate/ directory"
    );
}

#[test]
fn a_recipe_run_skips_before_every_gate_after_the_test_guard() {
    let f = fx();
    mode(&f.root, 0o757);
    let (o, _) = f.run_with(Opts {
        env: vec![(RECIPE, "r"), (ENV, "1")],
        auto_switch: false,
        euid: 0,
        docker: true,
    });
    mode(&f.root, 0o700);
    assert_eq!(
        o,
        Outcome::Skipped("recipe-run"),
        "before auto-switch, root, root-user and docker"
    );
}

#[test]
fn a_recipe_run_wins_over_an_owner_marker() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    let (o, err) = f.run_with(Opts {
        env: vec![(RECIPE, "r")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Skipped("recipe-run"));
    assert_eq!(err, "", "no owner warning inside a recipe run");
}

#[test]
fn the_test_guard_still_comes_first() {
    let f = fx();
    let (o, _) = f.run_with(Opts {
        env: vec![(TEST_HOME_GUARD_ENV, "1"), (RECIPE, "r")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Skipped("test"));
}

#[test]
fn an_empty_recipe_run_id_falls_through_to_the_spawner() {
    let f = fx();
    let (o, err) = f.run_with(Opts {
        env: vec![(RECIPE, "")],
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Failed);
    assert_eq!(f.spawned(), 1, "reached the spawner");
    assert!(f.rotate_err().contains(REFUSED_SPAWN), "{}", f.rotate_err());
    assert_eq!(err, "");
}

// --- No marker: unchanged -------------------------------------------------------

#[test]
fn no_marker_reaches_the_refusing_spawner() {
    let f = fx();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Failed);
    assert_eq!(f.spawned(), 1);
    let log = f.rotate_err();
    assert!(
        log.contains("autostart: failed: ") && log.contains(REFUSED_SPAWN),
        "{log:?}"
    );
    assert_eq!(err, "");
}

// --- Deferring to an owner ------------------------------------------------------

#[test]
fn a_running_owner_defers_silently_and_creates_nothing() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    f.owner_process(OWNER_PID);
    let before = f.rotate_dir();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Deferred { running: true });
    assert_eq!(err, "", "silent while the owner runs");
    assert_eq!(f.spawned(), 0);
    assert_eq!(f.rotate_dir(), before, "no pid file, log or other file");
    assert_eq!(before, vec![owner::FILE.to_string()]);
    assert!(
        !f.root.join(".auto-switch.d/lock").exists(),
        "the state lock is never taken"
    );
}

#[test]
fn a_stopped_owner_defers_and_warns_once_per_launch() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    for _ in 0..2 {
        let (o, err) = f.run();
        assert_eq!(o, Outcome::Deferred { running: false });
        assert_eq!(err, DEAD_LINE, "exactly one line per call");
    }
    assert_eq!(f.spawned(), 0);
    assert_eq!(f.rotate_err(), "", "the warning never goes to rotate.err");
    assert_eq!(f.rotate_dir(), vec![owner::FILE.to_string()]);
}

#[test]
fn a_recycled_owner_pid_counts_as_stopped() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    let d = f.proc_root.join(OWNER_PID.to_string());
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("cmdline"), nul_joined(&["/bin/bash"])).unwrap();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Deferred { running: false });
    assert_eq!(err, DEAD_LINE);
}

#[test]
fn the_warning_names_the_unit_from_the_marker() {
    let f = fx();
    f.marker("custom-rot.service", OWNER_PID);
    let (_, err) = f.run();
    assert_eq!(
        err,
        "amplihack: rotation is owned by custom-rot.service, which is not running; 'amplihack profile rotate owner clear' to let launches start the rotator\n"
    );
}

#[test]
fn an_unreadable_marker_defers_and_never_echoes_it() {
    for body in [
        &b"garbage SECRET"[..],
        b"",
        b"unit=x.service\n",
        b"unit=x.service\npid=0\n",
    ] {
        let f = fx();
        f.raw_marker(body);
        let (o, err) = f.run();
        assert_eq!(o, Outcome::Deferred { running: false }, "{body:?}");
        assert_eq!(err, UNREADABLE_LINE, "{body:?}");
        assert_eq!(f.spawned(), 0);
    }
}

#[test]
fn a_marker_writable_by_others_defers_as_unreadable() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    mode(&f.rs.dir.join(owner::FILE), 0o666);
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Deferred { running: false });
    assert_eq!(err, UNREADABLE_LINE);
}

#[test]
fn a_symlink_at_the_marker_path_defers_as_unreadable() {
    let f = fx();
    f.rs.ensure().unwrap();
    std::os::unix::fs::symlink("/dev/null", f.rs.dir.join(owner::FILE)).unwrap();
    let (o, err) = f.run();
    assert_eq!(o, Outcome::Deferred { running: false });
    assert_eq!(err, UNREADABLE_LINE);
}

// --- Gate order -------------------------------------------------------------------

#[test]
fn auto_switch_off_comes_before_the_owner() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    let (o, err) = f.run_with(Opts {
        auto_switch: false,
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Off);
    assert_eq!(err, "");
}

#[test]
fn a_root_that_is_not_private_comes_before_the_owner() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    mode(&f.root, 0o757);
    let (o, err) = f.run();
    mode(&f.root, 0o700);
    assert_eq!(o, Outcome::Skipped("root"));
    assert_eq!(err, "", "the marker is not read");
}

#[test]
fn root_never_acts_on_the_marker() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    let (o, err) = f.run_with(Opts {
        euid: 0,
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Skipped("root-user"));
    assert_eq!(err, "");
}

#[test]
fn the_owner_wins_over_autostart_on_and_off() {
    for (env, setting) in [
        (Some("1"), None),
        (Some("0"), None),
        (None, Some(&b"off\n"[..])),
        (None, Some(&b"on\n"[..])),
    ] {
        let f = fx();
        f.marker("amplihack-rotator.service", OWNER_PID);
        if let Some(body) = setting {
            private_file(&f.rs.dir.join("autostart"), body);
        }
        let (o, _) = f.run_with(Opts {
            env: env.map(|v| vec![(ENV, v)]).unwrap_or_default(),
            ..Opts::default()
        });
        assert_eq!(
            o,
            Outcome::Deferred { running: false },
            "env {env:?}, setting {setting:?}"
        );
        assert_eq!(f.spawned(), 0);
    }
}

#[test]
fn the_owner_comes_before_docker() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    let (o, _) = f.run_with(Opts {
        docker: true,
        ..Opts::default()
    });
    assert_eq!(o, Outcome::Deferred { running: false });
}

#[test]
fn the_owner_comes_before_the_daemon_state() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    f.owner_process(OWNER_PID);
    private_file(&f.rs.pid, format!("{OWNER_PID}\n").as_bytes());
    let (o, _) = f.run();
    assert_eq!(
        o,
        Outcome::Deferred { running: true },
        "not Running: the owner gate answers first"
    );
}

#[test]
fn clearing_the_marker_lets_the_next_launch_reach_the_spawner() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    assert_eq!(f.run().0, Outcome::Deferred { running: false });
    owner::clear(&f.root).unwrap();
    assert_eq!(f.run().0, Outcome::Failed);
    assert_eq!(f.spawned(), 1);
}

fn autostart_source() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/rotate/autostart.rs");
    fs::read_to_string(p).unwrap()
}

/// The code (comments removed) of `pub fn run(` up to its closing brace.
fn run_body() -> String {
    let text = autostart_source();
    let start = text.find("pub fn run(").expect("pub fn run");
    let rest = &text[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |i| i + 3);
    rest[..end]
        .lines()
        .map(|l| l.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

#[track_caller]
fn assert_before(body: &str, first: &str, then: &str) {
    let a = body
        .find(first)
        .unwrap_or_else(|| panic!("`{first}` not found in run()"));
    let b = body
        .find(then)
        .unwrap_or_else(|| panic!("`{then}` not found in run()"));
    assert!(a < b, "`{first}` must come before `{then}` in run()");
}

#[test]
fn source_gate_order_in_run() {
    let body = run_body();
    assert_before(&body, "TEST_HOME_GUARD_ENV", "RECIPE_RUN_ENV");
    assert_before(&body, "RECIPE_RUN_ENV", "auto_switch_on");
    assert_before(&body, "check_root_private", "owner::");
    assert_before(&body, "d.euid == 0", "owner::");
    assert_before(&body, "owner::", "effective(root");
    assert_before(&body, "owner::", "rs.ensure()");
    assert_before(&body, "owner::", "try_lock");
}

#[test]
fn the_recipe_run_variable_is_named_once() {
    let text = autostart_source();
    assert!(
        text.contains("const RECIPE_RUN_ENV: &str = \"AMPLIHACK_RECIPE_RUN_ID\";"),
        "a private const names the variable"
    );
}

// --- The autostart status line ------------------------------------------------------

/// `autostart_line` reads the real `AMPLIHACK_ROTATE_AUTOSTART`, so it runs
/// under the env lock with the variable unset.
fn line_for(root: &Path) -> String {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _restore = EnvGuard::capture(&[ENV]);
    let _unset = EnvGuard::unset([ENV]);
    super::autostart_line(root)
}

#[test]
fn autostart_line_without_a_marker_is_unchanged() {
    let f = fx();
    assert_eq!(line_for(&f.root), "autostart: on (default)");
}

#[test]
fn autostart_line_names_the_owner_it_defers_to() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    assert_eq!(
        line_for(&f.root),
        "autostart: on (default) (deferring to amplihack-rotator.service)"
    );
}

#[test]
fn autostart_line_with_an_unreadable_marker() {
    let f = fx();
    f.raw_marker(b"nonsense");
    assert_eq!(
        line_for(&f.root),
        "autostart: on (default) (deferring to an unreadable owner marker)"
    );
}

#[test]
fn autostart_line_for_a_root_that_is_not_private() {
    let f = fx();
    f.marker("amplihack-rotator.service", OWNER_PID);
    mode(&f.root, 0o757);
    let line = line_for(&f.root);
    mode(&f.root, 0o700);
    assert_eq!(
        line,
        "autostart: on (default) (skipped: root is not private)"
    );
}

// --- Exhausted while off ---------------------------------------------------------------

/// 2026-11-01T14:00:00Z.
const RESET: i64 = 1_793_541_600;
const NOW: i64 = RESET - 3600;

struct Ex {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

/// A private root with `toggle` as `.auto-switch` (none when `None`), the
/// rotate ledger naming `live`, and `usage` as `usage.json`.
fn ex(toggle: Option<&str>, live: Option<&str>, usage: &str) -> Ex {
    let tmp = tempfile::tempdir().unwrap();
    mode(tmp.path(), 0o700);
    let root = tmp.path().canonicalize().unwrap();
    if let Some(t) = toggle {
        private_file(&root.join(".auto-switch"), t.as_bytes());
    }
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    if let Some(live) = live {
        rs.save_ledger(&RotateLedger {
            live: Some(live.into()),
            ..RotateLedger::default()
        })
        .unwrap();
    }
    private_file(&root.join(".auto-switch.d/usage.json"), usage.as_bytes());
    Ex { _tmp: tmp, root }
}

fn auto_switch_ledger(root: &Path, dir: &str, live: &str) {
    private_file(
        &root.join(".auto-switch.d/ledger.json"),
        format!(
            r#"{{"version":1,"hosts":{{"{dir}":{{"live":"{live}","swapped_at":0,"pre_swap_fingerprint":null}}}}}}"#
        )
        .as_bytes(),
    );
}

fn statusline(account: &str, five: f64, seven: f64) -> String {
    format!(
        r#"{{"version":1,"accounts":{{"{account}":{{"five_hour":{{"used_percentage":{five},"resets_at":{RESET}}},"seven_day":{{"used_percentage":{seven},"resets_at":{RESET}}},"observed_at":{NOW},"source":"statusline"}}}}}}"#
    )
}

fn limit_failure(account: &str, resets_at: i64, pct: f64) -> String {
    format!(
        r#"{{"version":1,"accounts":{{"{account}":{{"five_hour":{{"used_percentage":{pct},"resets_at":{resets_at}}},"observed_at":{NOW},"source":"usage_limit_failure"}}}}}}"#
    )
}

const OFF_90: &str = "enabled=off\nthreshold=90\nexclude=\n";

/// `paused` reads `AMPLIHACK_CLAUDE_AUTO_SWITCH`, so every check runs under
/// the env lock with it unset.
fn check(root: &Path, now: i64) -> Option<Exhausted> {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _restore = EnvGuard::capture(&["AMPLIHACK_CLAUDE_AUTO_SWITCH"]);
    let _unset = EnvGuard::unset(["AMPLIHACK_CLAUDE_AUTO_SWITCH"]);
    exhausted::exhausted_while_off(root, now)
}

#[test]
fn the_alert_text() {
    assert_eq!(
        ALERT,
        "auto-switch is off while the live login is exhausted"
    );
}

#[test]
fn reason_forms() {
    let t = Reason::Threshold {
        pct: 99.7,
        threshold: 90,
    };
    assert_eq!(t.to_string(), "99% used, threshold 90%", "rounded down");
    let l = Reason::LimitStop { resets_at: RESET };
    assert_eq!(
        l.to_string(),
        "usage limit reached, resets 2026-11-01T14:00:00Z"
    );
    let e = Exhausted {
        profile: "spare".into(),
        reason: Reason::Threshold {
            pct: 100.0,
            threshold: 90,
        },
    };
    assert_eq!(e.to_string(), "spare: 100% used, threshold 90%");
}

#[test]
fn over_the_threshold_while_off_is_exhausted() {
    let x = ex(
        Some(OFF_90),
        Some("spare"),
        &statusline("spare", 100.0, 20.0),
    );
    let e = check(&x.root, NOW).expect("exhausted");
    assert_eq!(e.profile, "spare");
    assert_eq!(
        e.reason,
        Reason::Threshold {
            pct: 100.0,
            threshold: 90
        }
    );
}

#[test]
fn the_weekly_window_counts_too() {
    let x = ex(
        Some(OFF_90),
        Some("spare"),
        &statusline("spare", 10.0, 95.5),
    );
    assert_eq!(
        check(&x.root, NOW).map(|e| e.reason),
        Some(Reason::Threshold {
            pct: 95.5,
            threshold: 90
        })
    );
}

#[test]
fn exactly_at_the_threshold_counts_and_just_below_does_not() {
    let x = ex(Some(OFF_90), Some("spare"), &statusline("spare", 90.0, 0.0));
    assert!(check(&x.root, NOW).is_some(), "at the threshold");
    let x = ex(
        Some(OFF_90),
        Some("spare"),
        &statusline("spare", 89.99, 0.0),
    );
    assert_eq!(check(&x.root, NOW), None, "the unrounded value is compared");
}

#[test]
fn the_files_threshold_is_used() {
    let x = ex(
        Some("enabled=off\nthreshold=50\nexclude=\n"),
        Some("spare"),
        &statusline("spare", 60.0, 0.0),
    );
    assert_eq!(
        check(&x.root, NOW).map(|e| e.reason),
        Some(Reason::Threshold {
            pct: 60.0,
            threshold: 50
        })
    );
}

#[test]
fn a_usage_limit_with_a_future_reset_is_a_limit_stop() {
    let x = ex(
        Some(OFF_90),
        Some("spare"),
        &limit_failure("spare", RESET, 0.0),
    );
    assert_eq!(
        check(&x.root, NOW).map(|e| e.reason),
        Some(Reason::LimitStop { resets_at: RESET })
    );
}

#[test]
fn the_usage_limit_form_wins_over_the_threshold() {
    let x = ex(
        Some(OFF_90),
        Some("spare"),
        &limit_failure("spare", RESET, 100.0),
    );
    assert_eq!(
        check(&x.root, NOW).map(|e| e.reason),
        Some(Reason::LimitStop { resets_at: RESET })
    );
}

#[test]
fn a_usage_limit_that_has_reset_is_not_a_limit_stop() {
    let x = ex(
        Some(OFF_90),
        Some("spare"),
        &limit_failure("spare", RESET, 0.0),
    );
    assert_eq!(check(&x.root, RESET + 1), None);
}

#[test]
fn auto_switch_on_is_never_exhausted_while_off() {
    let x = ex(
        Some("enabled=on\nthreshold=90\nexclude=\n"),
        Some("spare"),
        &statusline("spare", 100.0, 100.0),
    );
    assert_eq!(check(&x.root, NOW), None);
}

#[test]
fn no_auto_switch_file_is_not_paused() {
    let x = ex(None, Some("spare"), &statusline("spare", 100.0, 100.0));
    assert_eq!(check(&x.root, NOW), None);
}

#[test]
fn a_corrupt_auto_switch_file_counts_as_off_with_the_default_threshold() {
    let x = ex(
        Some("not a toggle file"),
        Some("spare"),
        &statusline("spare", 91.0, 0.0),
    );
    assert_eq!(
        check(&x.root, NOW).map(|e| e.reason),
        Some(Reason::Threshold {
            pct: 91.0,
            threshold: 90
        })
    );
}

#[test]
fn no_live_profile_or_no_reading_gives_nothing() {
    let x = ex(Some(OFF_90), None, &statusline("spare", 100.0, 100.0));
    assert_eq!(check(&x.root, NOW), None, "no rotate ledger");
    let x = ex(
        Some(OFF_90),
        Some("work"),
        &statusline("spare", 100.0, 100.0),
    );
    assert_eq!(check(&x.root, NOW), None, "no reading for the live login");
}

#[test]
fn a_damaged_usage_file_gives_nothing() {
    let x = ex(Some(OFF_90), Some("spare"), "{not json");
    assert_eq!(check(&x.root, NOW), None);
}

#[test]
fn a_leased_directory_maps_through_the_auto_switch_ledger() {
    // `~/.claude` holds `work`'s directory, but the auto-switch ledger says
    // the login in `work` is `spare`'s, so `spare`'s reading decides.
    let x = ex(Some(OFF_90), Some("work"), &statusline("spare", 100.0, 0.0));
    auto_switch_ledger(&x.root, "work", "spare");
    let e = check(&x.root, NOW).expect("mapped to spare");
    assert_eq!(e.profile, "spare");
}

#[test]
fn a_huge_percentage_is_not_an_alert_when_the_file_cannot_parse_it() {
    // JSON has no NaN; a value serde cannot read as f64 makes the file
    // unreadable, which gives no alert.
    let x = ex(
        Some(OFF_90),
        Some("spare"),
        &statusline("spare", 7.0, 0.0).replacen(
            "\"used_percentage\":7,",
            "\"used_percentage\":1e400,",
            1,
        ),
    );
    assert_eq!(check(&x.root, NOW), None);
}

#[test]
fn the_check_creates_and_changes_nothing() {
    let x = ex(
        Some(OFF_90),
        Some("spare"),
        &statusline("spare", 100.0, 0.0),
    );
    let list = |p: &Path| {
        let mut v: Vec<(String, Vec<u8>)> = fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .map(|p| (p.display().to_string(), fs::read(&p).unwrap()))
            .collect();
        v.sort();
        v
    };
    let dirs = [
        x.root.clone(),
        x.root.join(".auto-switch.d"),
        x.root.join(".auto-switch.d/rotate"),
    ];
    let before: Vec<_> = dirs.iter().map(|d| list(d)).collect();
    let _ = check(&x.root, NOW);
    let after: Vec<_> = dirs.iter().map(|d| list(d)).collect();
    assert_eq!(before, after);
}
