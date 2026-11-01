//! Issue #1803: the rotation owner marker (`owner`) and the change log
//! (`change_log`). Every test uses a temporary root with mode 0700 and, where
//! liveness is checked, a fake `/proc`. Nothing here reads the real profile
//! root, `/proc/<pid>` of a real rotator or the real environment.
//!
//! Contract:
//! - `owner::FILE` is `"owner"`; the marker is `<root>/.auto-switch.d/rotate/owner`.
//! - `owner::DEFAULT_UNIT` is `"amplihack-rotator.service"`.
//! - `owner::Marker { unit: String, pid: u32, since: Option<i64> }`.
//! - `owner::OwnerState::{None, Present(Marker), Unreadable}`.
//! - `owner::read(root: &Path) -> OwnerState`: a regular file, opened without
//!   following symlinks and without blocking, owned by the euid, not
//!   writable by group or others, at most 4 KiB, holding `unit` and `pid`
//!   exactly once each; `pid` a positive decimal without a sign; `unit`
//!   non-empty, at most 256 bytes, no `=`, control or bidi character. Unknown
//!   keys are ignored; `since` is informational.
//! - `owner::write(root: &Path, m: &Marker) -> std::io::Result<()>`: writes
//!   `unit=..\npid=..\nsince=..\n` with mode 0600 into a directory that has
//!   already passed `RotateState::ensure`.
//! - `owner::clear(root: &Path) -> std::io::Result<Option<OwnerState>>`:
//!   what was removed, `None` when nothing was there. Removes a symlink
//!   itself and never its target. `owner::clear_checked(root)` first
//!   refuses a root that is not private, leaving the marker in place.
//! - `owner::alive(m: &Marker, proc_root: &Path, exe_name: &OsStr) -> bool`.
//! - `owner::own_service(proc_root: &Path) -> Option<String>`: the last
//!   component of the path in `<proc_root>/self/cgroup` when it is a
//!   `*.service` other than `user@*.service`.
//! - `owner::unit_name(flag: Option<&str>, proc_root: &Path) -> String`:
//!   the flag, then `own_service`, then `DEFAULT_UNIT`.
//! - `owner::supervised(flag: &Option<Option<String>>, env: &dyn Fn(&str) ->
//!   Option<OsString>, proc_root: &Path) -> bool`: the flag, or a non-empty
//!   `INVOCATION_ID` together with `own_service`.
//! - `owner::describe(root: &Path, proc_root: &Path, exe_name: Option<&OsStr>) ->
//!   String`: the whole `owner: ...` status line.
//! - `change_log::Caller { pid: u32, ppid: u32, parent: Option<Vec<u8>>,
//!   recipe_run: Option<String>, invocation: Option<String> }`, where
//!   `parent` is the raw NUL-separated `/proc/<ppid>/cmdline`.
//! - `change_log::caller(env, proc_root: &Path) -> Caller`.
//! - `change_log::line(setting: &str, old: &str, new: &str, c: &Caller) ->
//!   String`: `change: <setting> <old> -> <new> by pid=<p> ppid=<pp>
//!   parent="<cmd>"[ recipe_run=<id>][ invocation=<id>]`, without a time.
//! - `change_log::record(root: &Path, setting: &str, old: &str, new: &str)`:
//!   appends the timed line to `rotate.log`; never fails.

use super::change_log::{self, Caller};
use super::ledger::RotateState;
use super::owner::{self, DEFAULT_UNIT, Marker, OwnerState};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const EXE: &str = "amplihack-owner-test";
/// Above Linux's `PID_MAX_LIMIT` (4194304): never a real process.
const OWNER_PID: u32 = 4_300_000;
const CLEAR_HINT: &str = "run 'amplihack profile rotate owner clear' to release";

fn mode(path: &Path, m: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(m)).unwrap();
}

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    proc_root: PathBuf,
    rs: RotateState,
}

/// A private root with its rotate state directory created.
fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    mode(tmp.path(), 0o700);
    let root = tmp.path().join("root");
    let proc_root = tmp.path().join("proc");
    for d in [&root, &proc_root] {
        fs::create_dir(d).unwrap();
        mode(d, 0o700);
    }
    let root = root.canonicalize().unwrap();
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    Fx {
        _tmp: tmp,
        root,
        proc_root,
        rs,
    }
}

impl Fx {
    fn marker_path(&self) -> PathBuf {
        self.rs.dir.join(owner::FILE)
    }

    /// Writes raw marker bytes as a private file, bypassing `owner::write`.
    fn raw(&self, body: &[u8]) {
        fs::write(self.marker_path(), body).unwrap();
        mode(&self.marker_path(), 0o600);
    }

