//! `rotate::sessions_daily` (issue #1746): the rotator's watch loop archives
//! old sessions once a day.
//!
//! Contract:
//! - `tick_with(rs: &RotateState, home: &Path, now: i64,
//!   env: &dyn Fn(&str) -> Option<OsString>, log: &mut dyn FnMut(&str),
//!   run: &mut dyn FnMut(&Opts) -> Result<String, String>)`: `run` stands
//!   in for `archive::run_default` and returns the summary line or the
//!   error line. Never panics, never returns an error.
//! - Switch: `AMPLIHACK_SESSIONS_ARCHIVE_DAILY` through
//!   `autostart::parse_env`; off (`0`/`off`) means nothing runs and nothing
//!   is written. An invalid value runs, and logs
//!   `sessions archive: ignored AMPLIHACK_SESSIONS_ARCHIVE_DAILY value (use
//!   on, off, 1 or 0); running` (at most once per process).
//! - Record: `<rs.dir>/sessions-archive.last`, decimal epoch seconds. Due
//!   when missing or garbage, when `now − last ≥ 86400`, or when
//!   `last > now`. Written (0600) before `run`; if that write fails, `run`
//!   is not called.
//! - Every outcome is logged as `sessions archive: <line>`.
//! - Days and DEST come from `config::resolve` with the injected `env`
//!   (no flags), `apply` on.

use super::ledger::RotateState;
use super::sessions_daily::tick_with;
use crate::commands::sessions::config::Opts;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const DAY: i64 = 86_400;
const NOW: i64 = 1_793_314_037;

struct T {
    _tmp: tempfile::TempDir,
    rs: RotateState,
    home: PathBuf,
}

fn setup() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
    let root = base.join("profiles");
    let home = base.join("home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    T {
        _tmp: tmp,
        rs,
        home,
    }
}

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let m: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |k| m.get(k).cloned()
}

fn record(t: &T) -> PathBuf {
    t.rs.dir.join("sessions-archive.last")
}

/// Runs one tick; returns (log lines, opts passed to each run).
fn tick(
    t: &T,
    now: i64,
    e: &dyn Fn(&str) -> Option<OsString>,
    result: Result<&str, &str>,
) -> (Vec<String>, Vec<Opts>) {
    let mut logs = Vec::new();
    let mut runs = Vec::new();
    let mut log = |l: &str| logs.push(l.to_string());
    let mut run = |o: &Opts| {
        runs.push(o.clone());
        result.map(str::to_string).map_err(str::to_string)
    };
    tick_with(&t.rs, &t.home, now, e, &mut log, &mut run);
    (logs, runs)
}

const SUMMARY: &str =
    "archived 3 sessions, 0.00 GiB (older than 3 days; 0 live sessions kept; 0 failed)";

