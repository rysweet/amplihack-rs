//! The wait-state record (issue #2967): the small file a supervisor keeps for
//! the length of a usage-limit wait, which the stderr pump of
//! `amplihack recipe run` reads to rewrite the runner's heartbeat.
//!
//! Contract of `wait_state.rs` (Unix only; on other hosts nothing is written
//! or read):
//!
//! - `DIR_ENV` is `AMPLIHACK_USAGE_LIMIT_WAIT_DIR`, `STEPS_ENV` is
//!   `AMPLIHACK_USAGE_LIMIT_WAIT_STEPS`.
//! - `valid_dir(&Path) -> bool`: an absolute path to a directory (not a
//!   symlink) owned by the effective user with mode exactly `0700`.
//! - `chain_with(inherited: Option<&str>, pid: u32) -> String`: the value of
//!   `STEPS_ENV` for a child, `inherited` with `pid` appended. `inherited` is
//!   parsed strictly, as comma-separated decimal process ids; a malformed
//!   value is replaced, not repaired.
//! - `begin(dir, pid, until, expires, steps) -> Option<Guard>`: writes
//!   `<dir>/<pid>` as `{"v":1,"pid":N,"until":T,"expires":E,"steps":[..]}`,
//!   integers only, mode `0600`, through a temporary file made with
//!   `create_new` and renamed. `None`, and nothing written, when `dir` is not
//!   valid. Dropping the `Guard` removes the record, on return and on unwind.
//! - `waiting(dir, pid, now) -> Option<i64>`: the earliest `until` among the
//!   live records that are `pid`'s own or list `pid` in `steps` (an enclosing
//!   step's heartbeat names the earliest wait beneath it). A record counts
//!   only when `v` is 1, its `pid` matches its file name, `now` is before its
//!   `expires`, its `until` has not passed and its process is alive. An
//!   unknown key is ignored.
//!
//! The reader opens each file without following links, checks type and owner
//! on the open descriptor, reads at most 1024 bytes, accepts only file names
//! that are all digits and parse to a non-zero number no greater than
//! `i32::MAX`, and looks at no more than 64 entries, with no recursion.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(all(test, unix))] mod tests_wait_state;`.

use super::wait_state::{DIR_ENV, STEPS_ENV, begin, chain_with, valid_dir, waiting};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

const NOW: i64 = 1_792_335_600;

fn me() -> u32 {
    std::process::id()
}

/// A private directory, as `recipe run` makes it.
fn private() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap().join("usage-limit-wait");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    (tmp, dir)
}

/// A process id that is no longer alive: a reaped child.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn write_record(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn record(pid: u32, until: i64, expires: i64, steps: &str) -> String {
    format!(r#"{{"v":1,"pid":{pid},"until":{until},"expires":{expires},"steps":[{steps}]}}"#)
}

#[test]
fn the_variable_names_are_the_documented_ones() {
    assert_eq!(DIR_ENV, "AMPLIHACK_USAGE_LIMIT_WAIT_DIR");
    assert_eq!(STEPS_ENV, "AMPLIHACK_USAGE_LIMIT_WAIT_STEPS");
}

// --- valid_dir ------------------------------------------------------------------

#[test]
fn a_private_directory_owned_by_us_is_valid() {
    let (_tmp, dir) = private();
    assert!(valid_dir(&dir));
}