    fn proc_entry(&self, pid: u32, argv: &[&str]) {
        let d = self.proc_root.join(pid.to_string());
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("cmdline"), nul_joined(argv)).unwrap();
    }

    fn cgroup(&self, body: &str) {
        let d = self.proc_root.join("self");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("cgroup"), body).unwrap();
    }
}

fn nul_joined(parts: &[&str]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in parts {
        v.extend_from_slice(p.as_bytes());
        v.push(0);
    }
    v
}

fn marker(unit: &str, pid: u32) -> Marker {
    Marker {
        unit: unit.into(),
        pid,
        since: Some(1_793_524_205),
    }
}

fn rotator_argv() -> Vec<String> {
    vec![
        format!("/usr/bin/{EXE}"),
        "profile".into(),
        "rotate".into(),
        "watch".into(),
        "--supervised=test.service".into(),
    ]
}

fn no_env(_: &str) -> Option<OsString> {
    None
}

// --- The marker: write and read ---------------------------------------------

#[test]
fn write_then_read_round_trips_with_mode_0600() {
    let f = fx();
    let m = marker("amplihack-rotator.service", 48213);
    owner::write(&f.root, &m).unwrap();
    assert_eq!(owner::read(&f.root), OwnerState::Present(m));
    let meta = fs::symlink_metadata(f.marker_path()).unwrap();
    assert!(meta.file_type().is_file());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn write_uses_the_documented_key_order() {
    let f = fx();
    owner::write(&f.root, &marker("test.service", 77)).unwrap();
    assert_eq!(
        fs::read_to_string(f.marker_path()).unwrap(),
        "unit=test.service\npid=77\nsince=1793524205\n"
    );
}

#[test]
fn write_replaces_an_earlier_marker() {
    let f = fx();
    owner::write(&f.root, &marker("a.service", 10)).unwrap();
    owner::write(&f.root, &marker("b.service", 11)).unwrap();
    assert_eq!(
        owner::read(&f.root),
        OwnerState::Present(marker("b.service", 11))
    );
}

#[test]
fn write_never_follows_a_symlink() {
    let f = fx();
    let target = f.root.join("elsewhere");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, f.marker_path()).unwrap();
    let _ = owner::write(&f.root, &marker("a.service", 10));
    assert_eq!(fs::read(&target).unwrap(), b"keep", "target untouched");
}

#[test]
fn no_marker_is_none() {
    let f = fx();
    assert_eq!(owner::read(&f.root), OwnerState::None);
}

#[test]
fn no_state_directory_is_none_and_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    mode(tmp.path(), 0o700);
    let root = tmp.path().canonicalize().unwrap();
    assert_eq!(owner::read(&root), OwnerState::None);
    assert!(!root.join(".auto-switch.d").exists());
}

#[test]
fn unknown_keys_are_ignored_and_since_is_informational() {
    let f = fx();
    for body in [
        "unit=x.service\npid=5\n",
        "unit=x.service\npid=5\nsince=abc\n",
        "unit=x.service\npid=5\nsince=1\nsince=2\n",
        "colour=blue\nunit=x.service\npid=5\n",
        "pid=5\nunit=x.service\n",
    ] {
        f.raw(body.as_bytes());
        match owner::read(&f.root) {
            OwnerState::Present(m) => {
                assert_eq!((m.unit.as_str(), m.pid), ("x.service", 5), "{body:?}")
            }
            other => panic!("{body:?}: {other:?}"),
        }
    }
}

#[test]
fn a_damaged_marker_is_unreadable() {
    let f = fx();
    let long_unit = format!("unit={}\npid=5\n", "u".repeat(257));
    let cases: Vec<&[u8]> = vec![
        b"",
        b"\n",
        b"unit=x.service\n",
        b"pid=5\n",
        b"unit=x.service\npid=5\npid=6\n",
        b"unit=x.service\nunit=y.service\npid=5\n",
        b"unit=x.service\npid=0\n",
        b"unit=x.service\npid=-1\n",
        b"unit=x.service\npid=+5\n",
        b"unit=x.service\npid= 5\n",
        b"unit=x.service\npid=5x\n",
        b"unit=x.service\npid=99999999999999999999\n",
        b"unit=\npid=5\n",
        b"unit=a=b\npid=5\n",
        b"unit=a\x1bb\npid=5\n",
        b"unit=a\tb\npid=5\n",
        b"unit=\xff\xfe\npid=5\n",
        long_unit.as_bytes(),
    ];
    for body in cases {
        f.raw(body);
        assert_eq!(
            owner::read(&f.root),
            OwnerState::Unreadable,
            "{:?}",
            String::from_utf8_lossy(body)
        );
    }
}

