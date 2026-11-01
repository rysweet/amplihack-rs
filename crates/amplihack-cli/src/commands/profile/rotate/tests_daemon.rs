//! `rotate::daemon` pid-file and process verification, and the prototype
//! lock check from `rotate::cycle` (D17, D18, issue #1592). Nothing here
//! sends a signal.
//!
//! Contract:
//! - `parse_pid(bytes: &[u8], own: u32) -> Option<u32>` (shared with the
//!   limit-resume hook in `amplihack_utils::rotator_status`): at most 32
//!   bytes of ASCII digits (one trailing newline allowed), value > 1, not
//!   `own`.
//! - `is_rotator(cmdline: &[u8], Some(exe_name)) -> bool` (shared too):
//!   NUL-separated argv containing `profile`, `rotate` and `watch`, with
//!   `argv[0]`'s file name equal to `exe_name`.
//! - `daemon::state_at(&RotateState, proc_root: &Path, exe_name: &OsStr) ->
//!   DaemonState::{Running(u32), Stopped, Stale, Unverifiable}`; read-only.
//! - `cycle::prototype_guard(root: &Path) -> Result<(), ProfileError>`.

use super::cycle::prototype_guard;
use super::daemon::{DaemonState, state_at};
use amplihack_utils::rotator_status::parse_pid;

fn is_rotator(cmdline: &[u8], exe_name: &OsStr) -> bool {
    amplihack_utils::rotator_status::is_rotator(cmdline, Some(exe_name))
}
use super::ledger::RotateState;
use crate::commands::profile::ProfileError;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

const OWN: u32 = 4242;
const EXE: &str = "amplihack";

#[test]
fn valid_pid_files() {
    assert_eq!(parse_pid(b"48213", OWN), Some(48213));
    assert_eq!(parse_pid(b"48213\n", OWN), Some(48213));
    assert_eq!(parse_pid(b"2", OWN), Some(2));
}

