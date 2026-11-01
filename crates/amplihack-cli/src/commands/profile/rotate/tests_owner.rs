//! Issue #1803: the rotation owner, the recipe-run gate, the setting change
//! log and the exhausted-login alert.
//!
//! Every test uses a temporary root with mode 0700, a fake `/proc` and a
//! fake spawner whose pids are above Linux's `PID_MAX_LIMIT`, as in
//! `tests_autostart`. Nothing here reads the real environment, `/proc`,
//! `~/.claude`, `~/.amplihack` or a tmux server, and nothing calls the real
//! `daemon::spawn`.
//!
//! Contract (see "Rotation owner", "Rotate autostart", "Setting change log"
//! and "Exhausted live login" in docs/reference/profile-command.md):
//!
//! `rotate::owner`
//! - `DEFAULT_UNIT = "amplihack-rotator.service"`.
//! - `valid_unit(&str) -> bool`: at most 256 bytes of `[A-Za-z0-9_.@:-]`,
//!   a non-empty name followed by `.service`.
//! - `unit_from_cgroup(raw: &[u8]) -> Option<String>`: the last valid
//!   `*.service` path part of a `/proc/self/cgroup` text; `.scope` parts and
//!   invalid parts are ignored.
//! - `claim(supervised: Option<Option<&str>>, env, proc_root) ->
//!   Result<Option<String>, ProfileError>`: `None` = no `--supervised`,
//!   `Some(None)` = bare `--supervised`, `Some(Some(u))` = `--supervised=u`.
//!   `Ok(None)` when neither the flag nor a non-empty `INVOCATION_ID` asks
//!   for ownership. The unit is `u`, else the cgroup unit from
//!   `<proc_root>/self/cgroup` (read up to 64 KiB, no symlink), else
//!   `DEFAULT_UNIT`. An invalid `u` is `RotateConfig` with the documented
//!   text, before anything is written.
//! - `write(rs, unit, pid) -> Result<(), ProfileError>`: `owner.json`
//!   (`version` 1, `unit`, `pid`), mode 0600, atomically.
//! - `check(rs, proc_root, exe_name) -> OwnerState::{None, Running{unit,
//!   pid}, Down{unit, pid}, Damaged}`; `Running` only when `state_at` is
//!   `Running(p)` and `p` is the marker's pid.
//! - `OwnerState::warn_unit() -> Option<&str>`: the unit for `Down`,
//!   `DEFAULT_UNIT` for `Damaged`, `None` otherwise.
//! - `status_lines(rs, &OwnerState, autostart_on: bool) -> Vec<String>`:
//!   the `owner:` and `deferring:` lines, or none without a marker.
//!
//! `rotate::exhausted`
//! - `ALERT` and `STATUS`, the fixed log and status texts.
//! - `reading_exhausted(&Reading, session_at: u8, week_at: u8, now) -> bool`.
//! - `live_exhausted(root, session_at: u8, week_at: u8, now) -> bool`: the
//!   rotate ledger's `live` profile's reading in `usage.json`.
//!
//! `rotate::change_log`
//! - `record(root, event: &str) -> io::Result<()>` appends
//!   `<time> <event> pid=<pid> ppid=<ppid> exe=<exe>` to
//!   `<root>/.auto-switch.d/rotate/setting-changes.log`.
//! - `fields(pid, ppid, exe: Option<&Path>) -> String`: the
//!   `pid= ppid= exe=` tail, `exe=-` without a path, `exe=` last.
//!
//! `rotate::autostart::run`: `Skipped("recipe")` straight after the test
//! guard when `AMPLIHACK_RECIPE_RUN_ID` is non-empty; `Skipped("owner")`
//! after the docker gate when `owner.json` exists, printing
//! `rotation is owned by <unit>, which is not running` once to `d.err` when
//! the owner is not running or the marker is damaged.

use super::autostart::{self, Deps, ENV, Outcome, read_setting, set};
use super::change_log;
use super::exhausted;
use super::ledger::RotateState;
use super::owner::{self, DEFAULT_UNIT, OwnerState};
use super::settings::Settings;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Reading, euid};
use crate::commands::profile::store::ProfileStore;
use crate::user_config::TEST_HOME_GUARD_ENV;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const EXE: &str = "amplihack-owner-test";
/// Above `PID_MAX_LIMIT`: never a real process.
const FAKE_PID: u32 = 4_200_000;
const OWNER_PID: u32 = 4_200_100;
const RECIPE: &str = "AMPLIHACK_RECIPE_RUN_ID";
const NOW: i64 = 1_760_000_000;
const WARNING: &str = "rotation is owned by amplihack-rotator.service, which is not running\n";
const REFERENCE: &str = include_str!("../../../../../../docs/reference/profile-command.md");