#[test]
fn a_unit_of_exactly_256_bytes_is_accepted() {
    let f = fx();
    let unit = "u".repeat(256);
    f.raw(format!("unit={unit}\npid=5\n").as_bytes());
    assert!(matches!(owner::read(&f.root), OwnerState::Present(m) if m.unit == unit));
}

#[test]
fn a_marker_writable_by_group_or_others_is_unreadable() {
    let f = fx();
    for m in [0o666, 0o620, 0o602] {
        f.raw(b"unit=x.service\npid=5\n");
        mode(&f.marker_path(), m);
        assert_eq!(owner::read(&f.root), OwnerState::Unreadable, "{m:o}");
    }
}

#[test]
fn a_symlinked_marker_is_unreadable_and_not_followed() {
    let f = fx();
    let target = f.root.join("target");
    fs::write(&target, b"unit=x.service\npid=5\n").unwrap();
    mode(&target, 0o600);
    std::os::unix::fs::symlink(&target, f.marker_path()).unwrap();
    assert_eq!(owner::read(&f.root), OwnerState::Unreadable);
}

#[test]
fn a_directory_at_the_marker_path_is_unreadable() {
    let f = fx();
    fs::create_dir(f.marker_path()).unwrap();
    assert_eq!(owner::read(&f.root), OwnerState::Unreadable);
}

#[test]
fn a_fifo_marker_is_unreadable_and_returns_at_once() {
    let f = fx();
    let c = std::ffi::CString::new(f.marker_path().to_str().unwrap()).unwrap();
    // SAFETY: mkfifo with a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let t = Instant::now();
    assert_eq!(owner::read(&f.root), OwnerState::Unreadable);
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "never blocks on a FIFO"
    );
}

#[test]
fn a_marker_over_4_kib_is_unreadable() {
    let f = fx();
    let mut body = b"unit=x.service\npid=5\n".to_vec();
    body.extend(std::iter::repeat_n(b'#', 5 * 1024));
    f.raw(&body);
    assert_eq!(owner::read(&f.root), OwnerState::Unreadable);
}

// --- Clearing ----------------------------------------------------------------

#[test]
fn clear_removes_a_marker_and_says_what_it_was() {
    let f = fx();
    let m = marker("test.service", 5);
    owner::write(&f.root, &m).unwrap();
    assert_eq!(owner::clear(&f.root).unwrap(), Some(OwnerState::Present(m)));
    assert!(!f.marker_path().exists());
    assert_eq!(owner::read(&f.root), OwnerState::None);
}

#[test]
fn clear_with_nothing_there_is_none() {
    let f = fx();
    assert_eq!(owner::clear(&f.root).unwrap(), None);
}

#[test]
fn clear_removes_an_unreadable_marker() {
    let f = fx();
    f.raw(b"garbage");
    assert_eq!(owner::clear(&f.root).unwrap(), Some(OwnerState::Unreadable));
    assert!(!f.marker_path().exists());
}

#[test]
fn clear_removes_a_symlink_and_leaves_its_target() {
    let f = fx();
    let target = f.root.join("target");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, f.marker_path()).unwrap();
    assert_eq!(owner::clear(&f.root).unwrap(), Some(OwnerState::Unreadable));
    assert!(
        fs::symlink_metadata(f.marker_path()).is_err(),
        "link removed"
    );
    assert_eq!(fs::read(&target).unwrap(), b"keep", "target left alone");
}

#[test]
fn clear_checked_refuses_a_root_that_is_not_private_and_keeps_the_marker() {
    let f = fx();
    owner::write(&f.root, &marker("t.service", OWNER_PID)).unwrap();
    mode(&f.root, 0o777);
    let r = owner::clear_checked(&f.root);
    mode(&f.root, 0o700);
    let err = r.expect_err("refused").to_string();
    assert!(err.contains("left in place"), "{err}");
    assert!(f.marker_path().exists(), "the marker is still there");
}

#[test]
fn clear_checked_clears_under_a_private_root() {
    let f = fx();
    let m = marker("t.service", OWNER_PID);
    owner::write(&f.root, &m).unwrap();
    assert_eq!(
        owner::clear_checked(&f.root).unwrap(),
        Some(OwnerState::Present(m))
    );
    assert!(!f.marker_path().exists());
}

#[test]
fn bidi_characters_are_refused_in_a_unit_name() {
    for c in ['\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}'] {
        let unit = format!("rot{c}.service");
        assert!(!owner::valid_unit(&unit), "{c:?}");
        let err = owner::check_flag(&Some(Some(unit)))
            .unwrap_err()
            .to_string();
        assert!(err.contains("bidi"), "{err}");
    }
    assert!(
        owner::valid_unit("rot\u{00e9}.service"),
        "other letters are fine"
    );
}

// --- Liveness ----------------------------------------------------------------