#[test]
fn a_directory_that_is_not_exactly_0700_is_not_valid() {
    let (_tmp, dir) = private();
    for mode in [0o755, 0o750, 0o770, 0o777, 0o701, 0o500, 0o600, 0o000] {
        fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
        assert!(!valid_dir(&dir), "mode {mode:o}");
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_relative_missing_or_non_directory_path_is_not_valid() {
    let (_tmp, dir) = private();
    assert!(!valid_dir(Path::new("usage-limit-wait")));
    assert!(!valid_dir(Path::new("")));
    assert!(!valid_dir(&dir.join("missing")));
    let file = dir.join("file");
    fs::write(&file, "x").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!valid_dir(&file));
}

#[test]
fn a_symlink_to_a_private_directory_is_not_valid() {
    let (tmp, dir) = private();
    let link = tmp.path().join("link");
    symlink(&dir, &link).unwrap();
    assert!(!valid_dir(&link));
}

// --- chain_with -----------------------------------------------------------------

#[test]
fn the_chain_starts_with_the_supervisors_own_pid() {
    assert_eq!(chain_with(None, 123), "123");
    assert_eq!(chain_with(Some(""), 123), "123");
}

#[test]
fn the_chain_appends_to_a_well_formed_inherited_value() {
    assert_eq!(chain_with(Some("10"), 123), "10,123");
    assert_eq!(chain_with(Some("10,20"), 123), "10,20,123");
    assert_eq!(chain_with(Some("2147483647"), 5), "2147483647,5");
}

#[test]
fn a_malformed_inherited_value_is_replaced() {
    for bad in [
        "abc",
        "10,abc",
        "10,,20",
        ",10",
        "10,",
        " 10",
        "10 ",
        "10, 20",
        "0",
        "10,0",
        "-1",
        "2147483648",
        "99999999999999999999",
        "10;20",
        "10\n20",
        "1e3",
        "+5",
    ] {
        assert_eq!(chain_with(Some(bad), 123), "123", "{bad:?}");
    }
}

// --- begin and the guard --------------------------------------------------------

#[test]
fn begin_writes_the_record_with_integers_only_and_mode_0600() {
    let (_tmp, dir) = private();
    let _guard = begin(&dir, me(), NOW + 60, NOW + 600, &[7, 8]).expect("a valid directory");
    let path = dir.join(me().to_string());
    let body = fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "v": 1,
            "pid": me(),
            "until": NOW + 60,
            "expires": NOW + 600,
            "steps": [7, 8],
        })
    );
    assert!(body.len() <= 1024);
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn begin_leaves_no_temporary_file_behind() {
    let (_tmp, dir) = private();
    let _guard = begin(&dir, me(), NOW + 60, NOW + 600, &[]).unwrap();
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, [me().to_string()]);
}

#[test]
fn begin_in_an_invalid_directory_writes_nothing() {
    let (_tmp, dir) = private();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(begin(&dir, me(), NOW + 60, NOW + 600, &[]).is_none());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(begin(Path::new("relative"), me(), NOW + 60, NOW + 600, &[]).is_none());
}

#[test]
fn dropping_the_guard_removes_the_record() {
    let (_tmp, dir) = private();
    let guard = begin(&dir, me(), NOW + 60, NOW + 600, &[]).unwrap();
    assert!(dir.join(me().to_string()).exists());
    drop(guard);
    assert!(!dir.join(me().to_string()).exists());
}

#[test]
fn the_guard_removes_the_record_on_unwind() {
    let (_tmp, dir) = private();
    let d = dir.clone();
    let result = std::panic::catch_unwind(move || {
        let _guard = begin(&d, me(), NOW + 60, NOW + 600, &[]).unwrap();
        panic!("the wait failed");
    });
    assert!(result.is_err());
    assert!(!dir.join(me().to_string()).exists());
}

// --- waiting: the reader -------------------------------------------------------

#[test]
fn a_live_record_gives_its_until() {
    let (_tmp, dir) = private();
    let _guard = begin(&dir, me(), NOW + 60, NOW + 600, &[]).unwrap();
    assert_eq!(waiting(&dir, me(), NOW), Some(NOW + 60));
}

#[test]
fn no_record_gives_none() {
    let (_tmp, dir) = private();
    assert_eq!(waiting(&dir, me(), NOW), None);
}

#[test]
fn a_record_past_its_expiry_is_ignored_even_when_its_process_is_alive() {
    let (_tmp, dir) = private();
    write_record(
        &dir,
        &me().to_string(),
        &record(me(), NOW + 9999, NOW + 600, ""),
    );
    assert_eq!(waiting(&dir, me(), NOW + 599), Some(NOW + 9999));
    assert_eq!(waiting(&dir, me(), NOW + 600), None, "expires is exclusive");
    assert_eq!(waiting(&dir, me(), NOW + 601), None);
}

