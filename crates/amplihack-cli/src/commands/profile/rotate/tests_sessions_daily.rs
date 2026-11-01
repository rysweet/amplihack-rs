//! `rotate::sessions_daily` (issue #1746): the rotator's watch loop archives
//! old sessions once a day.
//!
//! Contract:
//! - `tick_with(rs: &RotateState, home: &Path, now: i64,
//!   log: &mut dyn FnMut(&str),
//!   run: &mut dyn FnMut(&Opts) -> Result<Ran, String>)`: `run` stands in
//!   for `archive::run` plus `ran` and returns the log line, with `more`
//!   set when the run stopped early after archiving something, or the
//!   error line. Never panics, never returns an error.
//! - `ran(&Report, &Opts, requested) -> Ran`: the summary line; for a run
//!   that stopped early, the reason (`rotate stop or a signal` when
//!   `requested`, else the 60-second `RUN_LIMIT`) and whether the rest
//!   continues at the next pass (`more`, only if something was archived).
//! - `more` removes the record, so the next pass is due.
//! - Switch: the user config key `sessions.daily_archive` in
//!   `<home>/.amplihack/config.toml`, read through
//!   `user_config::sessions_daily_archive` only when a run is due. `false`
//!   means nothing runs and nothing is written; `true` or no file runs. A
//!   file or value that cannot be used (bad TOML, not a bool, writable by
//!   others) fails closed: the record is written, nothing runs, and the log
//!   gets `sessions archive: cannot read sessions.daily_archive (<why>); not
//!   run`, so the next try is a day later.
//! - Record: `<rs.dir>/sessions-archive.last`, decimal epoch seconds. Due
//!   when missing or garbage, when `now − last ≥ 86400`, or when
//!   `last > now`. Written (0600) before `run`; if that write fails, `run`
//!   is not called.
//! - Every outcome is logged as `sessions archive: <line>`.
//! - Days and DEST come from `config::resolve` with no flags, `apply` on:
//!   the keys `sessions.archive_days` and `sessions.archive_dest` in the
//!   same file, read on each due pass. A key the file cannot give a value
//!   is logged as `sessions archive: cannot read <key> (<why>); not run`,
//!   after the record is written.

use super::ledger::RotateState;
use super::sessions_daily::{RUN_LIMIT, Ran, ran, tick_with};
use crate::commands::sessions::archive::Report;
use crate::commands::sessions::config::Opts;
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

fn record(t: &T) -> PathBuf {
    t.rs.dir.join("sessions-archive.last")
}

/// Runs one tick; returns (log lines, opts passed to each run).
fn tick(t: &T, now: i64, result: Result<&str, &str>) -> (Vec<String>, Vec<Opts>) {
    let mut logs = Vec::new();
    let mut runs = Vec::new();
    let mut log = |l: &str| logs.push(l.to_string());
    let mut run = |o: &Opts| {
        runs.push(o.clone());
        result.map(done).map_err(str::to_string)
    };
    tick_with(&t.rs, &t.home, now, &mut log, &mut run);
    (logs, runs)
}

/// A run that reached every session.
fn done(line: &str) -> Ran {
    Ran {
        line: line.to_string(),
        more: false,
    }
}

/// Runs one tick whose run returns `ran`; returns the log lines and
/// whether `run` was called.
fn tick_ran(t: &T, now: i64, ran: Ran) -> (Vec<String>, bool) {
    let mut logs = Vec::new();
    let mut called = false;
    let mut log = |l: &str| logs.push(l.to_string());
    let mut run = |_: &Opts| {
        called = true;
        Ok(ran.clone())
    };
    tick_with(&t.rs, &t.home, now, &mut log, &mut run);
    (logs, called)
}

const SUMMARY: &str =
    "archived 3 sessions, 0.00 GiB (older than 3 days; 0 live sessions kept; 0 failed)";