fn mode(path: &Path, m: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(m)).unwrap();
}

fn private_file(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    mode(path, 0o600);
}

fn fake_process(proc_root: &Path, pid: u32) {
    let d = proc_root.join(pid.to_string());
    fs::create_dir_all(&d).unwrap();
    let mut argv = Vec::new();
    for p in [
        &format!("/usr/bin/{EXE}")[..],
        "profile",
        "rotate",
        "watch",
        "--spawned",
    ] {
        argv.extend_from_slice(p.as_bytes());
        argv.push(0);
    }
    fs::write(d.join("cmdline"), argv).unwrap();
}

struct Fx {
    tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    proc_root: PathBuf,
    rs: RotateState,
    spawns: Arc<AtomicU32>,
}

/// Auto-switch on, autostart unset, no rotator, no owner.
fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    mode(tmp.path(), 0o700);
    let [root, home, proc_root] = ["root", "home", "proc"].map(|n| tmp.path().join(n));
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
    }
}

impl Fx {
    fn marker_path(&self) -> PathBuf {
        self.rs.dir.join("owner.json")
    }

    fn marker(&self, body: &str) {
        self.rs.ensure().unwrap();
        private_file(&self.marker_path(), body.as_bytes());
    }

    fn valid_marker(&self, pid: u32) {
        self.marker(&format!(
            r#"{{"version":1,"unit":"amplihack-rotator.service","pid":{pid}}}"#
        ));
    }

    fn running(&self, pid: u32) {
        self.rs.ensure().unwrap();
        private_file(&self.rs.pid, format!("{pid}\n").as_bytes());
        fake_process(&self.proc_root, pid);
    }

    fn check(&self) -> OwnerState {
        owner::check(&self.rs, &self.proc_root, OsStr::new(EXE))
    }

    fn spawned(&self) -> u32 {
        self.spawns.load(Ordering::SeqCst)
    }

    fn rotate_err(&self) -> String {
        fs::read_to_string(&self.rs.err).unwrap_or_default()
    }

    fn changes(&self) -> String {
        fs::read_to_string(self.rs.dir.join("setting-changes.log")).unwrap_or_default()
    }

    fn run(&self, env: &[(&'static str, &'static str)]) -> (Outcome, String) {
        self.run_opts(env, None, false)
    }

    /// One `autostart::run`. `auto_switch` overrides the `.auto-switch` file.
    fn run_opts(
        &self,
        env: &[(&'static str, &'static str)],
        auto_switch: Option<bool>,
        docker: bool,
    ) -> (Outcome, String) {
        let vars = env.to_vec();
        let env = move |k: &str| -> Option<OsString> {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        };
        let auto_switch_on = move |r: &Path| -> bool {
            auto_switch.unwrap_or_else(|| {
                fs::read_to_string(r.join(".auto-switch")).is_ok_and(|t| t.contains("enabled=on"))
            })
        };
        let docker_env = || false;
        let spawns = self.spawns.clone();
        let fake_proc = self.proc_root.clone();
        let spawn = move |_: &RotateState, _: &Settings, _: &Path| -> Result<u32, ProfileError> {
            let pid = FAKE_PID + spawns.fetch_add(1, Ordering::SeqCst);
            fake_process(&fake_proc, pid);
            Ok(pid)
        };
        let mut err: Vec<u8> = Vec::new();
        let o = autostart::run(
            &self.root,
            docker,
            Deps {
                env: &env,
                auto_switch_on: &auto_switch_on,
                docker_env: &docker_env,
                proc_root: &self.proc_root,
                exe_name: OsStr::new(EXE),
                spawn: &spawn,
                home: &self.home,
                euid: euid().max(1),
                err: &mut err,
            },
        );
        (o, String::from_utf8(err).unwrap())
    }
}

// --- R2: the recipe-run gate ------------------------------------------------

#[test]
fn a_launch_inside_a_recipe_run_starts_nothing_and_says_nothing() {
    let f = fx();
    let (o, err) = f.run(&[(RECIPE, "run-123")]);
    assert_eq!(o, Outcome::Skipped("recipe"));
    assert_eq!(err, "", "silent");
    assert_eq!(f.spawned(), 0);
    assert!(!f.root.join(".auto-switch.d").exists(), "creates nothing");
}

#[test]
fn autostart_forced_on_does_not_override_the_recipe_gate() {
    for v in ["1", "on"] {
        let f = fx();
        let (o, err) = f.run(&[(RECIPE, "run-123"), (ENV, v)]);
        assert_eq!(o, Outcome::Skipped("recipe"), "{ENV}={v}");
        assert_eq!(err, "");
        assert_eq!(f.spawned(), 0);
    }
}