#[test]
fn first_tick_runs_logs_and_writes_the_record() {
    let t = setup();
    let (logs, runs) = tick(&t, NOW, &env(&[]), Ok(SUMMARY));
    assert_eq!(runs.len(), 1);
    assert!(runs[0].apply);
    assert_eq!(runs[0].days, 3);
    assert_eq!(runs[0].dest, t.home.join(".amplihack/session-archive"));
    assert_eq!(logs, [format!("sessions archive: {SUMMARY}")]);
    assert_eq!(
        fs::read_to_string(record(&t)).unwrap().trim(),
        NOW.to_string()
    );
    assert_eq!(fs::metadata(record(&t)).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn runs_at_most_once_a_day() {
    let t = setup();
    tick(&t, NOW, &env(&[]), Ok(SUMMARY));
    let (logs, runs) = tick(&t, NOW + DAY - 1, &env(&[]), Ok(SUMMARY));
    assert!(runs.is_empty() && logs.is_empty());
    let (_, runs) = tick(&t, NOW + DAY, &env(&[]), Ok(SUMMARY));
    assert_eq!(runs.len(), 1);
}

#[test]
fn clock_moved_back_is_due() {
    let t = setup();
    tick(&t, NOW, &env(&[]), Ok(SUMMARY));
    let (_, runs) = tick(&t, NOW - 10, &env(&[]), Ok(SUMMARY));
    assert_eq!(runs.len(), 1);
    assert_eq!(
        fs::read_to_string(record(&t)).unwrap().trim(),
        (NOW - 10).to_string()
    );
}

#[test]
fn garbage_or_extreme_records_are_due_and_never_panic() {
    for raw in [
        "",
        "junk",
        "-5",
        "1.5",
        "99999999999999999999999",
        &i64::MIN.to_string(),
        &i64::MAX.to_string(),
    ] {
        let t = setup();
        fs::write(record(&t), raw).unwrap();
        fs::set_permissions(record(&t), fs::Permissions::from_mode(0o600)).unwrap();
        let (_, runs) = tick(&t, NOW, &env(&[]), Ok(SUMMARY));
        assert_eq!(runs.len(), 1, "record {raw:?}");
    }
    for now in [i64::MIN, i64::MAX, 0, -1] {
        let t = setup();
        fs::write(record(&t), "100").unwrap();
        tick(&t, now, &env(&[]), Ok(SUMMARY));
    }
}

#[test]
fn switch_off_does_nothing() {
    for off in ["0", "off", "OFF"] {
        let t = setup();
        let (logs, runs) = tick(
            &t,
            NOW,
            &env(&[("AMPLIHACK_SESSIONS_ARCHIVE_DAILY", off)]),
            Ok(SUMMARY),
        );
        assert!(runs.is_empty() && logs.is_empty(), "{off}");
        assert!(!record(&t).exists());
    }
}

#[test]
fn switch_on_or_empty_runs() {
    for on in ["1", "on", ""] {
        let t = setup();
        let (_, runs) = tick(
            &t,
            NOW,
            &env(&[("AMPLIHACK_SESSIONS_ARCHIVE_DAILY", on)]),
            Ok(SUMMARY),
        );
        assert_eq!(runs.len(), 1, "{on:?}");
    }
}

#[test]
fn invalid_switch_runs_and_says_so_without_the_value() {
    let t = setup();
    let (logs, runs) = tick(
        &t,
        NOW,
        &env(&[("AMPLIHACK_SESSIONS_ARCHIVE_DAILY", "maybe-SECRET")]),
        Ok(SUMMARY),
    );
    assert_eq!(runs.len(), 1);
    // Logged at most once per process, so another test may have logged it.
    for l in &logs {
        assert!(!l.contains("SECRET"), "{l}");
    }
    let ignored = "sessions archive: ignored AMPLIHACK_SESSIONS_ARCHIVE_DAILY value (use on, off, 1 or 0); running";
    assert!(
        logs.iter()
            .all(|l| l == ignored || l == &format!("sessions archive: {SUMMARY}")),
        "{logs:?}"
    );
}

#[test]
fn days_and_dest_variables_are_used() {
    let t = setup();
    let e = env(&[
        ("AMPLIHACK_SESSIONS_ARCHIVE_DAYS", "9"),
        ("AMPLIHACK_SESSIONS_ARCHIVE_DEST", "/var/archive"),
    ]);
    let (_, runs) = tick(&t, NOW, &e, Ok(SUMMARY));
    assert_eq!(runs[0].days, 9);
    assert_eq!(runs[0].dest, Path::new("/var/archive"));
}

#[test]
fn bad_days_variable_is_logged_and_still_postpones() {
    let t = setup();
    let (logs, runs) = tick(
        &t,
        NOW,
        &env(&[("AMPLIHACK_SESSIONS_ARCHIVE_DAYS", "zero")]),
        Ok(SUMMARY),
    );
    assert!(runs.is_empty());
    assert_eq!(
        logs,
        ["sessions archive: AMPLIHACK_SESSIONS_ARCHIVE_DAYS must be a whole number of at least 1"]
    );
    assert!(record(&t).exists(), "a config error retries every tick");
}

#[test]
fn failure_is_logged_and_the_next_try_is_a_day_later() {
    let t = setup();
    let lock = "another sessions archive run is in progress (lock: /d/.archive.lock)";
    let (logs, _) = tick(&t, NOW, &env(&[]), Err(lock));
    assert_eq!(logs, [format!("sessions archive: {lock}")]);
    let (_, runs) = tick(&t, NOW + 60, &env(&[]), Ok(SUMMARY));
    assert!(runs.is_empty());
}

#[test]
fn record_is_written_before_the_run() {
    let t = setup();
    let path = record(&t);
    let mut seen = None;
    let mut log = |_: &str| {};
    let mut run = |_: &Opts| {
        seen = fs::read_to_string(&path).ok();
        Ok(SUMMARY.to_string())
    };
    tick_with(&t.rs, &t.home, NOW, &env(&[]), &mut log, &mut run);
    assert_eq!(
        seen.as_deref().map(str::trim),
        Some(NOW.to_string().as_str())
    );
}

#[test]
fn record_write_failure_means_no_run() {
    let t = setup();
    // A directory where the record goes: the atomic write cannot replace it.
    fs::create_dir_all(record(&t)).unwrap();
    fs::write(record(&t).join("keep"), b"x").unwrap();
    let (_, runs) = tick(&t, NOW, &env(&[]), Ok(SUMMARY));
    assert!(runs.is_empty());
}

#[test]
fn symlinked_record_is_not_followed() {
    let t = setup();
    let victim = t.home.join("victim");
    fs::write(&victim, b"keep").unwrap();
    std::os::unix::fs::symlink(&victim, record(&t)).unwrap();
    tick(&t, NOW, &env(&[]), Ok(SUMMARY));
    assert_eq!(fs::read(&victim).unwrap(), b"keep");
}