#[test]
fn first_tick_runs_logs_and_writes_the_record() {
    let t = setup();
    let (logs, runs) = tick(&t, NOW, Ok(SUMMARY));
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
    tick(&t, NOW, Ok(SUMMARY));
    let (logs, runs) = tick(&t, NOW + DAY - 1, Ok(SUMMARY));
    assert!(runs.is_empty() && logs.is_empty());
    let (_, runs) = tick(&t, NOW + DAY, Ok(SUMMARY));
    assert_eq!(runs.len(), 1);
}

#[test]
fn clock_moved_back_is_due() {
    let t = setup();
    tick(&t, NOW, Ok(SUMMARY));
    let (_, runs) = tick(&t, NOW - 10, Ok(SUMMARY));
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
        let (_, runs) = tick(&t, NOW, Ok(SUMMARY));
        assert_eq!(runs.len(), 1, "record {raw:?}");
    }
    for now in [i64::MIN, i64::MAX, 0, -1] {
        let t = setup();
        fs::write(record(&t), "100").unwrap();
        tick(&t, now, Ok(SUMMARY));
    }
}

/// Writes `<home>/.amplihack/config.toml` the way `config set` leaves it:
/// the directory `0700`, the file `mode`.
fn config(t: &T, body: &str, mode: u32) -> PathBuf {
    let dir = t.home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let file = dir.join("config.toml");
    fs::write(&file, body).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
    file
}

#[test]
fn config_false_does_nothing() {
    let t = setup();
    config(&t, "[sessions]\ndaily_archive = false\n", 0o600);
    let (logs, runs) = tick(&t, NOW, Ok(SUMMARY));
    assert!(runs.is_empty() && logs.is_empty(), "{logs:?}");
    assert!(!record(&t).exists());
}

#[test]
fn config_true_or_no_key_runs() {
    for body in [
        "[sessions]\ndaily_archive = true\n",
        "[recipe]\nverbose = true\n",
        "",
    ] {
        let t = setup();
        config(&t, body, 0o600);
        let (logs, runs) = tick(&t, NOW, Ok(SUMMARY));
        assert_eq!(runs.len(), 1, "{body:?}: {logs:?}");
    }
}

#[test]
fn turning_it_off_reaches_a_running_rotator() {
    // The same process, the same state directory: only the file changes.
    let t = setup();
    let (_, runs) = tick(&t, NOW, Ok(SUMMARY));
    assert_eq!(runs.len(), 1);
    config(&t, "[sessions]\ndaily_archive = false\n", 0o600);
    let (logs, runs) = tick(&t, NOW + DAY, Ok(SUMMARY));
    assert!(runs.is_empty() && logs.is_empty(), "{logs:?}");
    config(&t, "[sessions]\ndaily_archive = true\n", 0o600);
    let (_, runs) = tick(&t, NOW + DAY + 60, Ok(SUMMARY));
    assert_eq!(runs.len(), 1, "back on at the next pass");
}

#[test]
fn an_unusable_config_fails_closed_and_waits_a_day() {
    let cases: [(&str, u32, &str); 3] = [
        (
            "[sessions]\ndaily_archive = \"no\"\n",
            0o600,
            "sessions.daily_archive: must be true or false",
        ),
        // Even `false` is not trusted from a file others can write, and
        // neither is `true`: the run does not happen either way.
        (
            "[sessions]\ndaily_archive = true\n",
            0o622,
            "sessions.daily_archive: ignored because the file is writable by group or others",
        ),
        ("[sessions\n", 0o600, "file ignored"),
    ];
    for (body, mode, why) in cases {
        let t = setup();
        let file = config(&t, body, mode);
        let (logs, runs) = tick(&t, NOW, Ok(SUMMARY));
        assert!(runs.is_empty(), "{body:?}");
        assert_eq!(logs.len(), 1, "{logs:?}");
        let line = &logs[0];
        assert!(
            line.starts_with(&format!(
                "sessions archive: cannot read sessions.daily_archive ({}: ",
                file.display()
            )),
            "{line}"
        );
        assert!(line.contains(why), "{line}");
        assert!(line.ends_with("); not run"), "{line}");
        assert!(
            record(&t).exists(),
            "{body:?}: retried a day later, not every pass"
        );
        let (logs, runs) = tick(&t, NOW + 60, Ok(SUMMARY));
        assert!(runs.is_empty() && logs.is_empty(), "{logs:?}");
    }
}