#[test]
fn a_record_whose_until_has_passed_is_ignored() {
    let (_tmp, dir) = private();
    write_record(
        &dir,
        &me().to_string(),
        &record(me(), NOW - 1, NOW + 600, ""),
    );
    assert_eq!(waiting(&dir, me(), NOW), None);
}

#[test]
fn a_record_of_a_dead_process_is_ignored() {
    let (_tmp, dir) = private();
    let pid = dead_pid();
    write_record(
        &dir,
        &pid.to_string(),
        &record(pid, NOW + 60, NOW + 600, ""),
    );
    assert_eq!(waiting(&dir, pid, NOW), None);
}

#[test]
fn a_record_whose_pid_differs_from_its_file_name_is_ignored() {
    let (_tmp, dir) = private();
    write_record(
        &dir,
        &me().to_string(),
        &record(me() + 1, NOW + 60, NOW + 600, ""),
    );
    assert_eq!(waiting(&dir, me(), NOW), None);
}

#[test]
fn an_unknown_key_is_ignored_and_a_version_other_than_1_is_not_read() {
    let (_tmp, dir) = private();
    let name = me().to_string();
    write_record(
        &dir,
        &name,
        &format!(
            r#"{{"v":1,"pid":{},"until":{},"expires":{},"steps":[],"later":[1,2]}}"#,
            me(),
            NOW + 60,
            NOW + 600
        ),
    );
    assert_eq!(waiting(&dir, me(), NOW), Some(NOW + 60));
    write_record(
        &dir,
        &name,
        &format!(
            r#"{{"v":2,"pid":{},"until":{},"expires":{},"steps":[]}}"#,
            me(),
            NOW + 60,
            NOW + 600
        ),
    );
    assert_eq!(waiting(&dir, me(), NOW), None);
}

#[test]
fn malformed_records_are_ignored() {
    let (_tmp, dir) = private();
    let name = me().to_string();
    for body in [
        "",
        "not json",
        "{}",
        "[]",
        "null",
        r#"{"v":1}"#,
        // Not integers.
        r#"{"v":1,"pid":"x","until":1,"expires":2,"steps":[]}"#,
        &format!(
            r#"{{"v":1,"pid":{},"until":"soon","expires":9999999999,"steps":[]}}"#,
            me()
        ),
        &format!(
            r#"{{"v":1,"pid":{},"until":1.5,"expires":9999999999,"steps":[]}}"#,
            me()
        ),
        // Steps that are not process ids.
        &format!(
            r#"{{"v":1,"pid":{},"until":{},"expires":{},"steps":["a"]}}"#,
            me(),
            NOW + 60,
            NOW + 600
        ),
        &format!(
            r#"{{"v":1,"pid":{},"until":{},"expires":{},"steps":[-1]}}"#,
            me(),
            NOW + 60,
            NOW + 600
        ),
    ] {
        write_record(&dir, &name, body);
        assert_eq!(waiting(&dir, me(), NOW), None, "{body:?}");
    }
}

#[test]
fn a_record_over_1024_bytes_is_ignored() {
    let (_tmp, dir) = private();
    let steps = vec!["1"; 600].join(",");
    let body = record(me(), NOW + 60, NOW + 600, &steps);
    assert!(body.len() > 1024);
    write_record(&dir, &me().to_string(), &body);
    assert_eq!(waiting(&dir, me(), NOW), None);
}

#[test]
fn a_symlink_in_place_of_a_record_is_not_followed() {
    let (tmp, dir) = private();
    let real = tmp.path().join("elsewhere");
    fs::write(&real, record(me(), NOW + 60, NOW + 600, "")).unwrap();
    symlink(&real, dir.join(me().to_string())).unwrap();
    assert_eq!(waiting(&dir, me(), NOW), None);
}

#[test]
fn a_fifo_in_place_of_a_record_neither_blocks_the_reader_nor_counts() {
    let (_tmp, dir) = private();
    let path = dir.join(me().to_string());
    let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let (tx, rx) = mpsc::channel();
    let d = dir.clone();
    std::thread::spawn(move || {
        let _ = tx.send(waiting(&d, me(), NOW));
    });
    let got = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the reader returned: a FIFO must not block it");
    assert_eq!(got, None);
}