#[test]
fn a_verified_rotator_at_the_pid_is_alive() {
    let f = fx();
    let argv = rotator_argv();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    f.proc_entry(OWNER_PID, &argv);
    assert!(owner::alive(
        &marker("t.service", OWNER_PID),
        &f.proc_root,
        OsStr::new(EXE)
    ));
}

#[test]
fn a_missing_process_is_not_alive() {
    let f = fx();
    assert!(!owner::alive(
        &marker("t.service", OWNER_PID),
        &f.proc_root,
        OsStr::new(EXE)
    ));
}

#[test]
fn a_recycled_pid_running_another_program_is_not_alive() {
    let f = fx();
    f.proc_entry(OWNER_PID, &["/bin/bash", "-l"]);
    assert!(!owner::alive(
        &marker("t.service", OWNER_PID),
        &f.proc_root,
        OsStr::new(EXE)
    ));
}

#[test]
fn a_rotator_of_another_binary_is_not_alive() {
    let f = fx();
    f.proc_entry(
        OWNER_PID,
        &["/usr/bin/other-binary", "profile", "rotate", "watch"],
    );
    assert!(!owner::alive(
        &marker("t.service", OWNER_PID),
        &f.proc_root,
        OsStr::new(EXE)
    ));
}

#[test]
fn a_proc_entry_that_is_a_symlink_is_not_alive() {
    let f = fx();
    let argv = rotator_argv();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    f.proc_entry(OWNER_PID + 1, &argv);
    std::os::unix::fs::symlink(
        f.proc_root.join((OWNER_PID + 1).to_string()),
        f.proc_root.join(OWNER_PID.to_string()),
    )
    .unwrap();
    assert!(!owner::alive(
        &marker("t.service", OWNER_PID),
        &f.proc_root,
        OsStr::new(EXE)
    ));
}

#[test]
fn an_oversized_cmdline_is_not_alive() {
    let f = fx();
    let mut argv = rotator_argv();
    argv.push("x".repeat(70 * 1024));
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    f.proc_entry(OWNER_PID, &argv);
    assert!(!owner::alive(
        &marker("t.service", OWNER_PID),
        &f.proc_root,
        OsStr::new(EXE)
    ));
}

#[test]
fn a_proc_entry_owned_by_another_user_is_not_alive() {
    // A fake `/proc` cannot hold a directory of another user without root,
    // so this uses the real `/proc/1`, which belongs to root.
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let meta = fs::symlink_metadata("/proc/1").unwrap();
    assert!(!amplihack_utils::secure_files::owned_by_euid(&meta));
    assert!(!owner::alive(
        &marker("t.service", 1),
        Path::new("/proc"),
        OsStr::new(EXE)
    ));
}

// --- Unit name -----------------------------------------------------------------

#[test]
fn the_flag_wins_over_the_cgroup() {
    let f = fx();
    f.cgroup(
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/amplihack-rotator.service\n",
    );
    assert_eq!(
        owner::unit_name(Some("mine.service"), &f.proc_root),
        "mine.service"
    );
}

#[test]
fn the_unit_comes_from_a_cgroup_v2_path() {
    let f = fx();
    f.cgroup("0::/user.slice/user-1000.slice/user@1000.service/app.slice/rot.service\n");
    assert_eq!(owner::unit_name(None, &f.proc_root), "rot.service");
}

#[test]
fn the_unit_comes_from_a_cgroup_v1_path() {
    let f = fx();
    f.cgroup(
        "12:pids:/user.slice/user-1000.slice/user@1000.service/rot1.service\n\
         1:name=systemd:/user.slice/user-1000.slice/user@1000.service/rot1.service\n",
    );
    assert_eq!(owner::unit_name(None, &f.proc_root), "rot1.service");
}

#[test]
fn only_a_service_as_the_last_component_names_the_unit() {
    let f = fx();
    f.cgroup("0::/user.slice/user@1000.service/app.slice/inner.service/sub\n");
    assert_eq!(owner::unit_name(None, &f.proc_root), DEFAULT_UNIT);
    assert_eq!(owner::own_service(&f.proc_root), None);
}

#[test]
fn a_shell_in_a_scope_under_the_user_manager_has_no_service() {
    let f = fx();
    f.cgroup("0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-8b86.scope\n");
    assert_eq!(owner::own_service(&f.proc_root), None);
    assert_eq!(owner::unit_name(None, &f.proc_root), DEFAULT_UNIT);
}

#[test]
fn the_user_manager_itself_is_never_the_unit() {
    let f = fx();
    f.cgroup("0::/user.slice/user-1000.slice/user@1000.service\n");
    assert_eq!(owner::own_service(&f.proc_root), None);
    assert_eq!(owner::unit_name(None, &f.proc_root), DEFAULT_UNIT);
}

