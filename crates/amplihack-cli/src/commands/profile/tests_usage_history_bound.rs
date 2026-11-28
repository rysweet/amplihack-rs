//! Issue #3459: the history's byte bound, and writes that cannot finish.
//!
//! Every test uses a private profile root. A test that lowers the file-size
//! limit runs its write in a child process (this test binary, run again), so
//! the shared test process keeps its limit.

use super::capacity_summary::RATE_WINDOW;
use super::usage_history::{
    Entry, FILE, FILE_LOW, FILE_MAX, HistoryError, Point, Writer, append_with, parse_line,
    read_all, read_recent, to_line,
};
use std::collections::BTreeMap;

const T: i64 = 1_793_376_900;
/// The measured rate: 128 lines an hour.
const LINE_SECS: i64 = 3600 / 128;
const CHILD_ENV: &str = "AMPLIHACK_TEST_HISTORY_CHILD";
const CHILD_ROOT_ENV: &str = "AMPLIHACK_TEST_HISTORY_ROOT";
const CHILD_LIMIT_ENV: &str = "AMPLIHACK_TEST_HISTORY_LIMIT";

fn point(i: usize, at: i64) -> Point {
    Point {
        s: Some((i % 100) as u8),
        w: ((i * 7) % 101) as u8,
        w_reset: Some(at + 3 * 86_400 + (i as i64) * 60),
        s_reset: None,
        observed_at: Some(at),
        no_session_limit: false,
        passive: false,
    }
}

/// A cycle of the measured size: 29 logins, each read during the cycle.
fn cycle(at: i64) -> Entry {
    let profiles: BTreeMap<String, Point> = (0..29)
        .map(|i| (format!("login-{i:02}"), point(i, at)))
        .collect();
    Entry {
        at,
        profiles,
        by: Some(Writer::Usage),
        unknown: Vec::new(),
    }
}

/// The same cycle in the older format: a time on every point.
fn long_form(e: &Entry) -> String {
    let mut v: serde_json::Value = serde_json::from_str(&to_line(e)).unwrap();
    v.as_object_mut().unwrap().remove("observed_at_batch");
    for p in v["profiles"].as_object_mut().unwrap().values_mut() {
        p["observed_at"] = serde_json::Value::String(super::usage_cmd::utc_time(e.at));
    }
    serde_json::to_string(&v).unwrap()
}

struct Root {
    _t: tempfile::TempDir,
    root: std::path::PathBuf,
}

