//! Issue #1797: `<root>/.auto-switch.d/usage-history.jsonl`.
//!
//! Contract (docs/reference/profile-command.md, "Usage history"):
//! - `entry_of` is the one validity rule: `ok` or `session_exhausted` with
//!   an all-models week, or `weekly_exhausted` (100% even without a week).
//!   Every other state is unknown and never written as 0 or 100.
//! - `from_rows` leaves out API-key rows and the `~/.claude` row, and counts
//!   a swapped live profile as unknown. (`from_probes` is tested in
//!   `rotate/tests_capacity_history.rs`, where the rotator's types are.)
//! - A line holds `at`, profile names, whole-number `s` and `w`, and
//!   `w_reset`; never `s_reset`, an account or anything Claude printed.
//! - Readers take the last 4 MiB, skip lines over 64 KiB, lines that are not
//!   JSON, entries with an `at` more than 100 years from now, and profiles
//!   whose name fails the name rules or whose percentage is outside 0-100
//!   (the rest of that line is kept). Skipped lines give one warning with
//!   their count. A missing `s` is `null`; offsets are read as UTC; unknown
//!   keys are ignored.
//! - Readers and writers accept the same files: regular, the user's own,
//!   one hard link, mode 0600. Any other history is refused by both.
//! - Writers lock `usage-history.lock` (50 ms retries for 1 s), append one
//!   line in one write, trim entries older than 8 days, keep the file under
//!   3 MiB after a rewrite, refuse symlinks, hard links, group or other
//!   bits and FIFOs, and never create `.auto-switch.d`.

use super::name::ProfileName;
use super::usage_cmd::Row;
use super::usage_history::{
    Entry, HistoryError, Point, Snapshot, Writer, entry_of, from_rows, parse_line, to_line,
};
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use std::collections::BTreeMap;

/// 2026-10-30T16:15:00Z.
const T: i64 = 1_793_376_900;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn win(label: &str, pct: f64, at: Option<i64>) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        used_percentage: pct,
        resets_text: Some("someone@example.com resets soon".to_string()),
        resets_at: at,
    }
}

fn report(state: UsageState, s: Option<f64>, w: Option<f64>) -> UsageReport {
    UsageReport {
        state,
        session: s.map(|p| win("session", p, Some(T + HOUR))),
        week_all: w.map(|p| win("all models", p, Some(T + DAY))),
        week_models: Vec::new(),
        session_limit: true,
        observed_at: None,
    }
}

fn row(name: &str, rep: UsageReport) -> Row {
    Row {
        profile: name.to_string(),
        account: format!("{name}-account@example.com"),
        default: false,
        leased_to: None,
        live_in: false,
        overflow: false,
        overflow_blocked_until: None,
        api_key: false,
        api_key_blocked: None,
        report: rep,
        error: None,
        raw: None,
        source: None,
        direct_failure: None,
        spend: super::usage_source::Spend::NotApplicable,
        extra_shape: None,
    }
}

fn p(s: Option<u8>, w: u8, w_reset: Option<i64>) -> Point {
    Point {
        s,
        w,
        w_reset,
        s_reset: None,
        no_session_limit: false,
        passive: false,
    }
}

fn entry(at: i64, points: &[(&str, Point)]) -> Entry {
    Entry {
        at,
        profiles: points
            .iter()
            .map(|(n, p)| (n.to_string(), p.clone()))
            .collect::<BTreeMap<_, _>>(),
        by: None,
        unknown: Vec::new(),
    }
}