#[test]
fn no_cgroup_or_no_service_gives_the_default() {
    let f = fx();
    assert_eq!(owner::unit_name(None, &f.proc_root), DEFAULT_UNIT);
    f.cgroup("");
    assert_eq!(owner::unit_name(None, &f.proc_root), DEFAULT_UNIT);
    f.cgroup("0::/user.slice/user-1000.slice/session-3.scope\n");
    assert_eq!(owner::unit_name(None, &f.proc_root), DEFAULT_UNIT);
}

#[test]
fn the_default_unit_name() {
    assert_eq!(DEFAULT_UNIT, "amplihack-rotator.service");
}

// --- Supervised ----------------------------------------------------------------

#[test]
fn supervised_truth_table() {
    let inv = |v: &'static str| move |k: &str| (k == "INVOCATION_ID").then(|| OsString::from(v));
    let none: Option<Option<String>> = None;
    let bare: Option<Option<String>> = Some(None);
    let named: Option<Option<String>> = Some(Some("x.service".into()));
    let f = fx();
    f.cgroup("0::/user.slice/user-1000.slice/user@1000.service/app.slice/rot.service\n");
    let p = &f.proc_root;
    assert!(!owner::supervised(&none, &no_env, p));
    assert!(
        !owner::supervised(&none, &inv(""), p),
        "empty INVOCATION_ID"
    );
    assert!(owner::supervised(&none, &inv("abc123"), p));
    assert!(owner::supervised(&bare, &no_env, p));
    assert!(owner::supervised(&named, &no_env, p));
    assert!(owner::supervised(&named, &inv(""), p));
}

#[test]
fn an_inherited_invocation_id_outside_a_service_is_not_supervision() {
    let inv = |k: &str| (k == "INVOCATION_ID").then(|| OsString::from("abc123"));
    let none: Option<Option<String>> = None;
    let f = fx();
    assert!(
        !owner::supervised(&none, &inv, &f.proc_root),
        "no cgroup file"
    );
    for cgroup in [
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-8b86.scope\n",
        "0::/user.slice/user-1000.slice/user@1000.service\n",
        "0::/user.slice/user-1000.slice/session-3.scope\n",
    ] {
        f.cgroup(cgroup);
        assert!(!owner::supervised(&none, &inv, &f.proc_root), "{cgroup}");
        assert!(
            owner::supervised(&Some(None), &inv, &f.proc_root),
            "{cgroup}"
        );
    }
}

// --- The owner: status line ---------------------------------------------------

#[test]
fn describe_without_a_marker() {
    let f = fx();
    assert_eq!(
        owner::describe(&f.root, &f.proc_root, Some(OsStr::new(EXE))),
        "owner: none"
    );
}

#[test]
fn describe_a_running_owner() {
    let f = fx();
    let argv = rotator_argv();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    f.proc_entry(OWNER_PID, &argv);
    owner::write(&f.root, &marker("amplihack-rotator.service", OWNER_PID)).unwrap();
    assert_eq!(
        owner::describe(&f.root, &f.proc_root, Some(OsStr::new(EXE))),
        format!(
            "owner: amplihack-rotator.service pid {OWNER_PID} (running); autostart defers to it"
        )
    );
}

#[test]
fn describe_a_stopped_owner() {
    let f = fx();
    owner::write(&f.root, &marker("amplihack-rotator.service", OWNER_PID)).unwrap();
    assert_eq!(
        owner::describe(&f.root, &f.proc_root, Some(OsStr::new(EXE))),
        format!(
            "owner: amplihack-rotator.service pid {OWNER_PID} (not running); autostart defers to it; {CLEAR_HINT}"
        )
    );
}

#[test]
fn describe_without_the_executable_name_does_not_claim_the_owner_stopped() {
    let f = fx();
    owner::write(&f.root, &marker("amplihack-rotator.service", OWNER_PID)).unwrap();
    assert_eq!(
        owner::describe(&f.root, &f.proc_root, None),
        format!(
            "owner: amplihack-rotator.service pid {OWNER_PID} (liveness unknown); autostart defers to it"
        )
    );
}

#[test]
fn describe_an_unreadable_marker_never_echoes_its_bytes() {
    let f = fx();
    f.raw(b"unit=SECRET\x1b[31m\npid=nope\n");
    let line = owner::describe(&f.root, &f.proc_root, Some(OsStr::new(EXE)));
    assert_eq!(
        line,
        format!("owner: unreadable; autostart defers to it; {CLEAR_HINT}")
    );
    assert!(!line.contains("SECRET"));
}