#[test]
fn an_empty_recipe_run_id_counts_as_unset() {
    let f = fx();
    let (o, _) = f.run(&[(RECIPE, "")]);
    assert_eq!(o, Outcome::Started(FAKE_PID));
}

#[test]
fn the_recipe_gate_comes_straight_after_the_test_guard() {
    let f = fx();
    let (o, _) = f.run(&[(TEST_HOME_GUARD_ENV, "1"), (RECIPE, "x")]);
    assert_eq!(o, Outcome::Skipped("test"), "the test guard is first");
    let (o, _) = f.run_opts(&[(RECIPE, "x")], Some(false), false);
    assert_eq!(o, Outcome::Skipped("recipe"), "before the auto-switch gate");
}

#[test]
fn a_recipe_run_prints_no_owner_warning() {
    let f = fx();
    f.valid_marker(OWNER_PID);
    let (o, err) = f.run(&[(RECIPE, "x")]);
    assert_eq!(o, Outcome::Skipped("recipe"));
    assert_eq!(err, "", "nothing is printed inside a recipe run");
}

// --- R2, R3: the owner gate -------------------------------------------------

#[test]
fn a_stopped_owner_blocks_autostart_and_warns_once() {
    let f = fx();
    f.valid_marker(OWNER_PID);
    let (o, err) = f.run(&[]);
    assert_eq!(o, Outcome::Skipped("owner"));
    assert_eq!(err, WARNING, "exactly one line");
    assert_eq!(f.spawned(), 0);
    assert!(!f.rs.pid.exists(), "no rotate.pid");
    assert_eq!(f.rotate_err(), "", "nothing logged");
}

#[test]
fn the_warning_names_the_unit_in_the_marker() {
    let f = fx();
    f.marker(&format!(
        r#"{{"version":1,"unit":"rot@ryan.service","pid":{OWNER_PID}}}"#
    ));
    let (_, err) = f.run(&[]);
    assert_eq!(
        err,
        "rotation is owned by rot@ryan.service, which is not running\n"
    );
}

#[test]
fn a_running_owner_blocks_autostart_silently() {
    let f = fx();
    f.running(OWNER_PID);
    f.valid_marker(OWNER_PID);
    let (o, err) = f.run(&[]);
    assert_eq!(o, Outcome::Skipped("owner"));
    assert_eq!(err, "");
    assert_eq!(f.spawned(), 0);
}

#[test]
fn a_manual_rotator_under_another_pid_does_not_count_as_the_owner() {
    let f = fx();
    f.running(48_213);
    f.valid_marker(OWNER_PID);
    let (o, err) = f.run(&[]);
    assert_eq!(
        o,
        Outcome::Skipped("owner"),
        "the owner gate is before step 8"
    );
    assert_eq!(err, WARNING);
    assert_eq!(f.spawned(), 0);
}

#[test]
fn autostart_forced_on_does_not_override_the_owner_gate() {
    for v in ["1", "on"] {
        let f = fx();
        f.valid_marker(OWNER_PID);
        let (o, err) = f.run(&[(ENV, v)]);
        assert_eq!(o, Outcome::Skipped("owner"), "{ENV}={v}");
        assert_eq!(err, WARNING);
        assert_eq!(f.spawned(), 0);
    }
}

#[test]
fn no_warning_when_auto_switch_or_autostart_is_off_or_under_docker() {
    let f = fx();
    f.valid_marker(OWNER_PID);
    let (o, err) = f.run_opts(&[], Some(false), false);
    assert_eq!((o, err.as_str()), (Outcome::Off, ""), "auto-switch off");
    let (o, err) = f.run(&[(ENV, "off")]);
    assert_eq!((o, err.as_str()), (Outcome::Off, ""), "autostart off");
    let (o, err) = f.run_opts(&[], None, true);
    assert_eq!(
        (o, err.as_str()),
        (Outcome::Skipped("docker"), ""),
        "docker comes first"
    );
    assert_eq!(f.spawned(), 0);
}

#[test]
fn a_damaged_marker_blocks_autostart_and_warns_with_the_default_unit() {
    let f = fx();
    f.marker("{\"version\":1,\"unit\":\"\u{1b}[31mevil.service\",\"pid\":7}");
    let (o, err) = f.run(&[]);
    assert_eq!(o, Outcome::Skipped("owner"));
    assert_eq!(err, WARNING, "never text from a damaged file");
    assert_eq!(f.spawned(), 0);
}

#[test]
fn random_bytes_in_the_marker_never_panic_or_start_a_rotator() {
    let f = fx();
    let mut seed: u64 = 0x1803;
    for len in [0usize, 1, 7, 64, 513, 4096] {
        let body: Vec<u8> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                (seed >> 33) as u8
            })
            .collect();
        f.rs.ensure().unwrap();
        private_file(&f.marker_path(), &body);
        let (o, err) = f.run(&[]);
        assert_eq!(o, Outcome::Skipped("owner"), "len {len}");
        assert_eq!(err, WARNING, "len {len}");
    }
    assert_eq!(f.spawned(), 0);
}