#[test]
fn invalid_pid_files() {
    for bad in [
        &b""[..],
        b"0",
        b"1",
        b"4242",
        b"-5",
        b"+5",
        b" 5",
        b"12abc",
        b"5\n\n",
        b"99999999999999999999",
        &[b'7'; 40][..],
        b"\xff\xfe",
    ] {
        assert_eq!(
            parse_pid(bad, OWN),
            None,
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

fn argv(parts: &[&str]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in parts {
        v.extend_from_slice(p.as_bytes());
        v.push(0);
    }
    v
}

#[test]
fn a_rotator_command_line_is_recognised() {
    let exe = OsStr::new(EXE);
    assert!(is_rotator(
        &argv(&[
            "/usr/local/bin/amplihack",
            "profile",
            "rotate",
            "watch",
            "--interval",
            "600",
            "--spawned"
        ]),
        exe
    ));
    assert!(is_rotator(
        &argv(&["amplihack", "profile", "rotate", "watch"]),
        exe
    ));
}

#[test]
fn other_command_lines_are_not_rotators() {
    let exe = OsStr::new(EXE);
    for parts in [
        &["sleep", "profile", "rotate", "watch"][..],
        &["/bin/amplihack-other", "profile", "rotate", "watch"],
        &["/bin/amplihack", "profile", "rotate", "status"],
        &["/bin/amplihack", "profile", "usage"],
        &["/bin/amplihack", "profile rotate watch"],
        &[],
    ] {
        assert!(!is_rotator(&argv(parts), exe), "{parts:?}");
    }
}

struct Fake {
    _tmp: tempfile::TempDir,
    rs: RotateState,
    proc_root: PathBuf,
}

fn fake() -> Fake {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(tmp.path());
    rs.ensure().unwrap();
    let proc_root = tmp.path().join("proc");
    fs::create_dir(&proc_root).unwrap();
    Fake {
        _tmp: tmp,
        rs,
        proc_root,
    }
}

impl Fake {
    fn pid_file(&self, body: &str) {
        fs::write(&self.rs.pid, body).unwrap();
        fs::set_permissions(&self.rs.pid, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn process(&self, pid: u32, parts: &[&str]) {
        let d = self.proc_root.join(pid.to_string());
        fs::create_dir(&d).unwrap();
        fs::write(d.join("cmdline"), argv(parts)).unwrap();
    }

    fn state(&self) -> DaemonState {
        state_at(&self.rs, &self.proc_root, OsStr::new(EXE))
    }
}

#[test]
fn no_pid_file_is_stopped() {
    let f = fake();
    assert_eq!(f.state(), DaemonState::Stopped);
}

#[test]
fn a_verified_rotator_is_running() {
    let f = fake();
    f.pid_file("31337\n");
    f.process(
        31337,
        &["/opt/amplihack", "profile", "rotate", "watch", "--spawned"],
    );
    assert_eq!(f.state(), DaemonState::Running(31337));
}

#[test]
fn a_dead_or_foreign_process_or_bad_pid_file_is_stale() {
    let f = fake();
    f.pid_file("31337");
    assert_eq!(f.state(), DaemonState::Stale, "no such process");
    f.process(31337, &["sleep", "30"]);
    assert_eq!(f.state(), DaemonState::Stale, "not a rotator");
    for body in ["0", "1", "-5", "12abc", &"7".repeat(40)] {
        f.pid_file(body);
        assert_eq!(f.state(), DaemonState::Stale, "{body}");
    }
}

#[test]
fn state_is_read_only() {
    let f = fake();
    f.pid_file("31337");
    let _ = f.state();
    assert!(f.rs.pid.exists(), "status never removes the pid file");
}

#[test]
fn no_proc_is_unverifiable() {
    let f = fake();
    f.pid_file("31337");
    let missing = f.proc_root.join("missing");
    assert_eq!(
        state_at(&f.rs, &missing, OsStr::new(EXE)),
        DaemonState::Unverifiable
    );
}

// --- Prototype coexistence -------------------------------------------------

fn rotator_dir(root: &Path) -> PathBuf {
    let d = root.join(".rotator");
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_missing_prototype_lock_passes_and_is_not_created() {
    let tmp = tempfile::tempdir().unwrap();
    prototype_guard(tmp.path()).unwrap();
    assert!(!tmp.path().join(".rotator").exists());
    let d = rotator_dir(tmp.path());
    prototype_guard(tmp.path()).unwrap();
    assert_eq!(fs::read_dir(&d).unwrap().count(), 0, "nothing created");
}

#[test]
fn a_free_prototype_lock_passes_and_is_left_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = rotator_dir(tmp.path()).join("lock");
    fs::write(&lock, "prototype").unwrap();
    prototype_guard(tmp.path()).unwrap();
    assert_eq!(fs::read_to_string(&lock).unwrap(), "prototype");
}

#[test]
fn a_held_prototype_lock_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = rotator_dir(tmp.path()).join("lock");
    fs::write(&lock, "").unwrap();
    let holder = fs::File::open(&lock).unwrap();
    // SAFETY: valid open descriptor.
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(matches!(
        prototype_guard(tmp.path()),
        Err(ProfileError::RotatePrototypeRunning)
    ));
    drop(holder);
    prototype_guard(tmp.path()).unwrap();
}

#[test]
fn a_fifo_prototype_lock_does_not_hang() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = rotator_dir(tmp.path()).join("lock");
    let c = std::ffi::CString::new(lock.to_str().unwrap()).unwrap();
    // SAFETY: valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let _ = prototype_guard(tmp.path());
    assert!(fs::symlink_metadata(&lock).unwrap().file_type().is_fifo());
}

#[test]
fn a_symlinked_prototype_lock_is_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    fs::write(&target, "").unwrap();
    let holder = fs::File::open(&target).unwrap();
    // SAFETY: valid open descriptor.
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    std::os::unix::fs::symlink(&target, rotator_dir(tmp.path()).join("lock")).unwrap();
    assert!(
        !matches!(
            prototype_guard(tmp.path()),
            Err(ProfileError::RotatePrototypeRunning)
        ),
        "O_NOFOLLOW: the link's target is never locked"
    );
}

// ---------------------------------------------------------------------------
// Issue #1690: the CI leak scan and the spawner agree on the daemon's argv.
// ---------------------------------------------------------------------------

const LEAK_SCAN: &str = include_str!("../../../../../../scripts/check-leaked-rotators.sh");

/// `scripts/check-leaked-rotators.sh` reports a process only when its argv
/// holds each of these as an exact entry. If `daemon::spawned_args` stops
/// producing one of them, the scan goes blind and CI stays green while
/// daemons leak; this test fails first.
#[test]
fn the_leak_scan_matches_the_argv_spawn_builds() {
    let s = super::settings::resolve(&super::TuneFlags::default(), None, &|_: &str| None)
        .expect("default settings");
    let args = super::daemon::spawned_args(&s);

    // The argv is a real command line: it parses as `profile rotate watch
    // --spawned`, so the markers below are what the daemon really runs with.
    let cli =
        crate::Cli::try_parse_from(std::iter::once("amplihack".to_string()).chain(args.clone()))
            .unwrap_or_else(|e| panic!("spawned argv {args:?} does not parse: {e}"));
    match cli.command {
        crate::Commands::Profile(crate::commands::profile::ProfileCommands::Rotate(r)) => {
            match r.action {
                Some(super::RotateAction::Watch(w)) => assert!(w.spawned, "{args:?}"),
                other => panic!("spawned argv {args:?} parsed as {other:?}, not `watch`"),
            }
        }
        _ => panic!("spawned argv {args:?} is not `profile rotate`"),
    }

    for marker in ["profile", "rotate", "watch", "--spawned"] {
        assert!(
            args.iter().any(|a| a == marker),
            "spawned argv {args:?} lacks `{marker}`, which scripts/check-leaked-rotators.sh matches on"
        );
        // The `case` arm for this marker, whatever its indentation, must set
        // the flag named after it (`--spawned` sets `have_spawned`).
        let flag = marker.trim_start_matches('-');
        let arm = format!("{marker}) have_{flag}=1");
        assert!(
            LEAK_SCAN.lines().any(|l| l.trim_start().starts_with(&arm)),
            "scripts/check-leaked-rotators.sh no longer matches `{marker}` with `{arm}`"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #1844 (SR-9): a stopping rotator waits for token refreshes in
// progress, so it never leaves Claude's `.oauth_refresh.lock` behind.
//
// Contract:
// - `daemon::refresh_grace(timeout) = timeout + 1 s`.
// - `daemon::finish_refreshes(grace) -> bool` waits until no refresh holds
//   a lock (`usage_direct::refreshes_in_progress() == 0`) or `grace` ends,
//   and says whether none is left. It never sets `PROBE_CANCEL`.
// ---------------------------------------------------------------------------

mod issue_1844 {
    use super::super::daemon::{finish_refreshes, refresh_grace};
    use crate::commands::profile::tests_fake_http::{FakeHttp, Reply};
    use crate::commands::profile::usage_direct::{Endpoints, Request, read};
    use serde_json::json;
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    #[test]
    fn the_grace_is_the_deadline_plus_one_second() {
        assert_eq!(
            refresh_grace(Duration::from_secs(120)),
            Duration::from_secs(121)
        );
        assert_eq!(
            refresh_grace(Duration::from_secs(1)),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn stopping_during_a_stalled_refresh_leaves_no_refresh_lock() {
        let now = chrono::Utc::now().timestamp();
        let iso = |t: i64| {
            chrono::DateTime::from_timestamp(t, 0)
                .unwrap()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        };
        let body = json!({
            "five_hour": {"utilization": 5.0, "resets_at": iso(now + 3600)},
            "seven_day": {"utilization": 6.0, "resets_at": iso(now + 86_400)}
        });
        let fake = FakeHttp::start(move |s, _| {
            if s.method == "POST" {
                Reply::json(
                    200,
                    &json!({"access_token": "FAKE-NEW-1844-d", "expires_in": 3600}),
                )
                .after(Duration::from_millis(1200))
            } else if s.header("authorization") == Some("Bearer FAKE-NEW-1844-d") {
                Reply::json(200, &body)
            } else {
                Reply::json(401, &json!({}))
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("spare");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let creds = dir.join(".credentials.json");
        fs::write(
            &creds,
            json!({"claudeAiOauth": {
                "accessToken": "FAKE-OLD-1844-d",
                "refreshToken": "FAKE-REFRESH-1844-d",
                "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000
            }})
            .to_string(),
        )
        .unwrap();
        fs::set_permissions(&creds, fs::Permissions::from_mode(0o600)).unwrap();

        let (base, token) = (fake.base(), fake.token_url());
        let reader_dir = dir.clone();
        let reader = std::thread::spawn(move || {
            let (ep, _) = Endpoints::resolve(Some(OsStr::new(&base)), Some(OsStr::new(&token)));
            let journals = Vec::new();
            let req = Request {
                creds_dir: &reader_dir,
                journals: &journals,
                allow_refresh: true,
                timeout: Duration::from_secs(5),
                now_ms: chrono::Utc::now().timestamp_millis(),
            };
            read(&ep, &req).is_ok()
        });
        let start = Instant::now();
        while fake.count("POST", "/v1/oauth/token") == 0 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "no refresh started"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(dir.join(".oauth_refresh.lock").is_dir());
        // The daemon stops now.
        assert!(finish_refreshes(refresh_grace(Duration::from_secs(5))));
        assert!(!dir.join(".oauth_refresh.lock").exists());
        assert!(reader.join().unwrap());
    }
}