#[test]
fn days_and_dest_come_from_the_config_file_on_each_run() {
    let t = setup();
    config(
        &t,
        "[sessions]\narchive_days = 9\narchive_dest = \"/var/archive\"\n",
        0o600,
    );
    let (_, runs) = tick(&t, NOW, Ok(SUMMARY));
    assert_eq!(runs[0].days, 9);
    assert_eq!(runs[0].dest, Path::new("/var/archive"));
    // The same rotator, a day later: the file changed, nothing restarted.
    config(&t, "[sessions]\narchive_days = 4\n", 0o600);
    let (_, runs) = tick(&t, NOW + DAY, Ok(SUMMARY));
    assert_eq!(runs[0].days, 4);
    assert_eq!(runs[0].dest, t.home.join(".amplihack/session-archive"));
}

#[test]
fn an_unusable_days_or_dest_key_is_logged_and_still_postpones() {
    for (body, key, why) in [
        (
            "[sessions]\narchive_days = 0\n",
            "sessions.archive_days",
            "must be an integer from 1 to 4294967295",
        ),
        (
            "[sessions]\narchive_dest = \"relative\"\n",
            "sessions.archive_dest",
            "must be an absolute path",
        ),
    ] {
        let t = setup();
        let file = config(&t, body, 0o600);
        let (logs, runs) = tick(&t, NOW, Ok(SUMMARY));
        assert!(runs.is_empty(), "{body:?}: never the default instead");
        assert_eq!(logs.len(), 1, "{logs:?}");
        let line = &logs[0];
        assert!(
            line.starts_with(&format!(
                "sessions archive: cannot read {key} ({}: {key}: {why}",
                file.display()
            )),
            "{line}"
        );
        assert!(line.ends_with("); not run"), "{line}");
        assert!(record(&t).exists(), "a config error waits a day");
    }
}

#[test]
fn failure_is_logged_and_the_next_try_is_a_day_later() {
    let t = setup();
    let lock = "another sessions archive run is in progress (lock: /d/.archive.lock)";
    let (logs, _) = tick(&t, NOW, Err(lock));
    assert_eq!(logs, [format!("sessions archive: {lock}")]);
    let (_, runs) = tick(&t, NOW + 60, Ok(SUMMARY));
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
        Ok(done(SUMMARY))
    };
    tick_with(&t.rs, &t.home, NOW, &mut log, &mut run);
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
    let (_, runs) = tick(&t, NOW, Ok(SUMMARY));
    assert!(runs.is_empty());
}

#[test]
fn symlinked_record_is_not_followed() {
    let t = setup();
    let victim = t.home.join("victim");
    fs::write(&victim, b"keep").unwrap();
    std::os::unix::fs::symlink(&victim, record(&t)).unwrap();
    tick(&t, NOW, Ok(SUMMARY));
    assert_eq!(fs::read(&victim).unwrap(), b"keep");
}

#[test]
fn a_home_outside_the_temp_dir_is_never_read_in_a_test() {
    // The test guard: no test reads the developer's real config, and the
    // run then fails closed rather than defaulting to on.
    let e = crate::user_config::sessions_daily_archive(Path::new("/")).unwrap_err();
    assert!(e.contains("AMPLIHACK_TEST_HOME_GUARD"), "{e}");
}

// ------------------------------------------------- runs that stop early