#[test]
fn describe_does_not_read_under_a_root_that_is_not_private() {
    let f = fx();
    owner::write(&f.root, &marker("a.service", 5)).unwrap();
    mode(&f.root, 0o757);
    let line = owner::describe(&f.root, &f.proc_root, Some(OsStr::new(EXE)));
    mode(&f.root, 0o700);
    assert_eq!(line, "owner: not read (root is not private)");
}

#[test]
fn describe_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    mode(tmp.path(), 0o700);
    let root = tmp.path().canonicalize().unwrap();
    assert_eq!(
        owner::describe(&root, &root.join("proc"), Some(OsStr::new(EXE))),
        "owner: none"
    );
    assert!(!root.join(".auto-switch.d").exists());
}

// --- The change log -------------------------------------------------------------

fn caller(parent: Option<&[&str]>) -> Caller {
    Caller {
        pid: 51877,
        ppid: 51702,
        parent: parent.map(nul_joined),
        recipe_run: None,
        invocation: None,
    }
}

#[test]
fn a_change_line_has_the_documented_form() {
    let c = caller(Some(&["bash", "/home/you/bin/pause-rotation.sh"]));
    assert_eq!(
        change_log::line("auto-switch", "on", "off", &c),
        "change: auto-switch on -> off by pid=51877 ppid=51702 parent=\"bash /home/you/bin/pause-rotation.sh\""
    );
}

#[test]
fn run_ids_follow_the_parent_in_order() {
    let mut c = caller(Some(&["-bash"]));
    c.recipe_run = Some("3f9c2a1e".into());
    c.invocation = Some("abc".into());
    assert_eq!(
        change_log::line("rotate-autostart", "unset", "off", &c),
        "change: rotate-autostart unset -> off by pid=51877 ppid=51702 parent=\"-bash\" recipe_run=3f9c2a1e invocation=abc"
    );
    c.recipe_run = None;
    assert!(
        change_log::line("rotate-autostart", "off", "on", &c)
            .ends_with("parent=\"-bash\" invocation=abc")
    );
}

#[test]
fn an_unreadable_parent_is_a_dash() {
    assert!(
        change_log::line("auto-switch", "corrupt", "on", &caller(None))
            .ends_with("ppid=51702 parent=\"-\"")
    );
}

#[test]
fn the_parent_is_cut_to_200_characters_on_a_char_boundary() {
    let long = "é".repeat(300);
    let line = change_log::line("auto-switch", "on", "off", &caller(Some(&[&long])));
    let parent = line
        .split_once("parent=\"")
        .unwrap()
        .1
        .strip_suffix('"')
        .unwrap();
    assert_eq!(parent.chars().count(), 200, "{parent}");
    assert!(parent.chars().all(|ch| ch == 'é'));
}

#[test]
fn quotes_in_the_parent_become_apostrophes() {
    let line = change_log::line(
        "auto-switch",
        "on",
        "off",
        &caller(Some(&["sh", "-c", "echo \"hi\" x=\"\""])),
    );
    let parent = line.split_once("parent=\"").unwrap().1;
    assert_eq!(
        parent.matches('"').count(),
        1,
        "only the closing quote: {line}"
    );
    assert!(parent.contains("echo 'hi'"), "{line}");
}

#[test]
fn injected_newlines_and_controls_make_exactly_one_line() {
    let evil = "a\nb\r2026-11-01T00:00:00Z change: auto-switch off -> on\u{202e}\u{1b}[31m";
    let mut c = caller(Some(&["sh", evil]));
    c.recipe_run = Some("r1\nchange: x".into());
    c.invocation = Some("i\r\u{202e}".into());
    let line = change_log::line("auto-switch", "on", "off", &c);
    assert_eq!(line.lines().count(), 1, "{line:?}");
    assert!(!line.contains('\r') && !line.contains('\u{1b}') && !line.contains('\u{202e}'));
    assert!(
        line.starts_with("change: auto-switch on -> off by "),
        "{line:?}"
    );
    assert_eq!(
        line.matches(" by pid=").count(),
        1,
        "no forged field: {line:?}"
    );
}

#[test]
fn run_ids_have_spaces_replaced_and_are_cut_to_64() {
    let mut c = caller(Some(&["x"]));
    c.recipe_run = Some(format!("a b {}", "z".repeat(100)));
    c.invocation = Some("inv id pid=1".into());
    let line = change_log::line("auto-switch", "on", "off", &c);
    let rr = line.split_once(" recipe_run=").unwrap().1;
    let rr = rr.split(' ').next().unwrap();
    assert!(rr.starts_with("a_b_"), "{line}");
    assert_eq!(rr.chars().count(), 64, "{line}");
    assert!(line.ends_with(" invocation=inv_id_pid=1"), "{line}");
}

