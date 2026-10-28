//! Readings recorded by `profile usage` (issue #1578): the set-up gate, the
//! lock, the swap-in-progress skip, account re-checks under the lock, the
//! status-line bounds, carrying over a still-valid window, and the lease view
//! read for marks.

use super::state::{HostEntry, Journal, Ledger, Op, State};
use super::usage::{self, Reading, ReadingSource, Window};
use super::{LeaseView, Observation, ObservedWindow, RecordOutcome, lease_view, record_usage};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn now() -> i64 {
    super::fsx::now()
}

struct Root {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    state: State,
}

/// A private root; with `set_up`, an empty `.auto-switch.d` so the D1 gate
/// passes whatever `AMPLIHACK_CLAUDE_AUTO_SWITCH` says.
fn root(set_up: bool) -> Root {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profiles");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let root = root.canonicalize().unwrap();
    let state = State::new(&root).unwrap();
    if set_up {
        state.ensure().unwrap();
    }
    Root {
        _tmp: tmp,
        root,
        state,
    }
}

fn ow(pct: f64, resets_at: Option<i64>) -> Option<ObservedWindow> {
    Some(ObservedWindow {
        used_percentage: pct,
        resets_at,
    })
}

fn obs(
    profile: &str,
    account: &str,
    session: Option<ObservedWindow>,
    week: Option<ObservedWindow>,
) -> Observation {
    Observation {
        profile: profile.into(),
        account: account.into(),
        session,
        week,
    }
}

fn hosts(state: &State, pairs: &[(&str, &str)]) {
    let ledger = Ledger {
        version: 1,
        hosts: pairs
            .iter()
            .map(|(dir, live)| {
                (
                    dir.to_string(),
                    HostEntry {
                        live: live.to_string(),
                        swapped_at: now() - 10,
                        pre_swap_fingerprint: None,
                    },
                )
            })
            .collect(),
    };
    state.save_ledger(&ledger).unwrap();
}

fn recorded(written: &[&str], moved: &[&str]) -> RecordOutcome {
    RecordOutcome::Recorded {
        written: written.iter().map(|s| s.to_string()).collect(),
        moved: moved.iter().map(|s| s.to_string()).collect(),
    }
}

fn usage_file(r: &Root) -> PathBuf {
    r.state.dir.join("usage.json")
}

// ---------------------------------------------------------------------------
// Gate, lock and journal (D1, D2)
// ---------------------------------------------------------------------------

#[test]
fn nothing_is_created_when_auto_switch_is_not_set_up() {
    // Other tests set the toggle under this lock; read it under it too.
    let _env = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if std::env::var_os("AMPLIHACK_CLAUDE_AUTO_SWITCH").is_some_and(|v| !v.is_empty()) {
        return; // the environment decides the gate in this process
    }
    let r = root(false);
    let out = record_usage(&r.root, &[obs("a", "a", ow(5.0, Some(now() + HOUR)), None)]).unwrap();
    assert_eq!(out, RecordOutcome::NotSetUp);
    assert!(!r.root.join(".auto-switch.d").exists(), "no state created");
}

#[test]
fn a_held_lock_means_busy_and_nothing_is_written() {
    let r = root(true);
    let _held = r.state.lock().unwrap();
    let start = std::time::Instant::now();
    let out = record_usage(&r.root, &[obs("a", "a", ow(5.0, Some(now() + HOUR)), None)]).unwrap();
    assert_eq!(out, RecordOutcome::Busy);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "bounded wait, not a blocking lock"
    );
    assert!(!usage_file(&r).exists());
}

#[test]
fn a_swap_journal_skips_recording_and_is_left_alone() {
    let r = root(true);
    let journal = Journal {
        version: 1,
        op: Op::Swap,
        dir: "a".into(),
        from: "a".into(),
        from_present: true,
        to: "b".into(),
        phase: "moved".into(),
        started_at: now(),
        pid: std::process::id(),
        fingerprint: None,
        refresh_lock_ino: None,
    };
    r.state.save_journal(&journal).unwrap();
    let out = record_usage(&r.root, &[obs("a", "a", ow(5.0, Some(now() + HOUR)), None)]).unwrap();
    assert_eq!(out, RecordOutcome::SwapInProgress);
    assert!(!usage_file(&r).exists());
    assert_eq!(
        r.state.load_journal().unwrap(),
        Some(journal),
        "no recovery run"
    );
}

#[test]
fn an_empty_list_writes_nothing() {
    let r = root(true);
    assert_eq!(record_usage(&r.root, &[]).unwrap(), recorded(&[], &[]));
    assert!(!usage_file(&r).exists());
}

