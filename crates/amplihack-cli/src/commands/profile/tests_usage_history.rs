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
    Entry, HistoryError, Point, Snapshot, entry_of, from_rows, parse_line, to_line,
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
        live_in_home: false,
        api_key: false,
        api_key_blocked: None,
        report: rep,
        error: None,
        raw: None,
    }
}

fn p(s: Option<u8>, w: u8, w_reset: Option<i64>) -> Point {
    Point {
        s,
        w,
        w_reset,
        s_reset: None,
    }
}

fn entry(at: i64, points: &[(&str, Point)]) -> Entry {
    Entry {
        at,
        profiles: points
            .iter()
            .map(|(n, p)| (n.to_string(), p.clone()))
            .collect::<BTreeMap<_, _>>(),
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
    let mut home = row("~/.claude", report(UsageState::Ok, Some(1.0), Some(1.0)));
    home.live_in_home = true;
    let mut over = row("over", report(UsageState::Ok, Some(5.0), Some(50.0)));
    over.overflow = true;
    let rows = vec![
        row("work", report(UsageState::Ok, Some(20.0), Some(30.0))),
        row("slow", UsageReport::empty(UsageState::Timeout)),
        row("blind", report(UsageState::Ok, Some(20.0), None)),
        key,
        home,
        over,
    ];
    let s: Snapshot = from_rows(T, &rows, None).unwrap();
    assert_eq!(s.at, T);
    assert_eq!(
        s.points.keys().map(String::as_str).collect::<Vec<_>>(),
        ["over", "work"],
        "overflow subscription rows count; API-key and ~/.claude rows do not"
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
    let s = from_rows(T, &rows, Some(&name("work"))).unwrap();
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
    let e = from_rows(T, &rows, None).unwrap().entry();
    assert_eq!(e.at, T);
    assert_eq!(e.profiles.keys().collect::<Vec<_>>(), ["work"]);
}

#[test]
fn from_rows_without_a_subscription_login_is_none() {
    let mut key = row("key", report(UsageState::Ok, Some(0.0), Some(0.0)));
    key.api_key = true;
    let mut home = row("~/.claude", report(UsageState::Ok, Some(1.0), Some(1.0)));
    home.live_in_home = true;
    assert!(from_rows(T, &[key, home], None).is_none());
    assert!(from_rows(T, &[], None).is_none());
    let slow = row("slow", UsageReport::empty(UsageState::Timeout));
    assert_eq!(
        from_rows(T, &[slow], None).map(|s| s.unknown),
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