#[test]
fn the_parent_command_line_is_redacted() {
    let line = change_log::line(
        "auto-switch",
        "on",
        "off",
        &caller(Some(&[
            "claude",
            "--api-key",
            "sk-abc",
            "TOKEN=xyz",
            "https://u:p@h/x",
        ])),
    );
    assert!(
        line.ends_with("parent=\"claude --api-key *** TOKEN=*** https://h/x\""),
        "{line}"
    );
    for secret in ["sk-abc", "xyz", "u:p@"] {
        assert!(!line.contains(secret), "{secret} leaked: {line}");
    }
}

#[test]
fn every_secret_flag_form_is_redacted() {
    let cases: &[(&[&str], &str)] = &[
        (&["x", "--token", "t1"], "x --token ***"),
        (&["x", "--auth-header", "h1"], "x --auth-header ***"),
        (&["x", "--client-secret", "s1"], "x --client-secret ***"),
        (&["x", "--password", "p1"], "x --password ***"),
        (&["x", "--api-key=k1"], "x --api-key=***"),
        (&["x", "--Password=P1"], "x --Password=***"),
        (&["x", "-token", "t2"], "x -token ***"),
        (&["env", "A=1", "B=two", "cmd"], "env A=*** B=*** cmd"),
        (
            &["git", "clone", "https://user:pw@example.org/r"],
            "git clone https://example.org/r",
        ),
        (&["x", "--interval", "120"], "x --interval 120"),
        (
            &["curl", "-H", "Authorization: Bearer sk-x", "u"],
            "curl -H Authorization: *** u",
        ),
        (
            &["curl", "-H", "X-Api-Key: k3", "u"],
            "curl -H X-Api-Key: *** u",
        ),
        (
            &["curl", "-H", "Content-Type: json", "u"],
            "curl -H Content-Type: json u",
        ),
        (
            &["curl", "-H", "Authorization:Bearer sk-y", "u"],
            "curl -H Authorization: *** u",
        ),
        (&["curl", "-u", "user:pw1", "u"], "curl -u *** u"),
        (&["curl", "--user", "user:pw2", "u"], "curl --user *** u"),
        (&["curl", "--user=user:pw3", "u"], "curl --user=*** u"),
        (&["curl", "-uuser:pw4", "u"], "curl -u*** u"),
        (&["git", "push", "-u", "origin"], "git push -u origin"),
        (&["mysql", "-psecretpw", "db"], "mysql -p*** db"),
        (&["ss", "-p", "8080"], "ss -p 8080"),
        (&["x", "--pass", "p2"], "x --pass ***"),
        (&["x", "--passwd", "p3"], "x --passwd ***"),
        (&["x", "--db-pwd=p4"], "x --db-pwd=***"),
        (&["x", "--bypass", "y"], "x --bypass y"),
        (
            &["curl", "https://h/x?a=1&access_token=t9#f"],
            "curl https://h/x?a=1&access_token=***#f",
        ),
        (
            &["az", "https://h/c?sv=1&sig=s9"],
            "az https://h/c?sv=1&sig=***",
        ),
        (
            &["curl", "https://h/cb#access_token=t10"],
            "curl https://h/cb#access_token=***",
        ),
        (
            &["aws", "https://b/o?X-Amz-Signature=s10"],
            "aws https://b/o?X-Amz-Signature=***",
        ),
        (&["x", "oauth://h/x"], "x oauth://h/x"),
    ];
    for (argv, want) in cases {
        let line = change_log::line("auto-switch", "on", "off", &caller(Some(argv)));
        assert!(
            line.ends_with(&format!("parent=\"{want}\"")),
            "{argv:?}: {line}"
        );
    }
}

#[test]
fn caller_reads_the_parent_from_the_given_proc_root() {
    let f = fx();
    // SAFETY: getppid has no preconditions.
    let ppid = unsafe { libc::getppid() } as u32;
    f.proc_entry(ppid, &["parent-shell", "--flag"]);
    let env = |k: &str| match k {
        "AMPLIHACK_RECIPE_RUN_ID" => Some(OsString::from("run-7")),
        "INVOCATION_ID" => Some(OsString::from("")),
        _ => None,
    };
    let c = change_log::caller(&env, &f.proc_root);
    assert_eq!(c.pid, std::process::id());
    assert_eq!(c.ppid, ppid);
    assert_eq!(
        c.parent.as_deref(),
        Some(nul_joined(&["parent-shell", "--flag"]).as_slice())
    );
    assert_eq!(c.recipe_run.as_deref(), Some("run-7"));
    assert_eq!(c.invocation, None, "an empty INVOCATION_ID is unset");
}