impl Root {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let root = t.path().to_path_buf();
        let state = root.join(".auto-switch.d");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { _t: t, root }
    }

    fn history(&self) -> std::path::PathBuf {
        self.root.join(".auto-switch.d").join(FILE)
    }

    fn seed(&self, body: &[u8]) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(self.history(), body).unwrap();
        std::fs::set_permissions(self.history(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn the_bound_is_in_bytes_under_two_mebibytes() {
    const { assert!(FILE_MAX < 2 * 1024 * 1024) };
    const { assert!(FILE_LOW < FILE_MAX) };
}

#[test]
fn the_longest_reader_span_fits_the_bound_at_the_measured_line() {
    let lines_in_span = (RATE_WINDOW / LINE_SECS) as usize + 1;
    let e = cycle(T);
    let short = to_line(&e).len() + 1;
    assert!(
        lines_in_span * short <= FILE_LOW,
        "{lines_in_span} lines of {short} bytes must fit the low mark"
    );
    // The same cycle in the older format is the measured 2.8 KiB line, and
    // the span of those would not fit: that is why lines are made shorter.
    let long = long_form(&e).len() + 1;
    assert!(long > 2800, "a long line is about 2.8 KiB, got {long}");
    assert!(lines_in_span * long > FILE_MAX);
}

#[test]
fn a_shortened_line_reads_back_as_the_same_entry() {
    let e = cycle(T);
    let line = to_line(&e);
    assert!(line.len() < long_form(&e).len());
    assert_eq!(parse_line(&line, T).unwrap(), e);
    assert_eq!(parse_line(&long_form(&e), T).unwrap(), e);
}

#[test]
fn a_point_with_its_own_time_or_none_keeps_it() {
    let mut e = cycle(T);
    e.profiles.get_mut("login-00").unwrap().observed_at = Some(T - 100);
    assert_eq!(parse_line(&to_line(&e), T).unwrap(), e);
    e.profiles.get_mut("login-01").unwrap().observed_at = None;
    assert_eq!(parse_line(&to_line(&e), T).unwrap(), e);
}

#[test]
fn appending_many_lines_keeps_the_bound_and_the_readers_span() {
    let r = Root::new();
    let total = 3 * (RATE_WINDOW / LINE_SECS) as usize;
    let mut last = T;
    for i in 0..total {
        last = T + i as i64 * LINE_SECS;
        append_with(&r.root, &cycle(last), last, None).unwrap();
        let len = std::fs::metadata(r.history()).unwrap().len() as usize;
        assert!(len <= FILE_MAX, "line {i}: {len} bytes passes the bound");
    }
    let now = last + 1;
    let (recent, warning) = read_recent(&r.root, now, RATE_WINDOW);
    assert!(warning.is_none());
    let want = (0..total)
        .filter(|i| now - (T + *i as i64 * LINE_SECS) <= RATE_WINDOW)
        .count();
    assert_eq!(recent.len(), want);
    let (all, _) = read_all(&r.root, now);
    assert!(all.len() >= want);
    assert_eq!(all.last().unwrap().at, last);
}

#[test]
fn a_file_from_an_older_writer_is_rewritten_without_cutting_the_span() {
    let r = Root::new();
    let total = 3 * (RATE_WINDOW / LINE_SECS) as usize;
    let mut body = String::new();
    for i in 0..total {
        body.push_str(&long_form(&cycle(T + i as i64 * LINE_SECS)));
        body.push('\n');
    }
    assert!(body.len() > FILE_MAX);
    r.seed(body.as_bytes());
    let last = T + total as i64 * LINE_SECS;
    append_with(&r.root, &cycle(last), last, None).unwrap();
    let len = std::fs::metadata(r.history()).unwrap().len() as usize;
    assert!(len < FILE_LOW);
    let (recent, _) = read_recent(&r.root, last + 1, RATE_WINDOW);
    let want = (0..=total)
        .filter(|i| last + 1 - (T + *i as i64 * LINE_SECS) <= RATE_WINDOW)
        .count();
    assert_eq!(recent.len(), want);
}

// ---------------------------------------------------------------------------
// Writes that cannot finish
// ---------------------------------------------------------------------------

/// Runs the child below in this test binary under the given file-size limit
/// and returns its exit status and what it printed.
fn run_child(root: &std::path::Path, limit: u64, test: &str) -> (bool, String) {
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args([test, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .env(CHILD_ROOT_ENV, root)
        .env(CHILD_LIMIT_ENV, limit.to_string())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn in_child() -> Option<(std::path::PathBuf, u64)> {
    std::env::var(CHILD_ENV).ok()?;
    let root = std::env::var_os(CHILD_ROOT_ENV)?.into();
    let limit = std::env::var(CHILD_LIMIT_ENV).ok()?.parse().ok()?;
    Some((root, limit))
}

fn lower_limit(bytes: u64) {
    let r = libc::rlimit {
        rlim_cur: bytes as libc::rlim_t,
        rlim_max: bytes as libc::rlim_t,
    };
    // SAFETY: `r` is a valid `rlimit`; this runs only in the child process.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &r) }, 0);
}

const CHILD: &str = "tests_usage_history_bound::child_append_under_a_limit";
const TOO_LARGE_MARK: &str = "RESULT:too-large";
const OK_MARK: &str = "RESULT:ok";

/// The child of [`an_append_past_the_file_size_limit_writes_nothing`].
#[test]
fn child_append_under_a_limit() {
    let Some((root, limit)) = in_child() else {
        return;
    };
    lower_limit(limit);
    match append_with(&root, &cycle(T + 10), T + 10, None) {
        Err(HistoryError::TooLarge) => println!("{TOO_LARGE_MARK}"),
        Ok(()) => println!("{OK_MARK}"),
        Err(other) => println!("RESULT:{other:?}"),
    }
}

#[test]
fn an_append_past_the_file_size_limit_writes_nothing() {
    let r = Root::new();
    let seed = format!("{}\n", to_line(&cycle(T)));
    r.seed(seed.as_bytes());
    // Under the limit's reach: the file is already at the limit.
    let (ok, out) = run_child(&r.root, seed.len() as u64 + 10, CHILD);
    assert!(ok, "the child must not be ended by a signal: {out}");
    assert!(out.contains(TOO_LARGE_MARK), "{out}");
    assert_eq!(std::fs::read(r.history()).unwrap(), seed.as_bytes());
}

#[test]
fn a_rewrite_past_the_file_size_limit_writes_nothing() {
    let r = Root::new();
    let mut body = String::new();
    let mut i = 0;
    while body.len() <= FILE_MAX {
        body.push_str(&to_line(&cycle(T + i * LINE_SECS)));
        body.push('\n');
        i += 1;
    }
    r.seed(body.as_bytes());
    let (ok, out) = run_child(&r.root, 1000, CHILD);
    assert!(ok, "the child must not be ended by a signal: {out}");
    assert!(out.contains(TOO_LARGE_MARK), "{out}");
    assert_eq!(std::fs::read(r.history()).unwrap(), body.as_bytes());
}

#[test]
fn an_append_that_fits_the_limit_is_written() {
    let r = Root::new();
    let (ok, out) = run_child(&r.root, 1_000_000, CHILD);
    assert!(ok, "{out}");
    assert!(out.contains(OK_MARK), "{out}");
    assert_eq!(
        std::fs::read_to_string(r.history())
            .unwrap()
            .lines()
            .count(),
        1
    );
}

/// A write that fails for a reason other than the limit is an error too.
#[test]
fn a_failed_write_of_another_kind_is_an_error_and_leaves_the_history() {
    let r = Root::new();
    // The history path is a directory: opening it for writing fails.
    std::fs::create_dir(r.history()).unwrap();
    let got = append_with(&r.root, &cycle(T), T, None);
    assert!(matches!(
        got,
        Err(HistoryError::WriteFailed | HistoryError::UnsafeFile)
    ));
    assert!(r.history().is_dir());
}