fn name(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

// ---------------------------------------------------------------------------
// entry_of: the one validity rule
// ---------------------------------------------------------------------------

#[test]
fn ok_with_a_week_is_valid_at_its_rounded_percentages_and_reset_times() {
    let pt = entry_of(&report(UsageState::Ok, Some(12.5), Some(40.4))).unwrap();
    assert_eq!(pt.s, Some(13), "half away from zero");
    assert_eq!(pt.w, 40);
    assert_eq!(pt.w_reset, Some(T + DAY));
    assert_eq!(pt.s_reset, Some(T + HOUR));
}

#[test]
fn ok_without_a_session_is_valid_with_no_session_figure() {
    let pt = entry_of(&report(UsageState::Ok, None, Some(40.0))).unwrap();
    assert_eq!(pt.s, None);
    assert_eq!(pt.w, 40);
}

#[test]
fn ok_or_session_exhausted_without_a_week_is_unknown() {
    assert_eq!(entry_of(&report(UsageState::Ok, Some(10.0), None)), None);
    assert_eq!(
        entry_of(&report(UsageState::SessionExhausted, Some(100.0), None)),
        None
    );
}

#[test]
fn session_exhausted_is_written_as_a_full_session() {
    let pt = entry_of(&report(
        UsageState::SessionExhausted,
        Some(98.0),
        Some(30.0),
    ))
    .unwrap();
    assert_eq!(pt.s, Some(100));
    assert_eq!(pt.w, 30);
    let pt = entry_of(&report(UsageState::SessionExhausted, None, Some(30.0))).unwrap();
    assert_eq!(pt.s, Some(100));
}

#[test]
fn weekly_exhausted_is_a_known_full_week_even_without_a_week_window() {
    let pt = entry_of(&report(UsageState::WeeklyExhausted, Some(20.0), None)).unwrap();
    assert_eq!(pt.w, 100);
    assert_eq!(pt.w_reset, None);
    let pt = entry_of(&report(UsageState::WeeklyExhausted, None, Some(97.0))).unwrap();
    assert_eq!(pt.w, 100);
    assert_eq!(pt.w_reset, Some(T + DAY));
}

#[test]
fn every_other_state_is_unknown_even_with_windows() {
    for state in [
        UsageState::SpendLimit,
        UsageState::NotLoggedIn,
        UsageState::Timeout,
        UsageState::Unparsed,
        UsageState::Error,
    ] {
        assert_eq!(
            entry_of(&report(state, Some(10.0), Some(10.0))),
            None,
            "{state:?} is never 0 or 100"
        );
    }
}

#[test]
fn percentages_are_clamped_to_0_to_100() {
    let pt = entry_of(&report(UsageState::Ok, Some(-5.0), Some(123.4))).unwrap();
    assert_eq!((pt.s, pt.w), (Some(0), 100));
    let pt = entry_of(&report(UsageState::Ok, Some(f64::NAN), Some(50.0)));
    assert!(
        pt.as_ref().is_none_or(|p| p.s.is_none()),
        "a NaN session is not a reading: {pt:?}"
    );
    assert_eq!(
        entry_of(&report(UsageState::Ok, Some(1.0), Some(f64::NAN))),
        None
    );
}

// ---------------------------------------------------------------------------
// from_rows (`profile usage`)
// ---------------------------------------------------------------------------

#[test]
fn from_rows_keeps_valid_subscription_rows_and_lists_the_unknown_ones() {
    let mut key = row("key", report(UsageState::Ok, Some(0.0), Some(0.0)));
    key.api_key = true;
    let mut over = row("over", report(UsageState::Ok, Some(5.0), Some(50.0)));
    over.overflow = true;
    let rows = vec![
        row("work", report(UsageState::Ok, Some(20.0), Some(30.0))),
        row("slow", UsageReport::empty(UsageState::Timeout)),
        row("blind", report(UsageState::Ok, Some(20.0), None)),
        key,
        over,
    ];
    let s: Snapshot = from_rows(T, T, &rows, None).unwrap();
    assert_eq!(s.at, T);
    assert_eq!(
        s.points.keys().map(String::as_str).collect::<Vec<_>>(),
        ["over", "work"],
        "overflow subscription rows count; API-key rows do not"
    );
    assert_eq!(s.unknown, ["blind", "slow"], "sorted by name");
}

#[test]
fn from_rows_counts_a_swapped_live_profile_as_unknown() {
    let mut live = row("work", report(UsageState::Ok, Some(20.0), Some(30.0)));
    live.live_in = true;
    let rows = vec![
        live,
        row("spare", report(UsageState::Ok, Some(1.0), Some(2.0))),
    ];
    let s = from_rows(T, T, &rows, Some(&name("work"))).unwrap();
    assert!(!s.points.contains_key("work"));
    assert_eq!(s.unknown, ["work"]);
    assert!(s.points.contains_key("spare"));
}

#[test]
fn a_snapshot_entry_holds_only_its_points() {
    let rows = vec![
        row("work", report(UsageState::Ok, Some(20.0), Some(30.0))),
        row("slow", UsageReport::empty(UsageState::Timeout)),
    ];
    let e = from_rows(T, T, &rows, None)
        .unwrap()
        .entry(super::usage_history::Writer::Usage);
    assert_eq!(e.at, T);
    assert_eq!(e.profiles.keys().collect::<Vec<_>>(), ["work"]);
}

#[test]
fn from_rows_without_a_subscription_login_is_none() {
    let mut key = row("key", report(UsageState::Ok, Some(0.0), Some(0.0)));
    key.api_key = true;
    assert!(from_rows(T, T, &[key], None).is_none());
    assert!(from_rows(T, T, &[], None).is_none());
    let slow = row("slow", UsageReport::empty(UsageState::Timeout));
    assert_eq!(
        from_rows(T, T, &[slow], None).map(|s| s.unknown),
        Some(vec!["slow".to_string()]),
        "a login with no reading still counts"
    );
}

// ---------------------------------------------------------------------------
// Line format
// ---------------------------------------------------------------------------

#[test]
fn a_line_is_the_documented_json_with_no_session_reset() {
    let mut e = entry(
        T,
        &[
            ("night", p(Some(100), 85, Some(T + 9 * HOUR + 45 * 60))),
            ("review", p(Some(5), 10, None)),
            ("blind", p(None, 7, None)),
        ],
    );
    e.profiles.get_mut("night").unwrap().s_reset = Some(T + HOUR);
    assert_eq!(
        to_line(&e),
        r#"{"at":"2026-10-30T16:15:00Z","profiles":{"blind":{"s":null,"w":7,"w_reset":null},"night":{"s":100,"w":85,"w_reset":"2026-10-31T02:00:00Z"},"review":{"s":5,"w":10,"w_reset":null}}}"#
    );
}

#[test]
fn a_line_reads_back_as_the_same_entry_without_session_resets() {
    let e = entry(
        T,
        &[
            ("a", p(Some(3), 4, Some(T + DAY))),
            ("b", p(None, 100, None)),
        ],
    );
    assert_eq!(parse_line(&to_line(&e), T), Some(e));
}

#[test]
fn times_with_an_offset_are_read_as_utc_and_a_missing_s_is_null() {
    let e = parse_line(
        r#"{"at":"2026-10-30T18:15:00+02:00","profiles":{"a":{"w":4,"w_reset":"2026-10-31T03:00:00+01:00"}}}"#,
        T,
    )
    .unwrap();
    assert_eq!(e.at, T);
    assert_eq!(e.profiles["a"], p(None, 4, Some(T + 9 * HOUR + 45 * 60)));
}

#[test]
fn unknown_keys_are_ignored() {
    let e = parse_line(
        r#"{"at":"2026-10-30T16:15:00Z","v":2,"profiles":{"a":{"s":1,"w":2,"w_reset":null,"new":"x"}}}"#,
        T,
    )
    .unwrap();
    assert_eq!(e.profiles["a"], p(Some(1), 2, None));
}

#[test]
fn a_percentage_outside_0_to_100_drops_that_profile_only() {
    let e = parse_line(
        r#"{"at":"2026-10-30T16:15:00Z","profiles":{"a":{"s":1,"w":101},"b":{"s":101,"w":2},"c":{"s":-1,"w":2},"d":{"s":1,"w":2.5},"ok":{"s":1,"w":2}}}"#,
        T,
    )
    .unwrap();
    assert_eq!(e.profiles.keys().collect::<Vec<_>>(), ["ok"]);
}

#[test]
fn a_name_that_fails_the_name_rules_drops_that_profile_only() {
    let e = parse_line(
        r#"{"at":"2026-10-30T16:15:00Z","profiles":{"../x":{"w":1},"a b":{"w":1},"\u001b[31mred":{"w":1},"":{"w":1},"ok":{"w":1}}}"#,
        T,
    )
    .unwrap();
    assert_eq!(e.profiles.keys().collect::<Vec<_>>(), ["ok"]);
}

#[test]
fn a_time_more_than_100_years_away_or_bad_json_rejects_the_line() {
    for line in [
        r#"{"at":"9999-01-01T00:00:00Z","profiles":{}}"#,
        r#"{"at":"1900-01-01T00:00:00Z","profiles":{}}"#,
        r#"{"at":"yesterday","profiles":{}}"#,
        r#"{"at":1793376900,"profiles":{}}"#,
        r#"{"profiles":{}}"#,
        r#"{"at":"2026-10-30T16:15:00Z","profiles":[]}"#,
        "not json",
        "",
    ] {
        assert_eq!(parse_line(line, T), None, "{line}");
    }
}

#[test]
fn an_entry_with_no_profiles_is_still_an_entry() {
    let e = parse_line(r#"{"at":"2026-10-30T16:15:00Z","profiles":{}}"#, T).unwrap();
    assert!(e.profiles.is_empty());
}

#[test]
fn the_history_errors_print_fixed_reasons_and_no_directory_is_silent() {
    let w = |e: HistoryError| e.warning();
    assert_eq!(
        w(HistoryError::LockBusy).as_deref(),
        Some("warning: usage history not updated (lock busy)")
    );
    assert_eq!(
        w(HistoryError::UnsafeFile).as_deref(),
        Some("warning: usage history not updated (unsafe file)")
    );
    assert_eq!(
        w(HistoryError::WriteFailed).as_deref(),
        Some("warning: usage history not updated (write failed)")
    );
    assert_eq!(
        w(HistoryError::TooLarge).as_deref(),
        Some("warning: usage history not updated (too large)")
    );
    assert_eq!(w(HistoryError::NoStateDir), None);
}

// ---------------------------------------------------------------------------
// Issue #1818 in the summary: no session limit against a session not read
// ---------------------------------------------------------------------------

fn no_limit(state: UsageState, w: f64) -> UsageReport {
    let mut r = report(state, None, Some(w));
    r.session_limit = false;
    r
}

#[test]
fn from_rows_a_login_with_no_session_limit_is_usable_now() {
    let rows = vec![row("free", no_limit(UsageState::Ok, 40.0))];
    let s = from_rows(T, T, &rows, None).unwrap();
    let pt = &s.points["free"];
    assert_eq!((pt.s, pt.w, pt.no_session_limit), (None, 40, true));
    assert!(super::capacity_summary::usable_now(pt, 95));
}

#[test]
fn from_rows_a_session_not_read_is_not_usable_now() {
    // `session_limit` stays `true`: the parser saw no evidence either way.
    let rows = vec![row("blank", report(UsageState::Ok, None, Some(40.0)))];
    let s = from_rows(T, T, &rows, None).unwrap();
    let pt = &s.points["blank"];
    assert_eq!((pt.s, pt.no_session_limit), (None, false));
    assert!(!super::capacity_summary::usable_now(pt, 95));
}

#[test]
fn from_rows_no_session_limit_with_a_full_week_is_not_usable_now() {
    let rows = vec![row("spent", no_limit(UsageState::WeeklyExhausted, 100.0))];
    let s = from_rows(T, T, &rows, None).unwrap();
    assert!(!super::capacity_summary::usable_now(&s.points["spent"], 95));
}

#[test]
fn a_session_figure_wins_over_a_stray_no_limit_flag() {
    let mut r = report(UsageState::Ok, Some(99.0), Some(10.0));
    r.session_limit = false;
    let pt = entry_of(&r).unwrap();
    assert!(!pt.no_session_limit, "a session window is a session limit");
    assert!(!super::capacity_summary::usable_now(&pt, 95));
}

#[test]
fn the_history_line_keeps_no_session_limit_writer_and_unknowns() {
    let rows = vec![
        row("free", no_limit(UsageState::Ok, 40.0)),
        row("blank", report(UsageState::Ok, None, Some(40.0))),
        row("slow", UsageReport::empty(UsageState::Timeout)),
    ];
    let e = from_rows(T, T, &rows, None).unwrap().entry(Writer::Usage);
    let line = to_line(&e);
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["by"], "usage");
    assert_eq!(v["unknown"], serde_json::json!(["slow"]));
    assert_eq!(v["profiles"]["free"]["session_limit"], false);
    assert!(
        v["profiles"]["blank"].get("session_limit").is_none(),
        "an unread session writes no flag: {line}"
    );
    let back = parse_line(&line, T).unwrap();
    assert_eq!(back, e);
    assert!(back.profiles["free"].no_session_limit);
    assert!(!back.profiles["blank"].no_session_limit);
}

#[test]
fn an_older_line_without_the_new_keys_reads_as_before() {
    let line = r#"{"at":"2026-10-30T16:15:00Z","profiles":{"a":{"s":null,"w":10,"w_reset":null}}}"#;
    let e = parse_line(line, T).unwrap();
    assert_eq!(e.by, None);
    assert!(e.unknown.is_empty());
    assert!(!e.profiles["a"].no_session_limit, "missing means not read");
}

#[test]
fn a_no_limit_flag_beside_a_session_figure_is_ignored_on_read() {
    let line = r#"{"at":"2026-10-30T16:15:00Z","by":"rotate","unknown":["b","a"],"profiles":{"a":{"s":5,"session_limit":false,"w":10,"w_reset":null}}}"#;
    let e = parse_line(line, T).unwrap();
    assert!(!e.profiles["a"].no_session_limit);
    assert_eq!(e.by, Some(Writer::Rotate));
    assert_eq!(e.unknown, ["b"], "a name with a reading is not unknown");
}

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod file {
    use super::super::usage_history::{
        FILE, HistoryError, LOCK, READ_REFUSED, append_with, read_all, read_recent,
        skipped_warning, to_line,
    };
    use super::{DAY, HOUR, T, entry, p};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const MIB: usize = 1024 * 1024;

    struct Root {
        _t: tempfile::TempDir,
        root: PathBuf,
        state: PathBuf,
    }

    impl Root {
        fn new() -> Self {
            let t = tempfile::tempdir().unwrap();
            let root = t.path().to_path_buf();
            let state = root.join(".auto-switch.d");
            fs::create_dir(&state).unwrap();
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
            Self { _t: t, root, state }
        }

        fn history(&self) -> PathBuf {
            self.state.join(FILE)
        }

        fn lock(&self) -> PathBuf {
            self.state.join(LOCK)
        }

        fn lines(&self) -> Vec<String> {
            fs::read_to_string(self.history())
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        fn seed(&self, body: &str) {
            fs::write(self.history(), body).unwrap();
            fs::set_permissions(self.history(), fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn one(at: i64, w: u8) -> super::Entry {
        entry(at, &[("work", p(Some(1), w, None))])
    }

    /// `append` with no credential variable, whatever the test's own
    /// environment holds.
    fn append(root: &Path, e: &super::Entry, now: i64) -> Result<(), HistoryError> {
        append_with(root, e, now, None)
    }

    /// The newest readable entry.
    fn newest(root: &Path, now: i64) -> Option<super::Entry> {
        read_all(root, now).0.pop()
    }

    fn refused() -> Option<String> {
        Some(READ_REFUSED.to_string())
    }

    /// Runs `f` on a thread; a FIFO that blocks the open fails the test
    /// instead of hanging it.
    fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(secs))
            .expect("did not finish: a FIFO must not block")
    }

    fn mkfifo(path: &Path) {
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    }

    #[test]
    fn the_file_names_are_the_documented_ones() {
        assert_eq!(FILE, "usage-history.jsonl");
        assert_eq!(LOCK, "usage-history.lock");
    }

    #[test]
    fn append_needs_a_private_state_directory() {
        let r = Root::new();
        append(&r.root, &one(T, 1), T).unwrap();
        let linked = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&r.state, linked.path().join(".auto-switch.d")).unwrap();
        assert_eq!(
            append(linked.path(), &one(T, 2), T),
            Err(HistoryError::NoStateDir),
            "a symlinked directory"
        );
        assert_eq!(r.lines().len(), 1);
        let open = Root::new();
        fs::set_permissions(&open.state, fs::Permissions::from_mode(0o750)).unwrap();
        assert_eq!(
            append(&open.root, &one(T, 2), T),
            Err(HistoryError::NoStateDir)
        );
        assert!(!open.history().exists());
    }

    #[test]
    fn append_writes_nothing_while_a_credential_variable_is_set() {
        let r = Root::new();
        assert_eq!(
            append_with(&r.root, &one(T, 1), T, Some("ANTHROPIC_API_KEY")),
            Err(HistoryError::CredentialVar)
        );
        assert_eq!(HistoryError::CredentialVar.warning(), None);
        assert!(!r.history().exists());
        assert!(!r.lock().exists());
    }

    #[test]
    fn an_untrusted_root_is_neither_written_nor_read() {
        let r = Root::new();
        r.seed(&format!("{}\n", to_line(&one(T - HOUR, 1))));
        fs::set_permissions(&r.root, fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(
            append(&r.root, &one(T, 2), T),
            Err(HistoryError::NoStateDir)
        );
        assert!(!r.lock().exists(), "nothing created under the root");
        assert_eq!(read_all(&r.root, T), (Vec::new(), refused()));
        assert_eq!(read_recent(&r.root, T, 6 * HOUR), (Vec::new(), refused()));
        fs::set_permissions(&r.root, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(r.lines().len(), 1);
    }

    #[test]
    fn a_missing_history_or_state_directory_is_empty_without_a_warning() {
        let r = Root::new();
        assert_eq!(read_all(&r.root, T), (Vec::new(), None));
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(read_all(bare.path(), T), (Vec::new(), None));
        assert_eq!(
            read_all(&bare.path().join("absent"), T),
            (Vec::new(), None),
            "no root at all"
        );
    }

    #[test]
    fn a_history_writable_by_group_or_others_is_refused_with_a_warning() {
        for mode in [0o664, 0o646, 0o622] {
            let r = Root::new();
            r.seed(&format!("{}\n", to_line(&one(T - HOUR, 1))));
            fs::set_permissions(r.history(), fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(read_all(&r.root, T), (Vec::new(), refused()), "{mode:o}");
        }
        assert_eq!(READ_REFUSED, "warning: usage history ignored (unsafe file)");
    }

    #[test]
    fn readers_and_writers_refuse_the_same_files() {
        // Group- or other-readable only: once read but never appended to.
        for mode in [0o640, 0o644, 0o604] {
            let r = Root::new();
            r.seed(&format!("{}\n", to_line(&one(T - HOUR, 1))));
            fs::set_permissions(r.history(), fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(read_all(&r.root, T), (Vec::new(), refused()), "{mode:o}");
            assert_eq!(
                append(&r.root, &one(T, 2), T),
                Err(HistoryError::UnsafeFile),
                "{mode:o}"
            );
        }
        // A second hard link.
        let r = Root::new();
        r.seed(&format!("{}\n", to_line(&one(T - HOUR, 1))));
        fs::hard_link(r.history(), r.root.join("other")).unwrap();
        assert_eq!(read_all(&r.root, T), (Vec::new(), refused()));
        assert_eq!(
            append(&r.root, &one(T, 2), T),
            Err(HistoryError::UnsafeFile)
        );
        // A 0600 file with one link is accepted by both.
        let r = Root::new();
        r.seed(&format!("{}\n", to_line(&one(T - HOUR, 1))));
        assert_eq!(read_all(&r.root, T).0.len(), 1);
        assert_eq!(append(&r.root, &one(T, 2), T), Ok(()));
        assert_eq!(
            read_all(&r.root, T),
            (vec![one(T - HOUR, 1), one(T, 2)], None)
        );
    }

    #[test]
    fn unreadable_lines_are_counted_in_one_warning() {
        let r = Root::new();
        let mut body =
            format!("garbage\n{}\n{{\"at\":\n\n", to_line(&one(T - HOUR, 1))).into_bytes();
        body.extend_from_slice(b"\xff\xfe\n");
        fs::write(r.history(), &body).unwrap();
        fs::set_permissions(r.history(), fs::Permissions::from_mode(0o600)).unwrap();
        let (entries, warning) = read_all(&r.root, T);
        assert_eq!(entries, [one(T - HOUR, 1)]);
        assert_eq!(
            warning,
            Some(skipped_warning(3)),
            "blank lines are not counted"
        );
        assert_eq!(
            skipped_warning(3),
            "warning: 3 unreadable usage history lines ignored"
        );
        assert_eq!(
            read_recent(&r.root, T, 6 * HOUR).1,
            Some(skipped_warning(3))
        );
    }

    #[test]
    fn a_fully_corrupted_history_does_not_look_empty() {
        let r = Root::new();
        r.seed("not json\nnor this\n");
        assert_eq!(read_all(&r.root, T), (Vec::new(), Some(skipped_warning(2))));
    }

    #[test]
    fn a_cut_last_line_is_ended_before_the_next_append() {
        let r = Root::new();
        let kept = to_line(&one(T - HOUR, 1));
        r.seed(&format!("{kept}\n{{\"at\":\"20"));
        append(&r.root, &one(T, 2), T).unwrap();
        assert_eq!(
            read_all(&r.root, T).0,
            [one(T - HOUR, 1), one(T, 2)],
            "the new line is read whole"
        );
    }

    #[test]
    fn a_failed_trim_still_appends_and_says_so() {
        let r = Root::new();
        let old = to_line(&one(T - 9 * DAY, 1));
        r.seed(&format!("{old}\n"));
        fs::write(r.lock(), "").unwrap();
        fs::set_permissions(r.lock(), fs::Permissions::from_mode(0o600)).unwrap();
        // A read-only directory: the atomic rewrite cannot create its file.
        fs::set_permissions(&r.state, fs::Permissions::from_mode(0o500)).unwrap();
        let got = append(&r.root, &one(T, 2), T);
        fs::set_permissions(&r.state, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(got, Err(HistoryError::TrimFailed));
        assert_eq!(r.lines(), [old, to_line(&one(T, 2))]);
        assert_eq!(
            HistoryError::TrimFailed.warning().as_deref(),
            Some("warning: usage history not trimmed")
        );
    }

    #[test]
    fn append_writes_one_private_line_per_entry() {
        let r = Root::new();
        append(&r.root, &one(T - HOUR, 10), T).unwrap();
        append(&r.root, &one(T, 20), T).unwrap();
        assert_eq!(
            r.lines(),
            [to_line(&one(T - HOUR, 10)), to_line(&one(T, 20))]
        );
        let body = fs::read_to_string(r.history()).unwrap();
        assert!(body.ends_with('\n'));
        let m = fs::metadata(r.history()).unwrap();
        assert_eq!(m.mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(r.lock()).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn append_without_a_state_directory_creates_nothing() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(
            append(t.path(), &one(T, 1), T),
            Err(HistoryError::NoStateDir)
        );
        assert!(!t.path().join(".auto-switch.d").exists());
    }

    #[test]
    fn append_refuses_a_symlinked_history_and_leaves_its_target_alone() {
        let r = Root::new();
        let target = r.root.join("target");
        fs::write(&target, "keep\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, r.history()).unwrap();
        assert_eq!(
            append(&r.root, &one(T, 1), T),
            Err(HistoryError::UnsafeFile)
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep\n");
    }

    #[test]
    fn append_refuses_a_symlinked_lock() {
        let r = Root::new();
        let target = r.root.join("target");
        fs::write(&target, "").unwrap();
        std::os::unix::fs::symlink(&target, r.lock()).unwrap();
        assert_eq!(
            append(&r.root, &one(T, 1), T),
            Err(HistoryError::UnsafeFile)
        );
        assert!(!r.history().exists() || r.lines().is_empty());
    }

    #[test]
    fn append_refuses_a_hard_linked_history() {
        let r = Root::new();
        r.seed("");
        fs::hard_link(r.history(), r.root.join("other")).unwrap();
        assert_eq!(
            append(&r.root, &one(T, 1), T),
            Err(HistoryError::UnsafeFile)
        );
        assert_eq!(fs::read_to_string(r.root.join("other")).unwrap(), "");
    }

    #[test]
    fn append_refuses_a_history_with_group_or_other_bits() {
        for mode in [0o640, 0o660, 0o604] {
            let r = Root::new();
            r.seed("");
            fs::set_permissions(r.history(), fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(
                append(&r.root, &one(T, 1), T),
                Err(HistoryError::UnsafeFile),
                "{mode:o}"
            );
        }
    }

    #[test]
    fn a_fifo_in_place_of_the_history_or_the_lock_does_not_hang() {
        let r = Root::new();
        mkfifo(&r.history());
        let root = r.root.clone();
        let got = within(10, move || append(&root, &one(T, 1), T));
        assert_eq!(got, Err(HistoryError::UnsafeFile));
        let root = r.root.clone();
        assert_eq!(
            within(10, move || read_all(&root, T)),
            (Vec::new(), refused())
        );
        let root = r.root.clone();
        assert_eq!(
            within(10, move || read_recent(&root, T, 6 * HOUR)),
            (Vec::new(), refused())
        );

        let r = Root::new();
        mkfifo(&r.lock());
        let root = r.root.clone();
        let got = within(10, move || append(&root, &one(T, 1), T));
        assert_eq!(got, Err(HistoryError::UnsafeFile));
    }

    #[test]
    fn a_held_lock_skips_the_append_after_about_a_second() {
        let r = Root::new();
        append(&r.root, &one(T - HOUR, 1), T).unwrap();
        let held = fs::OpenOptions::new().write(true).open(r.lock()).unwrap();
        held.lock().unwrap();
        let start = Instant::now();
        assert_eq!(append(&r.root, &one(T, 2), T), Err(HistoryError::LockBusy));
        let waited = start.elapsed();
        assert!(waited >= Duration::from_millis(900), "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");
        assert_eq!(r.lines().len(), 1, "nothing written while locked");
        drop(held);
        append(&r.root, &one(T, 2), T).unwrap();
        assert_eq!(r.lines().len(), 2);
    }

    #[test]
    fn two_writers_at_once_give_every_line_whole() {
        const N: i64 = 50;
        let r = Root::new();
        let writers: Vec<_> = (0..2)
            .map(|k| {
                let root = r.root.clone();
                std::thread::spawn(move || {
                    for i in 0..N {
                        let e = entry(
                            T - DAY + k * 10_000 + i,
                            &[(if k == 0 { "left" } else { "right" }, p(Some(1), 2, None))],
                        );
                        append(&root, &e, T).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let lines = r.lines();
        assert_eq!(lines.len(), 2 * N as usize);
        for l in &lines {
            assert!(super::parse_line(l, T).is_some(), "torn line: {l}");
        }
    }

    #[test]
    fn entries_older_than_eight_days_are_trimmed_on_append() {
        let r = Root::new();
        let old = to_line(&one(T - 9 * DAY, 1));
        let kept = to_line(&one(T - 7 * DAY, 2));
        r.seed(&format!("{old}\n{kept}\n"));
        append(&r.root, &one(T, 3), T).unwrap();
        assert_eq!(r.lines(), [kept, to_line(&one(T, 3))]);
        assert_eq!(fs::metadata(r.history()).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_first_line_less_than_an_hour_past_eight_days_is_appended_to_not_rewritten() {
        let r = Root::new();
        let old = to_line(&one(T - 8 * DAY - 30 * 60, 1));
        r.seed(&format!("{old}\n"));
        let inode = fs::metadata(r.history()).unwrap().ino();
        append(&r.root, &one(T, 2), T).unwrap();
        assert_eq!(r.lines(), [old, to_line(&one(T, 2))]);
        assert_eq!(
            fs::metadata(r.history()).unwrap().ino(),
            inode,
            "appended in place, not replaced by a rewrite"
        );

        let r = Root::new();
        let old = to_line(&one(T - 8 * DAY - HOUR - 1, 1));
        r.seed(&format!("{old}\n"));
        append(&r.root, &one(T, 2), T).unwrap();
        assert_eq!(r.lines(), [to_line(&one(T, 2))], "over the slack: trimmed");
    }

    #[test]
    fn an_unreadable_first_line_triggers_a_rewrite_without_bad_lines() {
        let r = Root::new();
        let kept = to_line(&one(T - HOUR, 2));
        r.seed(&format!("garbage\n{kept}\n{{\"at\":\n"));
        append(&r.root, &one(T, 3), T).unwrap();
        assert_eq!(r.lines(), [kept, to_line(&one(T, 3))]);
    }

    /// About `mib` MiB of entries, one a minute up to `T - 1 min`, with 20
    /// profiles each so a line is about 1 KiB.
    fn big(r: &Root, mib: usize) -> usize {
        let profiles: Vec<(String, super::Point)> = (0..20)
            .map(|i| {
                (
                    format!("profile-number-{i:02}"),
                    p(Some(50), 50, Some(T + DAY)),
                )
            })
            .collect();
        let refs: Vec<(&str, super::Point)> = profiles
            .iter()
            .map(|(n, p)| (n.as_str(), p.clone()))
            .collect();
        let line_len = to_line(&entry(T, &refs)).len() + 1;
        let n = mib * MIB / line_len + 1;
        let mut f = fs::File::create(r.history()).unwrap();
        for i in (1..=n as i64).rev() {
            writeln!(f, "{}", to_line(&entry(T - i * 60, &refs))).unwrap();
        }
        drop(f);
        fs::set_permissions(r.history(), fs::Permissions::from_mode(0o600)).unwrap();
        n
    }

    #[test]
    fn a_five_mib_history_is_read_from_its_tail_and_trimmed_under_three_mib() {
        let r = Root::new();
        big(&r, 5);
        assert!(fs::metadata(r.history()).unwrap().len() > 5 * MIB as u64);

        let (recent, warning) = read_recent(&r.root, T, 6 * HOUR);
        assert_eq!(warning, None);
        assert_eq!(recent.len(), 360, "one a minute over six hours");
        assert_eq!(recent.first().unwrap().at, T - 6 * HOUR);
        assert_eq!(recent.last().unwrap().at, T - 60);
        assert_eq!(newest(&r.root, T).unwrap().at, T - 60);

        append(&r.root, &one(T, 7), T).unwrap();
        let len = fs::metadata(r.history()).unwrap().len();
        assert!(len < 3 * MIB as u64, "{len} bytes");
        let lines = r.lines();
        assert_eq!(lines.last().unwrap(), &to_line(&one(T, 7)));
        let before = super::parse_line(&lines[lines.len() - 2], T).unwrap();
        assert_eq!(
            before.at,
            T - 60,
            "the newest old entries are the ones kept"
        );
        assert_eq!(fs::metadata(r.history()).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_line_over_64_kib_is_skipped_and_its_neighbours_read() {
        let r = Root::new();
        let mut huge = one(T - 2 * HOUR, 1);
        for i in 0..2000 {
            huge.profiles
                .insert(format!("padding-profile-{i:05}"), p(Some(1), 1, None));
        }
        let huge = to_line(&huge);
        assert!(huge.len() > 64 * 1024 + 4096, "{}", huge.len());
        r.seed(&format!(
            "{}\n{huge}\n{}\n",
            to_line(&one(T - 3 * HOUR, 1)),
            to_line(&one(T - HOUR, 2))
        ));
        let got: Vec<i64> = read_recent(&r.root, T, 6 * HOUR)
            .0
            .iter()
            .map(|e| e.at)
            .collect();
        assert_eq!(got, [T - 3 * HOUR, T - HOUR]);
    }

    #[test]
    fn read_recent_is_the_window_before_now_oldest_first() {
        let r = Root::new();
        let body: String = [T - 7 * HOUR, T - HOUR, T - 6 * HOUR, T, T + HOUR]
            .iter()
            .map(|&at| format!("{}\n", to_line(&one(at, 1))))
            .collect();
        r.seed(&body);
        let got: Vec<i64> = read_recent(&r.root, T, 6 * HOUR)
            .0
            .iter()
            .map(|e| e.at)
            .collect();
        assert_eq!(got, [T - 6 * HOUR, T - HOUR], "at >= now is not history");
    }

    #[test]
    fn the_newest_readable_entry_is_last_in_read_all() {
        let r = Root::new();
        assert!(newest(&r.root, T).is_none(), "no file");
        r.seed(&format!(
            "{}\n{}\nnot json\n",
            to_line(&one(T - HOUR, 1)),
            to_line(&one(T - 30 * 60, 2))
        ));
        assert_eq!(newest(&r.root, T).unwrap(), one(T - 30 * 60, 2));
    }

    #[test]
    fn readers_refuse_a_symlinked_history() {
        let r = Root::new();
        let target = r.root.join("target");
        fs::write(&target, format!("{}\n", to_line(&one(T - HOUR, 1)))).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, r.history()).unwrap();
        assert_eq!(read_all(&r.root, T), (Vec::new(), refused()));
        assert_eq!(read_recent(&r.root, T, 6 * HOUR), (Vec::new(), refused()));
    }
}

// ---------------------------------------------------------------------------
// Crusty round 5: reading times after #1844, and rows whose direct read
// failed
// ---------------------------------------------------------------------------

use super::usage_cmd::ErrorReason;
use super::usage_history::READING_MAX_AGE;
use super::usage_source::Source;

/// A row read from a passive reading stored `age` seconds before `T`.
fn passive(name: &str, s: f64, w: f64, age: i64) -> Row {
    let mut rep = report(UsageState::Ok, Some(s), Some(w));
    rep.observed_at = Some(T - age);
    let mut r = row(name, rep);
    r.source = Some(Source::Passive);
    r
}

#[test]
fn the_reading_age_limit_is_five_minutes() {
    // Pinned: the reference says 300 seconds, and that a reading at the
    // limit is out by under 2% of the 4.5 h the rate needs at least.
    assert_eq!(READING_MAX_AGE, 300);
}

#[test]
fn a_passive_reading_up_to_300_s_old_is_recorded_at_the_cycle_time() {
    let rows = vec![
        passive("fresh", 10.0, 20.0, 0),
        passive("edge", 11.0, 21.0, READING_MAX_AGE),
        row("direct", report(UsageState::Ok, Some(12.0), Some(22.0))),
    ];
    let s = from_rows(T, T, &rows, None).unwrap();
    assert!(s.unknown.is_empty(), "{:?}", s.unknown);
    let e = s.entry(Writer::Usage);
    assert_eq!(e.at, T, "the line's time is the cycle's");
    assert_eq!(e.profiles["edge"].w, 21);
}

#[test]
fn a_reading_age_is_judged_when_the_run_started_reading_not_when_it_ended() {
    // Crusty round 6: `profile usage` accepts passive readings against the
    // time it started reading, and a `claude` fallback can take tens of
    // seconds before the line's `at`. A reading exactly 300 s old at the
    // start is shown as read in the table, so it is counted too, at `at`.
    let start = T - 45;
    let mut edge = passive("edge", 11.0, 21.0, 0);
    edge.report.observed_at = Some(start - READING_MAX_AGE);
    let mut over = passive("over", 13.0, 23.0, 0);
    over.report.observed_at = Some(start - READING_MAX_AGE - 1);
    let rows = vec![
        edge,
        over,
        row("direct", report(UsageState::Ok, Some(12.0), Some(22.0))),
    ];
    let s = from_rows(start, T, &rows, None).unwrap();
    assert_eq!(s.unknown, ["over"], "only the one over 300 s at the start");
    let e = s.entry(Writer::Usage);
    assert_eq!(
        e.at, T,
        "the line's time is still when the readings were in"
    );
    assert_eq!(e.profiles["edge"].w, 21);
    // Judged on the later clock, `edge` would have been unknown.
    assert_eq!(
        from_rows(T, T, &rows, None).unwrap().unknown,
        ["edge", "over"]
    );
}

#[test]
fn a_passive_reading_older_than_300_s_is_unknown_and_never_a_history_point() {
    // AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS allows up to an hour. A
    // reading that old is never stamped with the cycle's time: it is
    // unknown, in the summary and in the line.
    for age in [READING_MAX_AGE + 1, HOUR] {
        let rows = vec![
            passive("stale", 10.0, 20.0, age),
            row("direct", report(UsageState::Ok, Some(12.0), Some(22.0))),
        ];
        let s = from_rows(T, T, &rows, None).unwrap();
        assert_eq!(s.unknown, ["stale"], "age {age}");
        let e = s.entry(Writer::Usage);
        assert!(!e.profiles.contains_key("stale"), "age {age}");
        assert_eq!(e.unknown, ["stale"], "age {age}");
        let line = to_line(&e);
        assert!(line.contains(r#""unknown":["stale"]"#), "{line}");
    }
}

#[test]
fn a_stale_passive_reading_adds_nothing_to_the_rate() {
    // Six hours of history in which `stale` used 0 points. Its reading now,
    // an hour old, says 60%. Counted at `T` it would add 60 points to the
    // rate; as unknown it adds none.
    use super::capacity_summary::{Inputs, compute};
    use super::capacity_weight::Weights;
    let start = entry(
        T - 5 * HOUR,
        &[
            ("stale", p(Some(0), 0, None)),
            ("direct", p(Some(0), 10, None)),
        ],
    );
    let rows = vec![
        passive("stale", 60.0, 60.0, HOUR),
        row("direct", report(UsageState::Ok, Some(20.0), Some(20.0))),
    ];
    let current = from_rows(T, T, &rows, None).unwrap();
    let only_direct = from_rows(T, T, &rows[1..], None).unwrap();
    let js = |c: &Snapshot| {
        let s = compute(&Inputs {
            current: c,
            weights: &Weights::default(),
            threshold: 95,
            history: std::slice::from_ref(&start),
            now: T,
        });
        serde_json::to_value(&s).unwrap()
    };
    let (with, without) = (js(&current), js(&only_direct));
    assert!(
        with["rate_login_weeks_per_hour"].is_number(),
        "the rate is taken: {with}"
    );
    for key in [
        "weekly_headroom_login_weeks",
        "weekly_headroom_pct",
        "valid_readings",
        "rate_login_weeks_per_hour",
        "runway_hours",
    ] {
        assert_eq!(with[key], without[key], "{key}");
    }
    assert_eq!(with["unknown"], serde_json::json!(["stale"]));
}

/// A row as `profile usage` builds it when the direct read failed with
/// `why` and the mode starts no `claude` (`Probe::direct_error`).
fn direct_error(name: &str, why: &'static str) -> Row {
    let mut r = row(name, UsageReport::empty(UsageState::Error));
    r.error = Some(ErrorReason::Direct(why));
    r.source = Some(Source::Direct);
    r.direct_failure = Some(why);
    r
}

#[test]
fn a_row_whose_direct_read_failed_is_unknown_and_in_no_figure() {
    #[cfg(unix)]
    assert_eq!(
        super::usage_direct::reason::REFRESH_NOT_SAVED,
        "refresh not saved"
    );
    let mut fell_back_and_timed_out = row("slow", UsageReport::empty(UsageState::Timeout));
    fell_back_and_timed_out.source = Some(Source::Fallback);
    fell_back_and_timed_out.direct_failure = Some("http 5xx");
    let mut fell_back_and_read = row("rescued", report(UsageState::Ok, Some(30.0), Some(40.0)));
    fell_back_and_read.source = Some(Source::Fallback);
    fell_back_and_read.direct_failure = Some("refresh not saved");
    let rows = vec![
        direct_error("unsaved", "refresh not saved"),
        direct_error("expired", "token expired"),
        fell_back_and_timed_out,
        fell_back_and_read,
        row("work", report(UsageState::Ok, Some(20.0), Some(30.0))),
    ];
    let s = from_rows(T, T, &rows, None).unwrap();
    assert_eq!(s.unknown, ["expired", "slow", "unsaved"]);
    assert_eq!(
        s.points.keys().map(String::as_str).collect::<Vec<_>>(),
        ["rescued", "work"],
        "a fallback that read the login is that reading"
    );
    let summary = super::capacity_summary::compute(&super::capacity_summary::Inputs {
        current: &s,
        weights: &super::capacity_weight::Weights::default(),
        threshold: 95,
        history: &[],
        now: T,
    });
    let lines = summary.text_lines();
    let unknown = lines.iter().find(|l| l.starts_with("unknown")).unwrap();
    assert_eq!(
        unknown,
        "unknown              3 profiles (no valid reading; excluded from all figures)"
    );
    let v = serde_json::to_value(&summary).unwrap();
    assert_eq!(
        v["valid_readings"],
        serde_json::json!(2),
        "two logins, not five"
    );
    assert_eq!(
        v["unknown"],
        serde_json::json!(["expired", "slow", "unsaved"])
    );
}

// ---------------------------------------------------------------------------
// Issue #1970: a stale passive reading is never written as new history
// ---------------------------------------------------------------------------
//
// A passive reading is the figure Claude's status line stored. A cached
// figure that reappears after a higher one made the week look as if it fell
// and rose again, and each fall was counted as a reset. Before a line is
// written, each passive reading is compared with that login's newest
// reading in the history. It is stale when (1) its week has ended
// (`w_reset` at or before the line's `at`), (2) it is the same week within
// `SAME_WEEK_SLACK` and its `w` is lower, or (3) its week is more than
// `SAME_WEEK_SLACK` earlier than the recorded one. A stale reading is
// replaced by the newest recorded reading while that one's week has not
// ended, so the login, which is one in use, stays in the line; otherwise
// its login is listed as unknown.

use super::usage_history::{SAME_WEEK_SLACK, stale};

/// The recorded week in these tests resets on 2026-11-04T16:15:00Z.
const WEEK: i64 = T + 5 * DAY;

fn pp(w: u8, w_reset: Option<i64>) -> Point {
    Point {
        passive: true,
        ..p(Some(10), w, w_reset)
    }
}

#[test]
fn the_same_week_slack_is_one_hour() {
    // Pinned: the reference says "within one hour of each other".
    assert_eq!(SAME_WEEK_SLACK, 3600);
}

#[test]
fn a_passive_reading_whose_week_has_ended_is_stale_even_with_no_history() {
    assert!(stale(None, &pp(40, Some(T)), T), "w_reset at the line's at");
    assert!(stale(None, &pp(40, Some(T - 120)), T));
    assert!(!stale(None, &pp(40, Some(T + 1)), T));
    let prev = p(Some(1), 10, Some(T - 60));
    assert!(stale(Some(&prev), &pp(90, Some(T - 60)), T));
}

#[test]
fn a_lower_passive_reading_in_the_same_week_is_stale() {
    let prev = p(Some(1), 50, Some(WEEK));
    assert!(stale(Some(&prev), &pp(38, Some(WEEK)), T));
    assert!(stale(Some(&prev), &pp(49, Some(WEEK)), T));
}

#[test]
fn an_equal_or_higher_passive_reading_in_the_same_week_is_not_stale() {
    let prev = p(Some(1), 50, Some(WEEK));
    assert!(!stale(Some(&prev), &pp(50, Some(WEEK)), T));
    assert!(!stale(Some(&prev), &pp(52, Some(WEEK)), T));
}

#[test]
fn reset_times_up_to_the_slack_apart_are_the_same_week() {
    let prev = p(Some(1), 50, Some(WEEK));
    for d in [SAME_WEEK_SLACK, -SAME_WEEK_SLACK, 1, -1] {
        assert!(stale(Some(&prev), &pp(38, Some(WEEK + d)), T), "{d}");
        assert!(!stale(Some(&prev), &pp(55, Some(WEEK + d)), T), "{d}");
    }
}

#[test]
fn a_passive_reading_from_an_earlier_week_is_stale_whatever_its_figure() {
    let prev = p(Some(1), 5, Some(WEEK));
    let earlier = WEEK - SAME_WEEK_SLACK - 1;
    assert!(stale(Some(&prev), &pp(90, Some(earlier)), T));
    assert!(stale(Some(&prev), &pp(3, Some(WEEK - 7 * DAY)), T));
}

#[test]
fn a_passive_reading_from_a_later_week_is_not_stale() {
    // The week reset since the recorded reading: a lower figure is the new
    // week's, not a stale one.
    let prev = p(Some(1), 90, Some(WEEK));
    assert!(!stale(
        Some(&prev),
        &pp(4, Some(WEEK + SAME_WEEK_SLACK + 1)),
        T
    ));
    assert!(!stale(Some(&prev), &pp(4, Some(WEEK + 7 * DAY)), T));
}

#[test]
fn an_unknown_reset_on_either_side_is_never_stale_unless_the_week_ended() {
    // The passive reading's week has not ended, so rule 1 does not apply.
    let known = p(Some(1), 50, Some(WEEK));
    let unknown = p(Some(1), 50, None);
    assert!(!stale(Some(&known), &pp(38, None), T), "own reset unknown");
    assert!(!stale(Some(&unknown), &pp(38, None), T), "both unknown");
    assert!(
        !stale(Some(&unknown), &pp(38, Some(WEEK)), T),
        "recorded reset unknown: only rule 1"
    );
    assert!(!stale(None, &pp(38, None), T), "no history, reset unknown");
    // Rule 1 still applies when only the recorded reset is unknown.
    assert!(stale(Some(&unknown), &pp(38, Some(T - 1)), T));
}

#[test]
fn stale_does_not_panic_on_extreme_resets() {
    let extremes = [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX];
    for &a in &extremes {
        for &b in &extremes {
            for &at in &extremes {
                let prev = p(Some(1), 50, Some(a));
                let _ = stale(Some(&prev), &pp(38, Some(b)), at);
                let _ = stale(Some(&prev), &pp(38, None), at);
                let _ = stale(None, &pp(38, Some(b)), at);
                let _ = stale(Some(&p(Some(1), 50, None)), &pp(38, Some(b)), at);
            }
        }
    }
}

#[test]
fn from_rows_marks_passive_readings_and_only_those() {
    let rows = vec![
        passive("cached", 10.0, 20.0, 30),
        row("direct", report(UsageState::Ok, Some(12.0), Some(22.0))),
    ];
    let s = from_rows(T, T, &rows, None).unwrap();
    assert!(s.points["cached"].passive);
    assert!(!s.points["direct"].passive);
}

#[test]
fn passive_is_never_written_and_parses_false() {
    let direct = entry(T, &[("a", p(Some(3), 40, Some(WEEK)))]);
    let cached = entry(T, &[("a", pp(40, Some(WEEK)))]);
    let mut cached_s3 = cached.clone();
    cached_s3.profiles.get_mut("a").unwrap().s = Some(3);
    assert_eq!(to_line(&cached_s3), to_line(&direct), "byte-identical");
    assert!(!to_line(&cached_s3).contains("passive"));
    let back = parse_line(&to_line(&cached_s3), T).unwrap();
    assert!(!back.profiles["a"].passive);
    assert_eq!(back, direct);
}

#[test]
fn a_history_line_cannot_mark_a_point_passive() {
    let e = parse_line(
        r#"{"at":"2026-10-30T16:15:00Z","profiles":{"a":{"s":1,"w":4,"w_reset":null,"passive":true}},"passive":true}"#,
        T,
    )
    .unwrap();
    assert!(!e.profiles["a"].passive);
    assert_eq!(e.profiles["a"].w, 4);
}

#[test]
fn an_older_line_still_parses_unchanged() {
    let line = r#"{"at":"2026-10-30T16:15:00Z","profiles":{"a":{"s":5,"w":10,"w_reset":"2026-11-04T16:15:00Z"}}}"#;
    let e = parse_line(line, T).unwrap();
    assert_eq!(e.profiles["a"], p(Some(5), 10, Some(T + 5 * DAY)));
    assert_eq!(to_line(&e), line);
}

#[cfg(unix)]
mod stale_file {
    use super::super::usage_history::{
        Entry, FILE, Point, STALE_LOOKBACK, Writer, append_with, parse_line, read_all, to_line,
    };
    use super::{DAY, HOUR, T, WEEK, entry, p, pp};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct Root {
        _t: tempfile::TempDir,
        root: PathBuf,
        state: PathBuf,
    }

    impl Root {
        fn new() -> Self {
            let t = tempfile::tempdir().unwrap();
            let root = t.path().to_path_buf();
            let state = root.join(".auto-switch.d");
            fs::create_dir(&state).unwrap();
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
            Self { _t: t, root, state }
        }

        fn history(&self) -> PathBuf {
            self.state.join(FILE)
        }

        fn lines(&self) -> Vec<String> {
            fs::read_to_string(self.history())
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        fn seed(&self, body: &str) {
            fs::write(self.history(), body).unwrap();
            fs::set_permissions(self.history(), fs::Permissions::from_mode(0o600)).unwrap();
        }

        /// Appends with no credential variable, whatever the test's own
        /// environment holds.
        fn append(&self, e: &Entry, now: i64) {
            append_with(&self.root, e, now, None).unwrap();
        }

        /// The last line, read back.
        fn last(&self, now: i64) -> Entry {
            read_all(&self.root, now).0.pop().expect("a line")
        }
    }

    fn by(w: Writer, at: i64, points: &[(&str, Point)], unknown: &[&str]) -> Entry {
        Entry {
            by: Some(w),
            unknown: unknown.iter().map(|n| n.to_string()).collect(),
            ..entry(at, points)
        }
    }

    #[test]
    fn a_stale_passive_reading_is_replaced_by_the_recorded_one() {
        let r = Root::new();
        r.append(
            &by(
                Writer::Usage,
                T - HOUR,
                &[("work", p(Some(1), 50, Some(WEEK)))],
                &[],
            ),
            T - HOUR,
        );

        // A cached 38 for the same week: the recorded reading, session
        // figure and reset included, is written in its place, and the login
        // is not unknown. The other login's passive reading is kept.
        r.append(
            &by(
                Writer::Rotate,
                T,
                &[("work", pp(38, Some(WEEK))), ("home", pp(20, Some(WEEK)))],
                &[],
            ),
            T,
        );
        let e = r.last(T);
        assert_eq!(e.at, T);
        assert_eq!(e.profiles["work"], p(Some(1), 50, Some(WEEK)));
        assert_eq!(e.profiles["home"].w, 20);
        assert!(e.unknown.is_empty(), "{:?}", e.unknown);
        assert!(!r.lines().last().unwrap().contains(r#""w":38"#));

        // A passive 52 for the same week is written.
        r.append(
            &by(Writer::Rotate, T + 60, &[("work", pp(52, Some(WEEK)))], &[]),
            T + 60,
        );
        let e = r.last(T + 60);
        assert_eq!(e.profiles["work"].w, 52);
        assert!(e.unknown.is_empty());

        // A reading taken during the run is never filtered, even lower.
        r.append(
            &by(
                Writer::Usage,
                T + 120,
                &[("work", p(Some(1), 38, Some(WEEK)))],
                &[],
            ),
            T + 120,
        );
        assert_eq!(r.last(T + 120).profiles["work"].w, 38);
        assert_eq!(r.lines().len(), 4);
    }

    #[test]
    fn the_unknown_list_stays_sorted_without_duplicates() {
        let r = Root::new();
        r.append(
            &entry(T - HOUR, &[("mid", p(Some(1), 50, Some(T - 120)))]),
            T - HOUR,
        );
        // `mid`'s passive reading is from a week that ended, and the
        // recorded one is too: nothing known is left, so it is unknown.
        r.append(
            &by(
                Writer::Rotate,
                T,
                &[("mid", pp(38, Some(T - 60)))],
                &["zed", "alpha"],
            ),
            T,
        );
        let e = r.last(T);
        assert!(e.profiles.is_empty());
        assert_eq!(e.unknown, ["alpha", "mid", "zed"]);
    }

    #[test]
    fn a_passive_reading_with_no_history_is_kept_unless_its_week_has_ended() {
        let r = Root::new();
        r.append(&entry(T, &[("work", pp(20, Some(T + DAY)))]), T);
        assert_eq!(r.last(T).profiles["work"].w, 20);

        let r = Root::new();
        r.append(&entry(T, &[("work", pp(20, Some(T - 60)))]), T);
        let e = r.last(T);
        assert!(e.profiles.is_empty());
        assert_eq!(e.unknown, ["work"]);

        // A recorded reading with no reset time: only the first rule
        // applies, so a lower passive reading is written.
        let r = Root::new();
        r.append(
            &entry(T - HOUR, &[("work", p(Some(1), 50, None))]),
            T - HOUR,
        );
        r.append(&entry(T, &[("work", pp(38, Some(WEEK)))]), T);
        assert_eq!(r.last(T).profiles["work"].w, 38);
    }

    #[test]
    fn the_newest_recorded_reading_is_the_one_compared() {
        // 30, then 40 in the same week: a passive 35 is below the newest
        // reading, so it is stale, though it is above the older one.
        let r = Root::new();
        r.append(
            &entry(T - 3 * HOUR, &[("work", p(Some(1), 30, Some(WEEK)))]),
            T - 3 * HOUR,
        );
        r.append(
            &entry(T - HOUR, &[("work", p(Some(1), 40, Some(WEEK)))]),
            T - HOUR,
        );
        r.append(&entry(T, &[("work", pp(35, Some(WEEK)))]), T);
        let e = r.last(T);
        assert_eq!(e.profiles["work"].w, 40, "the newest, not the older 30");
        assert!(e.unknown.is_empty());

        // An older 60 from the last week, then 30 in this one: a passive
        // 35 is above the newest reading and is written.
        let r = Root::new();
        r.append(
            &entry(
                T - 3 * HOUR,
                &[("work", p(Some(1), 60, Some(WEEK - 7 * DAY)))],
            ),
            T - 3 * HOUR,
        );
        r.append(
            &entry(T - HOUR, &[("work", p(Some(1), 30, Some(WEEK)))]),
            T - HOUR,
        );
        r.append(&entry(T, &[("work", pp(35, Some(WEEK)))]), T);
        assert_eq!(r.last(T).profiles["work"].w, 35);
    }

    #[test]
    fn a_stale_passive_reading_is_filtered_when_the_history_is_rewritten() {
        let r = Root::new();
        let old = to_line(&entry(T - 9 * DAY, &[("work", p(Some(1), 5, None))]));
        let kept = to_line(&entry(T - HOUR, &[("work", p(Some(1), 50, Some(WEEK)))]));
        r.seed(&format!("{old}\n{kept}\n"));
        r.append(
            &by(Writer::Rotate, T, &[("work", pp(38, Some(WEEK)))], &[]),
            T,
        );
        let lines = r.lines();
        assert_eq!(lines.len(), 2, "the over-age line was trimmed: {lines:?}");
        assert_eq!(lines[0], kept);
        let e = parse_line(&lines[1], T).unwrap();
        assert_eq!(e.profiles["work"].w, 50);
        assert!(e.unknown.is_empty());
    }

    /// A line of about 1 KiB holding 20 other logins.
    fn padding(at: i64) -> String {
        let names: Vec<String> = (0..20).map(|i| format!("padding-number-{i:02}")).collect();
        let points: Vec<(&str, Point)> = names
            .iter()
            .map(|n| (n.as_str(), p(Some(1), 1, Some(WEEK))))
            .collect();
        to_line(&entry(at, &points))
    }

    #[test]
    fn the_lookback_is_256_kib() {
        assert_eq!(STALE_LOOKBACK, 256 * 1024);
    }

    #[test]
    fn the_filter_reads_only_the_lookback() {
        let work = to_line(&entry(
            T - 2 * HOUR,
            &[("work", p(Some(1), 50, Some(WEEK)))],
        ));
        let pad_len = padding(T).len() + 1;
        let n = usize::try_from(STALE_LOOKBACK).unwrap() / pad_len + 8;

        // The recorded 50 is further back than the lookback: the passive 38
        // is compared with nothing and is written.
        let r = Root::new();
        let mut body = format!("{work}\n");
        for i in 0..n {
            body.push_str(&padding(T - 2 * HOUR + 1 + i as i64));
            body.push('\n');
        }
        r.seed(&body);
        r.append(&entry(T, &[("work", pp(38, Some(WEEK)))]), T);
        assert_eq!(r.last(T).profiles["work"].w, 38);

        // Within the lookback it is found and the 38 is replaced by it.
        let r = Root::new();
        let mut body = String::new();
        for i in 0..8 {
            body.push_str(&padding(T - 3 * HOUR + i));
            body.push('\n');
        }
        body.push_str(&format!("{work}\n"));
        r.seed(&body);
        r.append(&entry(T, &[("work", pp(38, Some(WEEK)))]), T);
        let e = r.last(T);
        assert_eq!(e.profiles["work"].w, 50);
        assert!(e.unknown.is_empty());
    }

    #[test]
    fn an_unreadable_recorded_line_is_not_compared() {
        // A recorded figure out of range is dropped by the parser and gives
        // no reading to compare with.
        let r = Root::new();
        r.seed(&format!(
            "{}\n{{\"at\":\"2026-10-30T15:15:00Z\",\"profiles\":{{\"work\":{{\"w\":500}}}}}}\n",
            to_line(&entry(T - 2 * HOUR, &[("other", p(Some(1), 1, None))]))
        ));
        r.append(&entry(T, &[("work", pp(38, Some(WEEK)))]), T);
        assert_eq!(r.last(T).profiles["work"].w, 38);
    }

    #[test]
    fn a_passive_reading_from_an_ended_or_earlier_week_takes_the_recorded_figure() {
        // Rule 1: the passive reading's week ended at T - 60, but the
        // recorded 50 is of the week now running, so it is written.
        let r = Root::new();
        r.append(
            &entry(T - HOUR, &[("work", p(Some(1), 50, Some(WEEK)))]),
            T - HOUR,
        );
        r.append(&entry(T, &[("work", pp(90, Some(T - 60)))]), T);
        assert_eq!(r.last(T).profiles["work"], p(Some(1), 50, Some(WEEK)));

        // Rule 3: a figure from the week before, however high, is replaced.
        let r = Root::new();
        r.append(
            &entry(T - HOUR, &[("work", p(Some(1), 5, Some(WEEK)))]),
            T - HOUR,
        );
        r.append(&entry(T, &[("work", pp(90, Some(WEEK - 7 * DAY)))]), T);
        assert_eq!(r.last(T).profiles["work"].w, 5);

        // Rule 1 with a recorded reading whose week has ended too: nothing
        // known is left, so the login is unknown.
        let r = Root::new();
        r.append(
            &entry(T - HOUR, &[("work", p(Some(1), 50, Some(T - 120)))]),
            T - HOUR,
        );
        r.append(&entry(T, &[("work", pp(90, Some(T - 60)))]), T);
        let e = r.last(T);
        assert!(e.profiles.is_empty());
        assert_eq!(e.unknown, ["work"]);
    }

    #[test]
    fn rotate_status_keeps_a_login_whose_passive_reading_was_stale() {
        use super::super::capacity_summary::status_line;
        use super::super::capacity_weight::Weights;
        // `profile usage` read both logins 5 h and 1 h ago. Then a rotator
        // cycle got a cached 38 for `work`, which is in use and at 50.
        let r = Root::new();
        let both = |w| {
            [
                ("work", p(Some(1), w, Some(WEEK))),
                ("home", p(Some(1), 20, Some(WEEK))),
            ]
        };
        r.append(
            &by(Writer::Usage, T - 5 * HOUR, &both(40), &[]),
            T - 5 * HOUR,
        );
        r.append(&by(Writer::Usage, T - HOUR, &both(50), &[]), T - HOUR);
        r.append(
            &by(
                Writer::Rotate,
                T,
                &[("work", pp(38, Some(WEEK))), ("home", pp(20, Some(WEEK)))],
                &[],
            ),
            T,
        );
        let (all, warning) = read_all(&r.root, T + 60);
        assert_eq!(warning, None);
        let (newest, earlier) = all.split_last().unwrap();
        assert_eq!(newest.by, Some(Writer::Rotate));
        let line = status_line(newest, earlier, &Weights::default(), 95, T + 60);

        // The cycle read every login, so the line says none was left out
        // and none is unknown. Headroom counts `work` at its recorded 50,
        // not the cached 38: (50 + 80) points of 2 logins. Its runway is
        // its own: 10 points used over 5 h, 0.02 login-weeks an hour, and
        // 1.30 login-weeks of headroom last about 65 h.
        assert_eq!(
            line,
            "capacity: headroom 1.30 login-weeks of 2 (65%), usable 2 of 2 logins, \
             runway ~65 h (as of 2026-10-30T16:15:00Z, 1m ago, rotate cycle: probed profiles only)"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #2218 (D7): history of an excluded profile is kept as written
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod excluded {
    use super::super::auto_switch::config::{self, ReadMode};
    use super::super::usage_history::{FILE, read_all, read_recent, to_line};
    use super::{Entry, HOUR, T, entry, name, p};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct Root {
        _t: tempfile::TempDir,
        root: PathBuf,
    }

    impl Root {
        /// A synthetic profile root with a private `.auto-switch.d` and a
        /// history holding `lines`.
        fn with(lines: &[Entry]) -> Self {
            let t = tempfile::tempdir().unwrap();
            let root = t.path().to_path_buf();
            let state = root.join(".auto-switch.d");
            fs::create_dir(&state).unwrap();
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
            let body: String = lines.iter().map(|e| format!("{}\n", to_line(e))).collect();
            fs::write(state.join(FILE), body).unwrap();
            fs::set_permissions(state.join(FILE), fs::Permissions::from_mode(0o600)).unwrap();
            Self { _t: t, root }
        }

        fn exclude(&self, names: &[&str]) {
            config::update(&self.root, ReadMode::Editable, |c| {
                c.exclude.extend(names.iter().map(|s| name(s)));
                Ok(())
            })
            .unwrap();
        }

        fn bytes(&self) -> Vec<u8> {
            fs::read(self.root.join(".auto-switch.d").join(FILE)).unwrap()
        }
    }

    /// A line from before the fix: `spare` has a reading and `extra` is
    /// unknown, alongside `work`.
    fn old_line(at: i64) -> Entry {
        let mut e = entry(
            at,
            &[
                ("work", p(Some(10), 40, None)),
                ("spare", p(Some(0), 0, None)),
            ],
        );
        e.unknown = vec!["extra".to_string()];
        e
    }

    #[test]
    fn read_all_returns_old_entries_for_an_excluded_profile_unchanged() {
        // D7: history is neither removed nor filtered for an excluded
        // profile; the readers that build figures leave it out themselves.
        let r = Root::with(&[old_line(T - 2 * HOUR), old_line(T - HOUR)]);
        r.exclude(&["spare", "extra"]);
        let before = r.bytes();
        assert_eq!(
            read_all(&r.root, T),
            (vec![old_line(T - 2 * HOUR), old_line(T - HOUR)], None)
        );
        let (recent, _) = read_recent(&r.root, T, 6 * HOUR);
        assert_eq!(recent, vec![old_line(T - 2 * HOUR), old_line(T - HOUR)]);
        assert_eq!(r.bytes(), before, "the file keeps the old lines as written");
    }
}

// ---------------------------------------------------------------------------
// Issue #2218 (D5): the refusal text for a named excluded profile
// ---------------------------------------------------------------------------

mod excluded_refusal {
    use super::super::usage_cmd::excluded_refusal;
    use super::name;

    #[test]
    fn the_refusal_names_the_profile_and_the_command_that_set_the_exclusion() {
        assert_eq!(
            excluded_refusal(&name("shared")),
            "shared: excluded (in auto-switch exclude list); not probed \
             (amplihack profile auto-switch exclude)"
        );
    }

    #[test]
    fn the_refusal_holds_no_path_or_account() {
        let line = excluded_refusal(&name("shared"));
        for word in ["/", "@", ".credentials", "uuid"] {
            assert!(!line.contains(word), "{word}: {line}");
        }
    }
}