fn opts() -> Opts {
    Opts {
        home: PathBuf::from("/h"),
        days: 3,
        dest: PathBuf::from("/d"),
        apply: true,
    }
}

fn report(archived: u64, stopped: bool) -> Report {
    Report {
        archived,
        bytes: 0,
        live_kept: 0,
        failed: 0,
        stopped,
    }
}

#[test]
fn the_limit_is_a_minute() {
    assert_eq!(RUN_LIMIT.as_secs(), 60);
}

#[test]
fn a_run_that_reached_every_session_is_just_its_summary() {
    let r = ran(&report(3, false), &opts(), false);
    assert_eq!(r, done(SUMMARY));
}

#[test]
fn a_run_stopped_at_the_limit_after_progress_continues() {
    let r = ran(&report(3, true), &opts(), false);
    assert_eq!(
        r.line,
        format!("{SUMMARY}; stopped at the 60-second limit; the rest continues at the next pass")
    );
    assert!(r.more);
}

#[test]
fn a_run_stopped_by_rotate_stop_after_progress_continues() {
    let r = ran(&report(3, true), &opts(), true);
    assert_eq!(
        r.line,
        format!(
            "{SUMMARY}; stopped by rotate stop or a signal; the rest continues at the next pass"
        )
    );
    assert!(r.more);
}

#[test]
fn a_run_stopped_before_archiving_anything_waits_a_day() {
    // Sessions that keep failing must not be retried, and logged, on every
    // pass: without progress the run waits a day like any other.
    let r = ran(&report(0, true), &opts(), false);
    assert!(
        r.line.ends_with(
            "; stopped at the 60-second limit; nothing archived, so the next try is a day later"
        ),
        "{}",
        r.line
    );
    assert!(!r.more);
}

#[test]
fn more_makes_the_next_pass_due_and_a_full_run_ends_it() {
    let t = setup();
    let cut = Ran {
        line: "partial".into(),
        more: true,
    };
    let (logs, called) = tick_ran(&t, NOW, cut.clone());
    assert!(called);
    assert_eq!(logs, ["sessions archive: partial"]);
    assert!(!record(&t).exists(), "removed, so the next pass is due");
    let (_, called) = tick_ran(&t, NOW + 60, cut);
    assert!(called, "the next pass carries on");
    let (_, called) = tick_ran(&t, NOW + 120, done(SUMMARY));
    assert!(called);
    assert_eq!(
        fs::read_to_string(record(&t)).unwrap().trim(),
        (NOW + 120).to_string()
    );
    let (logs, called) = tick_ran(&t, NOW + 180, done(SUMMARY));
    assert!(!called && logs.is_empty(), "finished: a day later");
}

#[test]
fn without_more_an_early_stop_waits_a_day() {
    let t = setup();
    let none = Ran {
        line: "nothing".into(),
        more: false,
    };
    tick_ran(&t, NOW, none.clone());
    let (_, called) = tick_ran(&t, NOW + 60, none);
    assert!(!called);
}

#[test]
fn a_record_that_cannot_be_removed_is_logged() {
    let t = setup();
    // The state directory is read-only once the record is written, so the
    // removal fails; the rest of the run then waits a day.
    let mut logs = Vec::new();
    let mut log = |l: &str| logs.push(l.to_string());
    let dir = t.rs.dir.clone();
    let mut run = |_: &Opts| {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        Ok(Ran {
            line: "partial".into(),
            more: true,
        })
    };
    tick_with(&t.rs, &t.home, NOW, &mut log, &mut run);
    fs::set_permissions(&t.rs.dir, fs::Permissions::from_mode(0o700)).unwrap();
    if nix_root() {
        return; // root ignores the mode
    }
    assert_eq!(
        logs,
        [
            "sessions archive: partial",
            "sessions archive: cannot remove the run record (permission denied); the rest waits a day",
        ]
    );
    assert!(record(&t).exists());
}

fn nix_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}