// ---------------------------------------------------------------------------
// What is written
// ---------------------------------------------------------------------------

#[test]
fn records_session_and_week_as_a_statusline_reading() {
    let r = root(true);
    let t = now();
    let out = record_usage(
        &r.root,
        &[obs(
            "a",
            "a",
            ow(20.0, Some(t + HOUR)),
            ow(30.0, Some(t + DAY)),
        )],
    )
    .unwrap();
    assert_eq!(out, recorded(&["a"], &[]));
    let file = r.state.try_load_usage().unwrap();
    let reading = &file.accounts["a"];
    assert_eq!(
        reading.five_hour,
        Some(Window {
            used_percentage: 20.0,
            resets_at: t + HOUR
        })
    );
    assert_eq!(
        reading.seven_day,
        Some(Window {
            used_percentage: 30.0,
            resets_at: t + DAY
        })
    );
    assert_eq!(reading.source, ReadingSource::Statusline);
    assert!((t..=now()).contains(&reading.observed_at));
    let mode = fs::metadata(usage_file(&r)).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_hosted_reading_goes_to_the_live_account() {
    let r = root(true);
    hosts(&r.state, &[("work", "personal")]);
    let out = record_usage(
        &r.root,
        &[obs("work", "personal", ow(20.0, Some(now() + HOUR)), None)],
    )
    .unwrap();
    assert_eq!(out, recorded(&["personal"], &[]));
    let file = r.state.try_load_usage().unwrap();
    assert!(file.accounts.contains_key("personal"));
    assert!(!file.accounts.contains_key("work"));
}

#[test]
fn an_account_change_during_the_probe_skips_that_row_only() {
    let r = root(true);
    // The probe saw `work` holding its own login; a swap has since happened.
    hosts(&r.state, &[("work", "personal")]);
    let out = record_usage(
        &r.root,
        &[
            obs("work", "work", ow(20.0, Some(now() + HOUR)), None),
            obs("c", "c", ow(5.0, Some(now() + HOUR)), None),
        ],
    )
    .unwrap();
    assert_eq!(out, recorded(&["c"], &["work"]));
    let file = r.state.try_load_usage().unwrap();
    assert!(!file.accounts.contains_key("work"));
    assert!(!file.accounts.contains_key("personal"));
    assert!(file.accounts.contains_key("c"));
}

#[test]
fn windows_failing_the_statusline_bounds_are_not_recorded() {
    let r = root(true);
    let t = now();
    let out = record_usage(
        &r.root,
        &[
            obs("late", "late", ow(5.0, Some(t + 8 * DAY + HOUR)), None),
            obs("old", "old", ow(5.0, Some(t - DAY - HOUR)), None),
            obs("unresolved", "unresolved", ow(5.0, None), ow(6.0, None)),
            obs(
                "nan",
                "nan",
                ow(f64::NAN, Some(t + HOUR)),
                ow(f64::INFINITY, Some(t + DAY)),
            ),
        ],
    )
    .unwrap();
    assert_eq!(out, recorded(&[], &[]));
    let file = r.state.load_usage();
    assert!(file.accounts.is_empty(), "{:?}", file.accounts);
}

#[test]
fn percentages_are_clamped_and_one_good_window_is_enough() {
    let r = root(true);
    let t = now();
    record_usage(
        &r.root,
        &[
            obs("hi", "hi", ow(150.0, Some(t + HOUR)), ow(5.0, None)),
            obs("lo", "lo", None, ow(-3.0, Some(t + DAY))),
        ],
    )
    .unwrap();
    let file = r.state.try_load_usage().unwrap();
    assert_eq!(
        file.accounts["hi"].five_hour.unwrap().used_percentage,
        100.0
    );
    assert_eq!(file.accounts["hi"].seven_day, None);
    assert_eq!(file.accounts["lo"].seven_day.unwrap().used_percentage, 0.0);
}

#[test]
fn a_missing_window_keeps_the_stored_one_while_it_has_not_reset() {
    let r = root(true);
    let t = now();
    let mut accounts = BTreeMap::new();
    accounts.insert(
        "a".to_string(),
        Reading {
            five_hour: Some(Window {
                used_percentage: 10.0,
                resets_at: t + HOUR,
            }),
            seven_day: Some(Window {
                used_percentage: 40.0,
                resets_at: t + 2 * DAY,
            }),
            observed_at: t - 100,
            source: ReadingSource::Statusline,
        },
    );
    accounts.insert(
        "b".to_string(),
        Reading {
            five_hour: None,
            seven_day: Some(Window {
                used_percentage: 90.0,
                resets_at: t - 10,
            }),
            observed_at: t - 100,
            source: ReadingSource::Statusline,
        },
    );
    r.state
        .save_usage(&super::state::UsageFile {
            version: 1,
            accounts,
        })
        .unwrap();

    record_usage(
        &r.root,
        &[
            obs("a", "a", ow(50.0, Some(t + 2 * HOUR)), None),
            obs("b", "b", ow(7.0, Some(t + HOUR)), None),
        ],
    )
    .unwrap();
    let file = r.state.try_load_usage().unwrap();
    let a = &file.accounts["a"];
    assert_eq!(
        a.five_hour.unwrap().used_percentage,
        50.0,
        "new window wins"
    );
    assert_eq!(
        a.seven_day,
        Some(Window {
            used_percentage: 40.0,
            resets_at: t + 2 * DAY
        }),
        "still-valid stored window carried over"
    );
    assert_eq!(
        file.accounts["b"].seven_day, None,
        "a reset window is not carried over"
    );
}

#[test]
fn other_accounts_in_usage_json_are_untouched() {
    let r = root(true);
    let t = now();
    let other = Reading {
        five_hour: Some(Window {
            used_percentage: 61.0,
            resets_at: t + HOUR,
        }),
        seven_day: None,
        observed_at: t - 5,
        source: ReadingSource::UsageLimitFailure,
    };
    let mut accounts = BTreeMap::new();
    accounts.insert("z".to_string(), other.clone());
    r.state
        .save_usage(&super::state::UsageFile {
            version: 1,
            accounts,
        })
        .unwrap();
    record_usage(&r.root, &[obs("a", "a", ow(1.0, Some(t + HOUR)), None)]).unwrap();
    assert_eq!(r.state.try_load_usage().unwrap().accounts["z"], other);
}

// ---------------------------------------------------------------------------
// Bounds owner (D7): one rule for status-line input and `profile usage`.
// ---------------------------------------------------------------------------

#[test]
fn plausible_matches_the_statusline_window_rule() {
    let t = 1_760_000_000;
    let w = |pct, at| {
        Some(Window {
            used_percentage: pct,
            resets_at: at,
        })
    };
    assert_eq!(usage::plausible(42.0, t + HOUR, t), w(42.0, t + HOUR));
    assert_eq!(usage::plausible(42.0, t - DAY, t), w(42.0, t - DAY));
    assert_eq!(usage::plausible(42.0, t + 8 * DAY, t), w(42.0, t + 8 * DAY));
    assert_eq!(usage::plausible(42.0, t - DAY - 1, t), None);
    assert_eq!(usage::plausible(42.0, t + 8 * DAY + 1, t), None);
    assert_eq!(usage::plausible(150.0, t, t), w(100.0, t));
    assert_eq!(usage::plausible(-1.0, t, t), w(0.0, t));
    assert_eq!(usage::plausible(f64::NAN, t, t), None);
    assert_eq!(usage::plausible(f64::NEG_INFINITY, t, t), None);
    assert_eq!(usage::plausible(1.0, i64::MAX, t), None);
    assert_eq!(usage::plausible(1.0, i64::MIN, t), None);
    assert_eq!(usage::plausible(1.0, 0, i64::MAX), None);
}

// ---------------------------------------------------------------------------
// Lease view (marks and attribution before probing)
// ---------------------------------------------------------------------------

#[test]
fn no_state_directory_means_no_lease_view() {
    let r = root(false);
    assert_eq!(lease_view(&r.root).unwrap(), None);
    assert!(
        !r.root.join(".auto-switch.d").exists(),
        "reading creates nothing"
    );
}

#[test]
fn lease_view_reflects_the_ledger_without_taking_the_lock() {
    let r = root(true);
    hosts(&r.state, &[("work", "personal")]);
    let _held = r.state.lock().unwrap();
    let view: LeaseView = lease_view(&r.root).unwrap().expect("state exists");
    assert_eq!(view.account_in("work"), "personal");
    assert_eq!(view.leased_to("personal"), Some("work"));
    assert_eq!(view.account_in("c"), "c");
}

#[test]
fn an_untrusted_or_damaged_ledger_is_an_error() {
    let r = root(true);
    fs::write(r.state.dir.join("ledger.json"), "{not json").unwrap();
    fs::set_permissions(
        r.state.dir.join("ledger.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(lease_view(&r.root).is_err());

    let r = root(true);
    hosts(&r.state, &[("work", "personal")]);
    fs::set_permissions(&r.state.dir, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        lease_view(&r.root).is_err(),
        "a world-writable state directory is not trusted"
    );
    fs::set_permissions(&r.state.dir, fs::Permissions::from_mode(0o700)).unwrap();
}