#[test]
fn without_a_marker_autostart_is_unchanged() {
    let f = fx();
    let (o, err) = f.run(&[]);
    assert_eq!(o, Outcome::Started(FAKE_PID));
    assert!(!err.contains("rotation is owned"), "{err:?}");
    assert!(!f.marker_path().exists(), "a launch never writes a marker");
}

// --- R1, D1, D3: reading the marker ------------------------------------------

#[test]
fn check_reports_each_state() {
    let f = fx();
    assert_eq!(f.check(), OwnerState::None);
    f.valid_marker(OWNER_PID);
    let down = OwnerState::Down {
        unit: DEFAULT_UNIT.into(),
        pid: OWNER_PID,
    };
    assert_eq!(f.check(), down, "no rotator");
    fake_process(&f.proc_root, OWNER_PID);
    assert_eq!(f.check(), down, "a live pid that is not the rotator");
    f.running(OWNER_PID + 1);
    assert_eq!(f.check(), down, "another rotator");
    f.running(OWNER_PID);
    assert_eq!(
        f.check(),
        OwnerState::Running {
            unit: DEFAULT_UNIT.into(),
            pid: OWNER_PID
        }
    );
}

#[test]
fn damaged_markers() {
    let ok = format!(r#"{{"version":1,"unit":"a.service","pid":{OWNER_PID}}}"#);
    let cases: Vec<(&str, String)> = vec![
        ("not json", "{".into()),
        ("empty", String::new()),
        (
            "wrong version",
            ok.replace("\"version\":1", "\"version\":2"),
        ),
        ("pid 0", ok.replace(&OWNER_PID.to_string(), "0")),
        ("negative pid", ok.replace(&OWNER_PID.to_string(), "-5")),
        ("unknown field", ok.replace('}', ",\"extra\":1}")),
        (
            "missing unit",
            format!(r#"{{"version":1,"pid":{OWNER_PID}}}"#),
        ),
        ("unit without .service", ok.replace("a.service", "rotator")),
        ("scope unit", ok.replace("a.service", "a.scope")),
        ("unit with a space", ok.replace("a.service", "a b.service")),
        ("unit with a slash", ok.replace("a.service", "../a.service")),
        (
            "unit too long",
            ok.replace("a.service", &format!("{}.service", "a".repeat(250))),
        ),
    ];
    for (what, body) in cases {
        let f = fx();
        f.marker(&body);
        assert_eq!(f.check(), OwnerState::Damaged, "{what}: {body}");
    }
}

#[test]
fn a_marker_over_64_kib_is_damaged() {
    let f = fx();
    let pad = " ".repeat(64 * 1024);
    f.marker(&format!(
        r#"{{"version":1,"unit":"a.service","pid":{OWNER_PID}}}{pad}"#
    ));
    assert_eq!(f.check(), OwnerState::Damaged);
}

#[test]
fn an_untrusted_marker_file_is_damaged() {
    for m in [0o622, 0o660, 0o602] {
        let f = fx();
        f.valid_marker(OWNER_PID);
        mode(&f.marker_path(), m);
        assert_eq!(f.check(), OwnerState::Damaged, "mode {m:o}");
    }
    let f = fx();
    f.valid_marker(OWNER_PID);
    mode(&f.marker_path(), 0o644);
    assert!(
        matches!(f.check(), OwnerState::Down { .. }),
        "readable by others is not writable by others"
    );
}

#[test]
fn a_symlinked_or_directory_marker_is_damaged_and_not_followed() {
    let f = fx();
    let target = f.tmp.path().join("elsewhere.json");
    private_file(
        &target,
        format!(r#"{{"version":1,"unit":"a.service","pid":{OWNER_PID}}}"#).as_bytes(),
    );
    f.rs.ensure().unwrap();
    std::os::unix::fs::symlink(&target, f.marker_path()).unwrap();
    assert_eq!(f.check(), OwnerState::Damaged);
    let (o, err) = f.run(&[]);
    assert_eq!((o, err.as_str()), (Outcome::Skipped("owner"), WARNING));

    let f = fx();
    f.rs.ensure().unwrap();
    fs::create_dir(f.marker_path()).unwrap();
    assert_eq!(f.check(), OwnerState::Damaged);
}

#[test]
fn warn_unit_per_state() {
    assert_eq!(OwnerState::None.warn_unit(), None);
    let running = OwnerState::Running {
        unit: "x.service".into(),
        pid: 9,
    };
    assert_eq!(running.warn_unit(), None);
    let down = OwnerState::Down {
        unit: "x.service".into(),
        pid: 9,
    };
    assert_eq!(down.warn_unit(), Some("x.service"));
    assert_eq!(OwnerState::Damaged.warn_unit(), Some(DEFAULT_UNIT));
}

// --- R1: writing the marker ---------------------------------------------------

#[test]
fn write_records_a_private_versioned_marker() {
    let f = fx();
    f.rs.ensure().unwrap();
    owner::write(&f.rs, "amplihack-rotator.service", OWNER_PID).unwrap();
    let raw = fs::read_to_string(f.marker_path()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"version": 1, "unit": "amplihack-rotator.service", "pid": OWNER_PID})
    );
    let m = fs::metadata(f.marker_path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(m, 0o600);
    assert!(matches!(f.check(), OwnerState::Down { pid: OWNER_PID, .. }));
}

#[test]
fn write_replaces_a_symlink_without_following_it() {
    let f = fx();
    let target = f.tmp.path().join("victim");
    private_file(&target, b"untouched");
    f.rs.ensure().unwrap();
    std::os::unix::fs::symlink(&target, f.marker_path()).unwrap();
    let _ = owner::write(&f.rs, "a.service", OWNER_PID);
    assert_eq!(fs::read(&target).unwrap(), b"untouched");
}

// --- D4, D5: unit names and claims -------------------------------------------

#[test]
fn unit_names() {
    let max = format!("{}.service", "a".repeat(248));
    assert_eq!(max.len(), 256);
    for ok in [
        "amplihack-rotator.service",
        "user@1000.service",
        "a.service",
        "x:y_z.v-1.service",
        max.as_str(),
    ] {
        assert!(owner::valid_unit(ok), "{ok:?} is valid");
    }
    let long = format!("{}.service", "a".repeat(249));
    for bad in [
        "",
        ".service",
        "rotator",
        "amplihack.scope",
        "a b.service",
        "a/b.service",
        "a\n.service",
        "\u{1b}[31m.service",
        "é.service",
        "a.service\0",
        "a.service.bak",
        long.as_str(),
    ] {
        assert!(!owner::valid_unit(bad), "{bad:?} is invalid");
    }
}

#[test]
fn cgroup_units() {
    let v2 =
        b"0::/user.slice/user-1000.slice/user@1000.service/app.slice/amplihack-rotator.service\n";
    assert_eq!(
        owner::unit_from_cgroup(v2).as_deref(),
        Some("amplihack-rotator.service")
    );
    let v1 = b"12:pids:/system.slice/rot.service\n1:name=systemd:/system.slice/rot.service\n0::/system.slice/rot.service\n";
    assert_eq!(owner::unit_from_cgroup(v1).as_deref(), Some("rot.service"));
    let scope = b"0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-1.scope\n";
    assert_eq!(
        owner::unit_from_cgroup(scope).as_deref(),
        Some("user@1000.service"),
        ".scope parts are skipped"
    );
    for none in [
        &b""[..],
        b"0::/\n",
        b"0::/user.slice/session-3.scope\n",
        b"0::/system.slice/\x1b[31mevil.service\n",
        b"0::/system.slice/a b.service\n",
        b"\xff\xfe\x00garbage",
    ] {
        assert_eq!(owner::unit_from_cgroup(none), None, "{none:?}");
    }
}

fn env_of(vars: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let vars = vars.to_vec();
    move |k: &str| {
        vars.iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| OsString::from(v))
    }
}

fn cgroup(proc_root: &Path, body: &[u8]) {
    private_file(&proc_root.join("self").join("cgroup"), body);
}

#[test]
fn claim_matrix() {
    let f = fx();
    let none = env_of(&[]);
    let inv = env_of(&[("INVOCATION_ID", "0123abcd")]);
    let empty = env_of(&[("INVOCATION_ID", "")]);
    let p = f.proc_root.as_path();
    assert_eq!(
        owner::claim(None, &none, p).unwrap(),
        None,
        "a manual watch"
    );
    assert_eq!(
        owner::claim(None, &empty, p).unwrap(),
        None,
        "empty is unset"
    );
    assert_eq!(
        owner::claim(None, &inv, p).unwrap().as_deref(),
        Some(DEFAULT_UNIT),
        "no cgroup file"
    );
    assert_eq!(
        owner::claim(Some(None), &none, p).unwrap().as_deref(),
        Some(DEFAULT_UNIT)
    );
    cgroup(p, b"0::/system.slice/my-rot.service\n");
    assert_eq!(
        owner::claim(None, &inv, p).unwrap().as_deref(),
        Some("my-rot.service")
    );
    assert_eq!(
        owner::claim(Some(None), &none, p).unwrap().as_deref(),
        Some("my-rot.service")
    );
    assert_eq!(
        owner::claim(Some(Some("x@1.service")), &none, p)
            .unwrap()
            .as_deref(),
        Some("x@1.service"),
        "the flag's value wins over the cgroup"
    );
}

#[test]
fn a_hostile_or_oversized_cgroup_falls_back_to_the_default_unit() {
    let f = fx();
    let p = f.proc_root.as_path();
    cgroup(p, b"0::/system.slice/\x1b]0;pwned\x07.service\n");
    assert_eq!(
        owner::claim(Some(None), &env_of(&[]), p)
            .unwrap()
            .as_deref(),
        Some(DEFAULT_UNIT)
    );
    let mut big = vec![b'/'; 64 * 1024 + 1];
    big.extend_from_slice(b"big.service\n");
    cgroup(p, &big);
    assert_eq!(
        owner::claim(Some(None), &env_of(&[]), p)
            .unwrap()
            .as_deref(),
        Some(DEFAULT_UNIT)
    );
}

#[test]
fn an_invalid_supervised_value_is_a_config_error() {
    let f = fx();
    let err = owner::claim(Some(Some("rotator")), &env_of(&[]), &f.proc_root).unwrap_err();
    let ProfileError::RotateConfig(msg) = &err else {
        panic!("{err:?}");
    };
    assert_eq!(
        msg,
        "invalid rotate setting --supervised \"rotator\": expected a unit name ending in .service"
    );
    for bad in ["", "a b.service", "a.scope"] {
        let e = owner::claim(Some(Some(bad)), &env_of(&[]), &f.proc_root).unwrap_err();
        assert!(matches!(e, ProfileError::RotateConfig(_)), "{bad:?}: {e:?}");
    }
}

#[test]
fn a_rejected_value_is_escaped_and_cut() {
    let f = fx();
    let hostile = format!("\u{1b}[31m{}", "x".repeat(5000));
    let err = owner::claim(Some(Some(hostile.as_str())), &env_of(&[]), &f.proc_root).unwrap_err();
    let msg = err.to_string();
    assert!(!msg.contains('\u{1b}'), "escaped: {msg:?}");
    assert!(
        msg.len() < 1024,
        "cut to 256 characters: {} bytes",
        msg.len()
    );
    assert!(msg.contains("expected a unit name ending in .service"));
}

#[test]
fn a_claim_never_echoes_invocation_id() {
    let f = fx();
    let inv = env_of(&[("INVOCATION_ID", "secret-invocation-0123")]);
    let unit = owner::claim(None, &inv, &f.proc_root).unwrap().unwrap();
    assert!(!unit.contains("secret-invocation"), "{unit}");
}

// --- R6: status lines --------------------------------------------------------

#[test]
fn status_lines_per_state() {
    let f = fx();
    assert!(owner::status_lines(&f.rs, &OwnerState::None, true).is_empty());
    let running = OwnerState::Running {
        unit: "a.service".into(),
        pid: 48_190,
    };
    assert_eq!(
        owner::status_lines(&f.rs, &running, true),
        ["owner: a.service (pid 48190, running)", "deferring: yes"]
    );
    let down = OwnerState::Down {
        unit: "a.service".into(),
        pid: 48_190,
    };
    assert_eq!(
        owner::status_lines(&f.rs, &down, true),
        [
            "owner: a.service (pid 48190, not running)",
            "deferring: yes"
        ]
    );
    assert_eq!(
        owner::status_lines(&f.rs, &down, false),
        [
            "owner: a.service (pid 48190, not running)",
            "deferring: no (autostart off)"
        ]
    );
    let damaged = owner::status_lines(&f.rs, &OwnerState::Damaged, true);
    assert_eq!(
        damaged,
        [
            format!("owner: unreadable marker {}", f.marker_path().display()),
            "deferring: yes".to_string()
        ]
    );
}

// --- R10, D10: the exhausted live login ---------------------------------------

fn reading(v: serde_json::Value) -> Reading {
    serde_json::from_value(v).unwrap()
}

fn statusline(five: Option<(f64, i64)>, week: Option<(f64, i64)>) -> Reading {
    let mut v = serde_json::json!({"observed_at": NOW - 60, "source": "statusline"});
    if let Some((p, r)) = five {
        v["five_hour"] = serde_json::json!({"used_percentage": p, "resets_at": r});
    }
    if let Some((p, r)) = week {
        v["seven_day"] = serde_json::json!({"used_percentage": p, "resets_at": r});
    }
    reading(v)
}

#[test]
fn readings_at_or_over_their_own_threshold_are_exhausted() {
    let later = NOW + 600;
    let ex = |r: &Reading| exhausted::reading_exhausted(r, 85, 90, NOW);
    assert!(ex(&statusline(Some((85.0, later)), None)), "session at");
    assert!(!ex(&statusline(Some((84.9, later)), None)), "session under");
    assert!(ex(&statusline(None, Some((90.0, later)))), "week at");
    assert!(!ex(&statusline(None, Some((89.0, later)))), "week under");
    assert!(
        !ex(&statusline(Some((10.0, later)), Some((88.0, later)))),
        "a week reading is judged against week_at, not session_at"
    );
    assert!(ex(&statusline(Some((100.0, later)), Some((5.0, later)))));
    assert!(!ex(&statusline(None, None)), "no window");
}

#[test]
fn a_window_that_has_reset_is_not_exhausted() {
    let ex = |r: &Reading| exhausted::reading_exhausted(r, 85, 90, NOW);
    assert!(!ex(&statusline(Some((100.0, NOW)), None)), "reset now");
    assert!(
        !ex(&statusline(Some((100.0, NOW - 1)), None)),
        "reset before"
    );
    assert!(!ex(&statusline(None, Some((100.0, NOW - 3600)))));
}

#[test]
fn an_unexpired_usage_limit_record_is_exhausted() {
    let limit = |resets_at: i64| {
        reading(serde_json::json!({
            "five_hour": {"used_percentage": 100.0, "resets_at": resets_at},
            "observed_at": NOW - 60,
            "source": "usage_limit_failure"
        }))
    };
    assert!(exhausted::reading_exhausted(&limit(NOW + 60), 85, 90, NOW));
    assert!(!exhausted::reading_exhausted(&limit(NOW - 60), 85, 90, NOW));
    let bare = reading(serde_json::json!({"observed_at": NOW, "source": "usage_limit_failure"}));
    assert!(
        !exhausted::reading_exhausted(&bare, 85, 90, NOW),
        "no window recorded"
    );
}

fn ledger(f: &Fx, live: Option<&str>) {
    f.rs.ensure().unwrap();
    let mut v = serde_json::json!({"version": 1});
    if let Some(l) = live {
        v["live"] = serde_json::json!(l);
    }
    private_file(&f.rs.ledger, v.to_string().as_bytes());
}

fn usage(f: &Fx, accounts: serde_json::Value) {
    f.rs.ensure().unwrap();
    let body = serde_json::json!({"version": 1, "accounts": accounts}).to_string();
    private_file(
        &f.root.join(".auto-switch.d").join("usage.json"),
        body.as_bytes(),
    );
}

fn hot(pct: f64) -> serde_json::Value {
    serde_json::json!({
        "five_hour": {"used_percentage": pct, "resets_at": NOW + 3600},
        "observed_at": NOW - 60,
        "source": "statusline"
    })
}

#[test]
fn live_exhausted_reads_the_live_profiles_cached_reading() {
    let f = fx();
    let ex = |f: &Fx| exhausted::live_exhausted(&f.root, 85, 90, NOW);
    assert!(!ex(&f), "no ledger, no usage");
    ledger(&f, None);
    usage(&f, serde_json::json!({"work": hot(95.0)}));
    assert!(!ex(&f), "no live profile");
    ledger(&f, Some("spare"));
    assert!(!ex(&f), "no reading for the live profile");
    ledger(&f, Some("work"));
    assert!(ex(&f), "the live profile is over the threshold");
    usage(
        &f,
        serde_json::json!({"work": hot(50.0), "spare": hot(99.0)}),
    );
    assert!(!ex(&f), "only the live profile counts");
    assert!(
        exhausted::live_exhausted(&f.root, 40, 90, NOW),
        "thresholds are the caller's"
    );
}

#[test]
fn damaged_inputs_count_as_not_exhausted() {
    let f = fx();
    ledger(&f, Some("work"));
    private_file(&f.root.join(".auto-switch.d").join("usage.json"), b"{nope");
    assert!(!exhausted::live_exhausted(&f.root, 85, 90, NOW));
    usage(&f, serde_json::json!({"work": hot(95.0)}));
    private_file(&f.rs.ledger, b"not json");
    assert!(!exhausted::live_exhausted(&f.root, 85, 90, NOW));
}

#[test]
fn the_alert_texts_are_fixed_and_name_nothing() {
    assert_eq!(
        exhausted::ALERT,
        "ALERT: auto-switch is off while the live login is exhausted"
    );
    assert_eq!(
        exhausted::STATUS,
        "alert: auto-switch is off while the live login is exhausted"
    );
    for t in [exhausted::ALERT, exhausted::STATUS] {
        assert!(
            !t.chars()
                .any(|c| c.is_ascii_digit() || c == '@' || c == '%')
        );
    }
}

// --- R8: the autostart change log ---------------------------------------------

fn change_lines(f: &Fx) -> Vec<String> {
    f.changes().lines().map(String::from).collect()
}

/// `<time> <event> pid=.. ppid=.. exe=..` for this process.
#[track_caller]
fn assert_change(line: &str, event: &str) {
    let (_, rest) = line.split_once(' ').expect("time prefix");
    let tail = change_log::fields(
        std::process::id(),
        // SAFETY: getppid has no preconditions.
        unsafe { libc::getppid() } as u32,
        std::env::current_exe().ok().as_deref(),
    );
    assert_eq!(rest, format!("{event} {tail}"), "{line:?}");
}

#[test]
fn autostart_on_and_off_are_logged_with_the_old_value() {
    let f = fx();
    let store = ProfileStore::new(f.root.clone());
    set(&store, false).unwrap();
    set(&store, true).unwrap();
    set(&store, true).unwrap();
    let lines = change_lines(&f);
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_change(&lines[0], "autostart set=off was=missing");
    assert_change(&lines[1], "autostart set=on was=off");
    assert_change(&lines[2], "autostart set=on was=on");
    let m = fs::metadata(f.rs.dir.join("setting-changes.log"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(m, 0o600);
}

#[test]
fn a_corrupt_autostart_setting_is_logged_as_corrupt() {
    let f = fx();
    f.rs.ensure().unwrap();
    private_file(&f.rs.dir.join("autostart"), b"maybe\n");
    set(&ProfileStore::new(f.root.clone()), true).unwrap();
    assert_change(&change_lines(&f)[0], "autostart set=on was=corrupt");
}

#[test]
fn reading_the_autostart_state_logs_nothing() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = crate::test_support::EnvGuard::unset([ENV]);
    let f = fx();
    autostart::cli(&ProfileStore::new(f.root.clone()), None).unwrap();
    assert_eq!(f.changes(), "");
}

#[test]
fn an_unwritable_change_log_does_not_fail_the_setting() {
    let f = fx();
    f.rs.ensure().unwrap();
    let target = f.tmp.path().join("victim");
    private_file(&target, b"untouched");
    std::os::unix::fs::symlink(&target, f.rs.dir.join("setting-changes.log")).unwrap();
    set(&ProfileStore::new(f.root.clone()), false).unwrap();
    assert_eq!(
        read_setting(&f.root),
        super::autostart::FileValue::Off,
        "the setting is still written"
    );
    assert_eq!(fs::read(&target).unwrap(), b"untouched", "not followed");

    let f = fx();
    f.rs.ensure().unwrap();
    fs::create_dir(f.rs.dir.join("setting-changes.log")).unwrap();
    set(&ProfileStore::new(f.root.clone()), true).unwrap();
    assert_eq!(read_setting(&f.root), super::autostart::FileValue::On);
}

#[test]
fn record_refuses_a_root_that_is_not_private() {
    let f = fx();
    mode(&f.root, 0o757);
    let r = change_log::record(&f.root, "autostart set=on was=off");
    mode(&f.root, 0o700);
    assert!(r.is_err());
    assert!(!f.rs.dir.join("setting-changes.log").exists());
}

#[test]
fn record_appends_one_timed_line_per_call() {
    let f = fx();
    change_log::record(&f.root, "auto-switch enabled=off was=on").unwrap();
    change_log::record(&f.root, "auto-switch enabled=on was=off").unwrap();
    let lines = change_lines(&f);
    assert_eq!(lines.len(), 2);
    assert_change(&lines[0], "auto-switch enabled=off was=on");
    assert_change(&lines[1], "auto-switch enabled=on was=off");
}

#[test]
fn change_fields_keep_exe_last_and_one_line() {
    assert_eq!(
        change_log::fields(10, 2, Some(Path::new("/a b/amplihack"))),
        "pid=10 ppid=2 exe=/a b/amplihack"
    );
    assert_eq!(change_log::fields(10, 2, None), "pid=10 ppid=2 exe=-");
    let odd = change_log::fields(10, 2, Some(Path::new("/x\npid=1 ppid=1 exe=/y")));
    assert!(!odd.contains('\n'), "{odd:?}");
    assert!(odd.starts_with("pid=10 ppid=2 exe="), "{odd:?}");
}

// --- R11: the code's texts are the documented ones ----------------------------

#[test]
fn the_reference_shows_the_codes_texts() {
    for t in [
        WARNING.trim_end(),
        exhausted::ALERT,
        exhausted::STATUS,
        DEFAULT_UNIT,
        "owner: amplihack-rotator.service (pid 48190, not running)",
        "`no (autostart off)`",
        "warning: setting change not logged: <error>",
        "expected a unit name ending in .service",
    ] {
        assert!(REFERENCE.contains(t), "docs/reference lacks {t:?}");
    }
}