#[test]
fn caller_without_a_proc_entry_has_no_parent() {
    let f = fx();
    let c = change_log::caller(&no_env, &f.proc_root);
    assert_eq!(c.parent, None);
    assert_eq!(c.recipe_run, None);
    assert_eq!(c.invocation, None);
}

#[test]
fn record_appends_one_timed_private_line() {
    let f = fx();
    // `record` reads the run ids from the real environment, which holds them
    // when the tests themselves run inside a recipe run or a systemd unit.
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let ids = ["AMPLIHACK_RECIPE_RUN_ID", "INVOCATION_ID"];
    let _restore = crate::test_support::EnvGuard::capture(&ids);
    let _unset = crate::test_support::EnvGuard::unset(ids);
    change_log::record(&f.root, "rotate-autostart", "on", "off");
    change_log::record(&f.root, "rotate-autostart", "off", "on");
    let log = fs::read_to_string(&f.rs.log).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    let re = regex::Regex::new(
        r#"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z change: rotate-autostart on -> off by pid=\d+ ppid=\d+ parent=".*"$"#,
    )
    .unwrap();
    assert!(re.is_match(lines[0]), "{}", lines[0]);
    assert!(lines[1].contains(" change: rotate-autostart off -> on by "));
    let m = fs::metadata(&f.rs.log).unwrap().permissions().mode() & 0o777;
    assert_eq!(m, 0o600);
}

#[test]
fn record_never_panics_on_a_root_it_cannot_write() {
    let tmp = tempfile::tempdir().unwrap();
    mode(tmp.path(), 0o700);
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    mode(&root, 0o757);
    change_log::record(&root, "auto-switch", "on", "off");
    mode(&root, 0o700);
    assert!(!root.join(".auto-switch.d/rotate/rotate.log").exists());
}

// --- Source guards ----------------------------------------------------------------

fn source(name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/commands/profile/rotate")
        .join(name);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The code (comments removed) of `fn <name>` up to its closing brace at
/// column 0.
fn fn_body(text: &str, name: &str) -> String {
    let start = text
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let rest = &text[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |i| i + 3);
    rest[..end]
        .lines()
        .map(|l| l.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text of the `{ .. }` block that opens at the first `{` after `at`.
fn block_after<'a>(text: &'a str, at: &str) -> &'a str {
    let i = text.find(at).unwrap_or_else(|| panic!("`{at}` not found"));
    let open = i + text[i..].find('{').unwrap();
    let mut depth = 0usize;
    for (j, ch) in text[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open..=open + j];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced block after `{at}`")
}

#[test]
fn spawn_removes_invocation_id_from_the_child() {
    let body = fn_body(&source("daemon.rs"), "spawn");
    assert!(
        body.contains("env_remove(\"INVOCATION_ID\")"),
        "spawn() must strip INVOCATION_ID so a spawned rotator never claims ownership"
    );
}

#[test]
fn the_exhausted_alert_is_logged_outside_the_idle_latch() {
    let body = fn_body(&source("daemon.rs"), "watch");
    assert!(
        body.contains("exhausted::watch_line"),
        "watch checks the exhausted-while-off condition"
    );
    let latch = block_after(&body, "else if !idle");
    assert!(
        !latch.contains("exhausted::watch_line"),
        "the ALERT is written on every paused cycle, not once per idle latch"
    );
}

#[test]
fn a_spawned_watch_never_writes_the_marker() {
    let body = fn_body(&source("daemon.rs"), "watch");
    let unspawned = block_after(&body, "if !spawned");
    assert!(
        unspawned.contains("owner::write"),
        "the marker is written only in the !spawned branch"
    );
    assert_eq!(
        body.matches("owner::write").count(),
        1,
        "no other path writes the marker"
    );
    assert!(
        !body.contains("owner::clear"),
        "a watch never removes the marker"
    );
}

#[test]
fn the_new_modules_never_unwrap_or_expect() {
    for name in ["owner.rs", "change_log.rs", "exhausted.rs"] {
        let text = source(name);
        let code = text.split("#[cfg(test)]").next().unwrap_or_default();
        for (i, line) in code.lines().enumerate() {
            let line = line.split("//").next().unwrap_or_default();
            assert!(
                !line.contains(".unwrap(") && !line.contains(".expect("),
                "{name}:{}: unwrap or expect outside tests",
                i + 1
            );
        }
    }
}

#[test]
fn only_owner_rs_and_its_cli_touch_the_marker_file() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/rotate");
    for e in fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("tests_") || name == "owner.rs" || !name.ends_with(".rs") {
            continue;
        }
        let text = fs::read_to_string(&p).unwrap();
        assert!(
            !text.contains("join(\"owner\")") && !text.contains("owner::clear("),
            "{name} touches the owner marker directly; only owner.rs may"
        );
    }
}