#[test]
fn a_directory_in_place_of_a_record_is_ignored_and_not_entered() {
    let (_tmp, dir) = private();
    let sub = dir.join(me().to_string());
    fs::create_dir(&sub).unwrap();
    write_record(
        &sub,
        &me().to_string(),
        &record(me(), NOW + 60, NOW + 600, ""),
    );
    assert_eq!(waiting(&dir, me(), NOW), None);
}

#[test]
fn file_names_that_are_not_plain_process_ids_are_never_read() {
    let (_tmp, dir) = private();
    // Each of these holds a perfectly good record for `me()`, and the
    // directory is scanned for `me()` as an enclosing step.
    let good = record(me(), NOW + 60, NOW + 600, &me().to_string());
    for name in [
        "abc",
        "0",
        "00",
        "2147483648",
        "99999999999999999999",
        "-5",
        "+5",
        "41822.tmp",
        ".41822",
        "4182 2",
    ] {
        write_record(&dir, name, &good);
    }
    assert_eq!(waiting(&dir, 41_822, NOW), None);
    assert_eq!(waiting(&dir, me(), NOW), None);
}

#[test]
fn a_crowded_directory_is_scanned_without_recursion_and_returns() {
    let (_tmp, dir) = private();
    // More than 64 entries: the scan looks at no more than 64 and must
    // return whatever it finds.
    for i in 1..=100u32 {
        write_record(&dir, &format!("{}", 900_000 + i), "x");
    }
    write_record(
        &dir,
        &me().to_string(),
        &record(me(), NOW + 60, NOW + 600, ""),
    );
    // The record for `me()` itself is read by name, with no scan.
    assert_eq!(waiting(&dir, me(), NOW), Some(NOW + 60));
    // An enclosing pid has no record of its own, so only the scan could
    // find a nested record; here it finds none and says so.
    assert_eq!(waiting(&dir, 4242, NOW), None);
}

// --- waiting: the chain of enclosing steps -------------------------------------

#[test]
fn an_enclosing_step_sees_the_earliest_wait_beneath_it() {
    let (_tmp, dir) = private();
    // `me()` is the waiting supervisor; its chain names the enclosing
    // supervisor `outer`, which has no record of its own.
    let outer = 4242u32;
    write_record(
        &dir,
        &me().to_string(),
        &record(me(), NOW + 300, NOW + 900, &format!("{outer},{}", me())),
    );
    assert_eq!(waiting(&dir, outer, NOW), Some(NOW + 300));
}

#[test]
fn an_enclosing_step_takes_the_earliest_of_several_waits_beneath_it() {
    let (_tmp, dir) = private();
    let outer = 4242u32;
    let pid = me();
    write_record(
        &dir,
        &pid.to_string(),
        &record(pid, NOW + 300, NOW + 900, &format!("{outer}")),
    );
    // A second, live supervisor: the parent of this test process.
    let other = std::os::unix::process::parent_id();
    write_record(
        &dir,
        &other.to_string(),
        &record(other, NOW + 100, NOW + 900, &format!("{outer}")),
    );
    assert_eq!(waiting(&dir, outer, NOW), Some(NOW + 100));
}

#[test]
fn a_step_not_in_any_chain_sees_nothing() {
    let (_tmp, dir) = private();
    write_record(
        &dir,
        &me().to_string(),
        &record(me(), NOW + 300, NOW + 900, "4242"),
    );
    assert_eq!(waiting(&dir, 5151, NOW), None);
}

#[test]
fn an_expired_or_dead_record_beneath_an_enclosing_step_is_not_counted() {
    let (_tmp, dir) = private();
    let outer = 4242u32;
    let dead = dead_pid();
    write_record(
        &dir,
        &dead.to_string(),
        &record(dead, NOW + 100, NOW + 900, &format!("{outer}")),
    );
    write_record(
        &dir,
        &me().to_string(),
        &record(me(), NOW + 300, NOW - 1, &format!("{outer}")),
    );
    assert_eq!(waiting(&dir, outer, NOW), None);
}
