//! The no-capacity notice, writer side: `compute`, the write rules, the
//! utils-reader round trip, and the parity tables that keep amplihack-utils'
//! root and gate in step with this crate's. Also the launch and recipe-run
//! warning lines.
//!
//! Every root is a temporary directory (0700); `HOME` is temporary wherever it
//! matters. Tests that touch the environment hold `env_lock()`.

use super::candidate::{OverflowPool, Pool, select_with_overflow};
use super::capacity::{
    Evidence, Rotated, launch_warning, recipe_warning, refresh, refresh_from_rotation,
};
use super::config::{self, Config};
use super::state::{HostEntry, Ledger, State, UsageFile};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{Gate, OverflowEntry};
use crate::test_support::{EnvGuard, env_lock};
use amplihack_utils::claude_capacity::{self as cap, CapacityFile, State as Cap};
use chrono::DateTime;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
const T: i64 = 2_000_000_000;

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock().lock().unwrap_or_else(|p| p.into_inner())
}

fn ts(epoch: i64) -> DateTime<chrono::Utc> {
    DateTime::from_timestamp(epoch, 0).unwrap()
}

fn now() -> i64 {
    super::fsx::now()
}

fn names(list: &[&str]) -> Vec<ProfileName> {
    list.iter()
        .map(|n| ProfileName::parse(n).unwrap())
        .collect()
}

fn cfg(threshold: u8, exclude: &[&str]) -> Config {
    Config {
        enabled: true,
        threshold,
        exclude: names(exclude),
        overflow: Vec::new(),
        kind: Default::default(),
    }
}

fn win(pct: f64, resets_at: i64) -> Option<Window> {
    Some(Window {
        used_percentage: pct,
        resets_at,
    })
}

fn reading(five: Option<Window>, week: Option<Window>, at: i64) -> Reading {
    Reading {
        five_hour: five,
        seven_day: week,
        observed_at: at,
        source: ReadingSource::Statusline,
    }
}

fn usage(pct: f64, reset: i64) -> Evidence {
    Evidence::Usage(reading(win(pct, reset), None, T))
}

fn ev(pairs: Vec<(&str, Evidence)>) -> BTreeMap<String, Evidence> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

// ---------------------------------------------------------------------------
// compute (pure)
// ---------------------------------------------------------------------------

/// `compute` with no overflow inputs.
fn compute(
    profiles: &[ProfileName],
    evidence: &BTreeMap<String, Evidence>,
    cfg: &Config,
    now: i64,
    writer: &str,
) -> CapacityFile {
    let none = OverflowPool {
        order: &[],
        blocked: &[],
        gate: None,
        platform: None,
    };
    super::capacity::compute(profiles, evidence, cfg, &none, now, writer)
}

const GATE: Gate = Gate {
    wait: 1800,
    unknown_reset: false,
    session_at: 85,
    week_at: 90,
};

#[test]
fn all_over_threshold_is_low() {
    let f = compute(
        &names(&["b", "a"]),
        &ev(vec![
            ("a", usage(95.0, T + 2 * HOUR)),
            ("b", usage(91.0, T + HOUR)),
        ]),
        &cfg(90, &[]),
        T,
        "statusline",
    );
    assert_eq!(
        f,
        CapacityFile {
            version: 1,
            state: Cap::Low,
            threshold: 90,
            over: vec!["a".into(), "b".into()],
            earliest_reset: Some(ts(T + HOUR)),
            written_at: ts(T),
            writer: "statusline".into(),
        }
    );
}

#[test]
fn exactly_at_the_threshold_counts_as_over() {
    let f = compute(
        &names(&["a"]),
        &ev(vec![("a", usage(90.0, T + HOUR))]),
        &cfg(90, &[]),
        T,
        "usage",
    );
    assert_eq!(f.state, Cap::Low);
}

#[test]
fn all_at_the_hard_limit_is_exhausted() {
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![
            ("a", usage(100.0, T + 3 * HOUR)),
            (
                "b",
                Evidence::Usage(reading(win(10.0, T + HOUR), win(100.0, T + 2 * DAY), T)),
            ),
        ]),
        &cfg(90, &[]),
        T,
        "statusline",
    );
    assert_eq!(f.state, Cap::Exhausted);
    assert_eq!(f.over, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(f.earliest_reset, Some(ts(T + 3 * HOUR)));
}

#[test]
fn one_at_limit_and_one_merely_over_is_low() {
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![
            ("a", usage(100.0, T + HOUR)),
            ("b", usage(95.0, T + HOUR)),
        ]),
        &cfg(90, &[]),
        T,
        "statusline",
    );
    assert_eq!(f.state, Cap::Low);
}

#[test]
fn mixed_is_ok() {
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![
            ("a", usage(95.0, T + HOUR)),
            ("b", usage(40.0, T + HOUR)),
        ]),
        &cfg(90, &[]),
        T,
        "statusline",
    );
    assert_eq!(f.state, Cap::Ok);
}

#[test]
fn an_excluded_profile_is_ignored() {
    let evidence = ev(vec![
        ("a", usage(95.0, T + HOUR)),
        ("b", usage(10.0, T + HOUR)),
    ]);
    let f = compute(&names(&["a", "b"]), &evidence, &cfg(90, &["b"]), T, "x");
    assert_eq!(f.state, Cap::Low, "b has room but is excluded");
    assert_eq!(f.over, vec!["a".to_string()]);
}

// --- Overflow (issue #1606) -------------------------------------------------
//
// The plan profiles decide the state. When every one is over the threshold,
// an overflow profile makes it `ok` exactly when auto-switch could move onto
// it: the shared gate allows it (earliest plan reset beyond the wait), and
// the profile is not blocked and not excluded. Its own plan reading does
// not count (issue #1694): a credit account at its plan limit is usable.

/// a and b (plan) over the threshold, the earliest resetting at `reset`;
/// cred (overflow) at 5%.
fn overflow_case(reset: i64, blocked: &[String], gate: Option<Gate>) -> CapacityFile {
    overflow_case_cred(reset, blocked, gate, 5.0)
}

/// [`overflow_case`] with cred's own plan use at `cred_pct`.
fn overflow_case_cred(
    reset: i64,
    blocked: &[String],
    gate: Option<Gate>,
    cred_pct: f64,
) -> CapacityFile {
    let mut c = cfg(90, &[]);
    c.overflow = vec![OverflowEntry::new(ProfileName::parse("cred").unwrap(), 1)];
    let order = names(&["cred"]);
    let pool = OverflowPool {
        order: &order,
        blocked,
        gate,
        platform: None,
    };
    super::capacity::compute(
        &names(&["a", "b", "cred"]),
        &ev(vec![
            ("a", usage(100.0, reset + HOUR)),
            ("b", usage(100.0, reset)),
            ("cred", usage(cred_pct, T + HOUR)),
        ]),
        &c,
        &pool,
        T,
        "usage",
    )
}

#[test]
fn a_credit_profile_at_its_plan_limit_still_makes_the_file_ok() {
    let f = overflow_case_cred(T + 2 * HOUR, &[], Some(GATE), 100.0);
    assert_eq!(f.state, Cap::Ok, "auto-switch moves onto cred at 100%");
    assert_eq!(f.earliest_reset, None);
    assert!(!f.over.contains(&"cred".to_string()), "cred is not plan");
    let f = overflow_case_cred(T + 2 * HOUR, &["cred".to_string()], Some(GATE), 100.0);
    assert_eq!(
        f.state,
        Cap::Exhausted,
        "only a spend block makes it unusable"
    );
}

#[test]
fn a_usable_overflow_profile_makes_the_file_ok_once_the_plan_reset_is_far() {
    let f = overflow_case(T + 2 * HOUR, &[], Some(GATE));
    assert_eq!(f.state, Cap::Ok, "auto-switch could move onto cred");
    assert_eq!(f.earliest_reset, None, "no reset to wait for");
    assert_eq!(
        f.over,
        vec!["a".to_string(), "b".to_string()],
        "the plan profiles still over"
    );
}

#[test]
fn an_overflow_profile_does_not_count_while_the_plan_reset_is_inside_the_wait() {
    let f = overflow_case(T + 1800, &[], Some(GATE));
    assert_eq!(
        f.state,
        Cap::Exhausted,
        "a reset exactly at the wait keeps the plan"
    );
    assert_eq!(f.over, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(f.earliest_reset, Some(ts(T + 1800)));
}

#[test]
fn a_blocked_overflow_profile_or_a_closed_gate_is_no_capacity() {
    let f = overflow_case(T + 2 * HOUR, &["cred".to_string()], Some(GATE));
    assert_eq!(f.state, Cap::Exhausted, "spend-blocked");
    let f = overflow_case(T + 2 * HOUR, &[], None);
    assert_eq!(f.state, Cap::Exhausted, "gate unreadable");
}

/// The file and the selection on the same readings (issue #1606, review
/// round 3): `ok` through overflow exactly when a session on `a`, at the
/// threshold, would be moved onto `cred`. Plan profiles `a` and `b` read
/// `a` and `b`; `cred` reads `cred_pct` on its five-hour window.
fn file_and_selection(threshold: u8, a: Reading, b: Reading, cred_pct: f64) -> (Cap, bool) {
    let mut c = cfg(threshold, &[]);
    c.overflow = vec![OverflowEntry::new(ProfileName::parse("cred").unwrap(), 1)];
    let order = names(&["cred"]);
    let pool = OverflowPool {
        order: &order,
        blocked: &[],
        gate: Some(GATE),
        platform: None,
    };
    let profiles = names(&["a", "b", "cred"]);
    let cred = reading(win(cred_pct, T + HOUR), None, T);
    let file = super::capacity::compute(
        &profiles,
        &ev(vec![
            ("a", Evidence::Usage(a.clone())),
            ("b", Evidence::Usage(b.clone())),
            ("cred", Evidence::Usage(cred.clone())),
        ]),
        &c,
        &pool,
        T,
        "usage",
    );
    let readings: BTreeMap<String, Reading> = [("a", a), ("b", b), ("cred", cred)]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let p = Pool {
        profiles: &profiles,
        readings: &readings,
        threshold,
        exclude: &[],
        now: T,
    };
    let picked = select_with_overflow(&p, &pool, "a", |_| true);
    (file.state, picked.is_some_and(|p| p.as_str() == "cred"))
}

#[test]
fn the_overflow_state_uses_the_gate_s_reset_not_the_threshold_s() {
    // b: session 100% resetting in two hours, week 95% resetting in ten
    // minutes. b regains capacity by the gate's test only when both have
    // reset, in two hours: beyond the 30-minute wait, so auto-switch moves
    // onto cred. The file must say so, not take the week's reset alone.
    let a = reading(win(100.0, T + 3 * HOUR), win(10.0, T + 100 * HOUR), T);
    let b = reading(win(100.0, T + 2 * HOUR), win(95.0, T + 600), T);
    let (state, spends) = file_and_selection(90, a, b, 5.0);
    assert!(spends, "the selection moves onto cred");
    assert_eq!(state, Cap::Ok, "so the file is ok");
}

#[test]
fn a_plan_profile_with_gate_capacity_over_the_threshold_keeps_the_file_low() {
    // Threshold 80, below the gate's 85/90. b at 0% session and 85% week is
    // over 80 but has capacity by the gate's test: the rotator would not
    // spend, auto-switch does not move onto cred, and the file stays low.
    let a = reading(win(95.0, T + 3 * HOUR), win(10.0, T + 100 * HOUR), T);
    let b = reading(win(0.0, T + 2 * HOUR), win(85.0, T + 100 * HOUR), T);
    let (state, spends) = file_and_selection(80, a, b, 5.0);
    assert!(!spends, "the selection keeps the plan");
    assert_eq!(state, Cap::Low);
}

#[test]
fn the_file_and_the_selection_agree_on_overflow() {
    for (threshold, cred_pct) in [70u8, 80, 90]
        .into_iter()
        .flat_map(|t| [(t, 5.0), (t, 100.0)])
    {
        for (bs, bw, bs_reset, bw_reset) in [
            (100.0, 95.0, 2 * HOUR, 600),
            (100.0, 95.0, 600, 2 * HOUR),
            (87.0, 95.0, 2 * HOUR, 600),
            (95.0, 10.0, 600, 100 * HOUR),
            (95.0, 10.0, 2 * HOUR, 100 * HOUR),
            (0.0, 85.0, HOUR, 100 * HOUR),
            (0.0, 92.0, HOUR, 100 * HOUR),
            (82.0, 10.0, HOUR, 100 * HOUR),
        ] {
            let a = reading(win(100.0, T + 3 * HOUR), win(10.0, T + 100 * HOUR), T);
            let b = reading(win(bs, T + bs_reset), win(bw, T + bw_reset), T);
            let (state, spends) = file_and_selection(threshold, a, b, cred_pct);
            if b_over(threshold, bs, bw) {
                assert_eq!(
                    state == Cap::Ok,
                    spends,
                    "threshold {threshold}, cred {cred_pct}%, b {bs}/{bw} resets {bs_reset}/{bw_reset}"
                );
            }
        }
    }
}

/// b over the auto-switch threshold: only then does overflow decide the
/// file's state (otherwise b keeps it `ok`).
fn b_over(threshold: u8, session: f64, week: f64) -> bool {
    session.max(week) >= f64::from(threshold)
}

#[test]
fn an_overflow_profile_is_never_plan_capacity() {
    // Only cred is under the threshold, but it is overflow: with the gate
    // shut, the plan state stands and cred is not listed as over.
    let f = overflow_case(T + 600, &[], Some(GATE));
    assert_eq!(f.state, Cap::Exhausted);
    assert!(!f.over.contains(&"cred".to_string()));
}

#[test]
fn a_profile_with_no_reading_is_not_over() {
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![("a", usage(99.0, T + HOUR))]),
        &cfg(90, &[]),
        T,
        "x",
    );
    assert_eq!(f.state, Cap::Ok, "missing data never raises an alarm");
}

#[test]
fn zero_eligible_profiles_is_ok() {
    let f = compute(&[], &BTreeMap::new(), &cfg(90, &[]), T, "x");
    assert_eq!(f.state, Cap::Ok);
    let f = compute(
        &names(&["a"]),
        &ev(vec![("a", usage(100.0, T + HOUR))]),
        &cfg(90, &["a"]),
        T,
        "x",
    );
    assert_eq!(f.state, Cap::Ok, "everything excluded");
}

#[test]
fn a_window_past_its_reset_counts_as_zero() {
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![
            ("a", usage(100.0, T - 1)),
            ("b", usage(100.0, T + HOUR)),
        ]),
        &cfg(90, &[]),
        T,
        "x",
    );
    assert_eq!(f.state, Cap::Ok, "a has reset");
}

#[test]
fn the_threshold_comes_from_the_config() {
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![
            ("a", usage(80.0, T + HOUR)),
            ("b", usage(76.0, T + HOUR)),
        ]),
        &cfg(75, &[]),
        T,
        "x",
    );
    assert_eq!(f.state, Cap::Low);
    assert_eq!(f.threshold, 75);
}

#[test]
fn evidence_for_profiles_not_listed_is_ignored() {
    let f = compute(
        &names(&["a"]),
        &ev(vec![
            ("a", usage(95.0, T + HOUR)),
            ("ghost", usage(10.0, T + HOUR)),
        ]),
        &cfg(90, &[]),
        T,
        "x",
    );
    assert_eq!(f.state, Cap::Low);
    assert_eq!(f.over, vec!["a".to_string()]);
}

#[test]
fn a_usage_limit_failure_counts_as_the_hard_limit() {
    let failure = |pct: f64| {
        Evidence::Usage(Reading {
            five_hour: win(pct, T + HOUR),
            seven_day: None,
            observed_at: T,
            source: ReadingSource::UsageLimitFailure,
        })
    };
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![("a", failure(100.0)), ("b", failure(97.0))]),
        &cfg(90, &[]),
        T,
        "x",
    );
    assert_eq!(f.state, Cap::Exhausted);
}

#[test]
fn a_usage_limit_failure_whose_window_reset_is_not_over() {
    let f = compute(
        &names(&["a"]),
        &ev(vec![(
            "a",
            Evidence::Usage(Reading {
                five_hour: win(100.0, T - 1),
                seven_day: None,
                observed_at: T - HOUR,
                source: ReadingSource::UsageLimitFailure,
            }),
        )]),
        &cfg(90, &[]),
        T,
        "x",
    );
    assert_eq!(f.state, Cap::Ok);
}

#[test]
fn both_windows_over_use_the_later_reset() {
    // a: session and week both over, so a has capacity again only after the
    // week resets. b: only the session is over.
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![
            (
                "a",
                Evidence::Usage(reading(win(95.0, T + HOUR), win(96.0, T + 3 * DAY), T)),
            ),
            (
                "b",
                Evidence::Usage(reading(win(99.0, T + 2 * HOUR), win(10.0, T + DAY), T)),
            ),
        ]),
        &cfg(90, &[]),
        T,
        "x",
    );
    assert_eq!(f.state, Cap::Low);
    assert_eq!(
        f.earliest_reset,
        Some(ts(T + 2 * HOUR)),
        "min over accounts of each account's reset"
    );
}

#[test]
fn only_the_window_that_is_over_sets_the_reset() {
    let f = compute(
        &names(&["a"]),
        &ev(vec![(
            "a",
            Evidence::Usage(reading(win(20.0, T + HOUR), win(92.0, T + 2 * DAY), T)),
        )]),
        &cfg(90, &[]),
        T,
        "x",
    );
    assert_eq!(f.earliest_reset, Some(ts(T + 2 * DAY)));
}

#[test]
fn rotation_exhausted_without_a_stored_reset_is_exhausted_and_unknown() {
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![
            (
                "a",
                Evidence::Rotation {
                    reading: Rotated::Exhausted,
                    stored: None,
                },
            ),
            (
                "b",
                Evidence::Rotation {
                    reading: Rotated::Exhausted,
                    stored: None,
                },
            ),
        ]),
        &cfg(90, &[]),
        T,
        "rotate",
    );
    assert_eq!(f.state, Cap::Exhausted);
    assert_eq!(f.earliest_reset, None);
    assert_eq!(f.writer, "rotate");
}

#[test]
fn rotation_measured_takes_the_reset_from_the_stored_reading() {
    let stored = reading(win(95.0, T + HOUR), win(10.0, T + 3 * DAY), T - 60);
    let f = compute(
        &names(&["a"]),
        &ev(vec![(
            "a",
            Evidence::Rotation {
                reading: Rotated::Measured {
                    session: 95.0,
                    week: 10.0,
                },
                stored: Some(stored),
            },
        )]),
        &cfg(90, &[]),
        T,
        "rotate",
    );
    assert_eq!(f.state, Cap::Low);
    assert_eq!(f.earliest_reset, Some(ts(T + HOUR)));
}

#[test]
fn rotation_with_a_stale_stored_reset_is_unknown() {
    let stored = reading(win(95.0, T - 10), None, T - DAY);
    let f = compute(
        &names(&["a"]),
        &ev(vec![(
            "a",
            Evidence::Rotation {
                reading: Rotated::Measured {
                    session: 97.0,
                    week: 0.0,
                },
                stored: Some(stored),
            },
        )]),
        &cfg(90, &[]),
        T,
        "rotate",
    );
    assert_eq!(f.state, Cap::Low, "the rotator's own reading decides");
    assert_eq!(f.earliest_reset, None, "only resets_at > now count");
}

#[test]
fn rotation_failed_is_no_reading() {
    let f = compute(
        &names(&["a", "b"]),
        &ev(vec![
            (
                "a",
                Evidence::Rotation {
                    reading: Rotated::Exhausted,
                    stored: None,
                },
            ),
            (
                "b",
                Evidence::Rotation {
                    reading: Rotated::Failed,
                    stored: None,
                },
            ),
        ]),
        &cfg(90, &[]),
        T,
        "rotate",
    );
    assert_eq!(f.state, Cap::Ok);
}

// ---------------------------------------------------------------------------
// refresh / refresh_from_rotation: the write rules
// ---------------------------------------------------------------------------

struct Root {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    state: State,
}

impl Root {
    fn capacity(&self) -> PathBuf {
        cap::path(&self.root)
    }
}

fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

/// A private root holding `profiles`, with its state directory set up.
fn root(profiles: &[&str]) -> Root {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    private_dir(&home);
    let root = tmp.path().join("profiles");
    private_dir(&root);
    let root = root.canonicalize().unwrap();
    for p in profiles {
        private_dir(&root.join(p));
    }
    let state = State::new(&root).unwrap();
    state.ensure().unwrap();
    Root {
        _tmp: tmp,
        root,
        home,
        state,
    }
}

fn usage_file(pairs: &[(&str, Reading)]) -> UsageFile {
    UsageFile {
        version: 1,
        accounts: pairs
            .iter()
            .map(|(k, r)| (k.to_string(), r.clone()))
            .collect(),
    }
}

fn over(pct: f64, reset_in: i64) -> Reading {
    let t = now();
    reading(win(pct, t + reset_in), None, t)
}

fn on() -> EnvGuard {
    EnvGuard::set([(config::TOGGLE_ENV, "1")])
}

#[test]
fn refresh_writes_a_private_file_with_profile_names_only() {
    let _l = lock();
    let _on = on();
    let r = root(&["alpha", "beta"]);
    let u = usage_file(&[("alpha", over(95.0, HOUR)), ("beta", over(92.0, 2 * HOUR))]);
    refresh(&r.state, &u, &Ledger::default(), "statusline");
    let meta = fs::metadata(r.capacity()).expect("written");
    assert_eq!(meta.mode() & 0o777, 0o600);
    assert_eq!(
        fs::metadata(r.root.join(".auto-switch.d")).unwrap().mode() & 0o777,
        0o700
    );
    let bytes = fs::read(r.capacity()).unwrap();
    let text = String::from_utf8(bytes).unwrap().to_lowercase();
    for needle in ["@", "token", "credential", "oauth", "/"] {
        assert!(!text.contains(needle), "{needle:?} in {text}");
    }
    let f = cap::read(&r.root).expect("the utils reader accepts the cli writer");
    assert_eq!(f.state, Cap::Low);
    assert_eq!(f.over, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(f.writer, "statusline");
    assert_eq!(f.threshold, 90);
    let reset = f.earliest_reset.expect("known reset").timestamp();
    assert!((reset - (now() + HOUR)).abs() <= 5, "{reset}");
}

#[test]
fn round_trip_matches_compute() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "b"]);
    let t = now();
    let u = usage_file(&[
        ("a", reading(win(100.0, t + HOUR), win(100.0, t + DAY), t)),
        ("b", reading(None, win(100.0, t + 2 * DAY), t)),
    ]);
    refresh(&r.state, &u, &Ledger::default(), "usage");
    let read = cap::read(&r.root).expect("readable");
    let evidence = u
        .accounts
        .iter()
        .map(|(k, v)| (k.clone(), Evidence::Usage(v.clone())))
        .collect();
    let want = compute(&names(&["a", "b"]), &evidence, &cfg(90, &[]), t, "usage");
    assert_eq!(read.state, want.state);
    assert_eq!(read.over, want.over);
    assert_eq!(read.earliest_reset, want.earliest_reset);
    assert_eq!(read.state, Cap::Exhausted);
    assert_eq!(read.earliest_reset, Some(ts(t + DAY)), "a: later of both");
}

#[test]
fn refresh_reads_the_account_live_in_each_profile() {
    let _l = lock();
    let _on = on();
    // Directory a holds c's login and c holds a's; c is excluded.
    let r = root(&["a", "b", "c"]);
    config::write(&r.root, &cfg(90, &["c"])).unwrap();
    let ledger = Ledger {
        version: 1,
        hosts: [("a", "c"), ("c", "a")]
            .iter()
            .map(|(dir, live)| {
                (
                    dir.to_string(),
                    HostEntry {
                        live: live.to_string(),
                        swapped_at: now() - HOUR,
                        pre_swap_fingerprint: None,
                    },
                )
            })
            .collect(),
    };
    let u = usage_file(&[
        ("a", over(20.0, HOUR)),
        ("b", over(95.0, HOUR)),
        ("c", over(96.0, HOUR)),
    ]);
    refresh(&r.state, &u, &ledger, "statusline");
    let f = cap::read(&r.root).expect("written");
    assert_eq!(f.state, Cap::Low, "a's directory runs c's (full) account");
    assert_eq!(f.over, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn refresh_does_not_write_while_auto_switch_is_off() {
    let _l = lock();
    let _off = EnvGuard::set([(config::TOGGLE_ENV, "0")]);
    let r = root(&["a"]);
    refresh(
        &r.state,
        &usage_file(&[("a", over(100.0, HOUR))]),
        &Ledger::default(),
        "statusline",
    );
    assert!(!r.capacity().exists());
    refresh_from_rotation(
        &r.root,
        &[(names(&["a"])[0].clone(), Rotated::Exhausted)],
        now(),
        false,
    );
    assert!(!r.capacity().exists());
}

#[test]
fn an_ok_state_never_creates_the_file() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "b"]);
    refresh(
        &r.state,
        &usage_file(&[("a", over(95.0, HOUR)), ("b", over(10.0, HOUR))]),
        &Ledger::default(),
        "statusline",
    );
    assert!(!r.capacity().exists());
}

#[test]
fn ok_replaces_an_existing_low_file() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "b"]);
    refresh(
        &r.state,
        &usage_file(&[("a", over(95.0, HOUR)), ("b", over(95.0, HOUR))]),
        &Ledger::default(),
        "statusline",
    );
    assert_eq!(cap::read(&r.root).map(|f| f.state), Some(Cap::Low));
    refresh(
        &r.state,
        &usage_file(&[("a", over(95.0, HOUR)), ("b", over(5.0, HOUR))]),
        &Ledger::default(),
        "statusline",
    );
    let after = cap::read(&r.root);
    assert!(
        after.as_ref().is_none_or(|f| f.state == Cap::Ok),
        "{after:?}"
    );
}

/// Rewrites the file's `written_at` to `ago` seconds in the past.
fn backdate(r: &Root, ago: i64) -> Vec<u8> {
    let mut f = cap::read(&r.root).unwrap();
    f.written_at = ts(now() - ago);
    let bytes = serde_json::to_vec_pretty(&f).unwrap();
    super::fsx::write_private_atomic(&r.root.join(".auto-switch.d"), cap::FILE, &bytes).unwrap();
    bytes
}

#[test]
fn an_unchanged_state_is_not_rewritten() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "b"]);
    let u = usage_file(&[("a", over(95.0, HOUR)), ("b", over(97.0, 2 * HOUR))]);
    refresh(&r.state, &u, &Ledger::default(), "statusline");
    let before = backdate(&r, 60);
    let mtime = fs::metadata(r.capacity()).unwrap().mtime_nsec();
    refresh(&r.state, &u, &Ledger::default(), "statusline");
    assert_eq!(fs::read(r.capacity()).unwrap(), before, "same bytes");
    assert_eq!(fs::metadata(r.capacity()).unwrap().mtime_nsec(), mtime);
}

#[test]
fn a_changed_set_of_accounts_is_rewritten() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "b", "c"]);
    let ledger = Ledger::default();
    refresh(
        &r.state,
        &usage_file(&[
            ("a", over(95.0, HOUR)),
            ("b", over(95.0, HOUR)),
            ("c", over(95.0, HOUR)),
        ]),
        &ledger,
        "statusline",
    );
    let before = backdate(&r, 60);
    refresh(
        &r.state,
        &usage_file(&[
            ("a", over(100.0, HOUR)),
            ("b", over(100.0, HOUR)),
            ("c", over(100.0, HOUR)),
        ]),
        &ledger,
        "statusline",
    );
    assert_ne!(fs::read(r.capacity()).unwrap(), before);
    assert_eq!(cap::read(&r.root).unwrap().state, Cap::Exhausted);
}

#[test]
fn a_null_reset_state_gets_a_heartbeat_rewrite_after_an_hour() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "b"]);
    let both = [
        (names(&["a"])[0].clone(), Rotated::Exhausted),
        (names(&["b"])[0].clone(), Rotated::Exhausted),
    ];
    refresh_from_rotation(&r.root, &both, now(), false);
    let f = cap::read(&r.root).expect("written");
    assert_eq!((f.state, f.earliest_reset), (Cap::Exhausted, None));
    assert_eq!(f.writer, "rotate");

    let recent = backdate(&r, 10 * 60);
    refresh_from_rotation(&r.root, &both, now(), false);
    assert_eq!(fs::read(r.capacity()).unwrap(), recent, "under 1h: skipped");

    backdate(&r, 2 * HOUR);
    refresh_from_rotation(&r.root, &both, now(), false);
    let f = cap::read(&r.root).unwrap();
    assert!(
        (f.written_at.timestamp() - now()).abs() <= 5,
        "heartbeat keeps a still-true unknown-reset state from expiring"
    );
}

#[test]
fn rotation_resets_come_from_usage_json_and_the_auto_switch_threshold() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "b"]);
    config::write(&r.root, &cfg(80, &[])).unwrap();
    let t = now();
    r.state
        .save_usage(&usage_file(&[
            (
                "a",
                reading(win(85.0, t + HOUR), win(5.0, t + 3 * DAY), t - 60),
            ),
            (
                "b",
                reading(win(5.0, t + 2 * HOUR), win(82.0, t + 2 * DAY), t - 60),
            ),
        ]))
        .unwrap();
    refresh_from_rotation(
        &r.root,
        &[
            (
                names(&["a"])[0].clone(),
                Rotated::Measured {
                    session: 85.0,
                    week: 5.0,
                },
            ),
            (
                names(&["b"])[0].clone(),
                Rotated::Measured {
                    session: 5.0,
                    week: 82.0,
                },
            ),
        ],
        t,
        false,
    );
    let f = cap::read(&r.root).expect("written");
    assert_eq!(f.state, Cap::Low);
    assert_eq!(f.threshold, 80, "the auto-switch threshold, not session_at");
    assert_eq!(f.earliest_reset, Some(ts(t + HOUR)));
}

#[test]
fn rotation_with_a_failed_probe_is_ok_and_writes_nothing_new() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "b"]);
    refresh_from_rotation(
        &r.root,
        &[
            (names(&["a"])[0].clone(), Rotated::Exhausted),
            (names(&["b"])[0].clone(), Rotated::Failed),
        ],
        now(),
        false,
    );
    assert!(!r.capacity().exists());
}

#[test]
fn write_errors_are_swallowed() {
    let _l = lock();
    let _on = on();
    let r = root(&["a"]);
    fs::create_dir(r.capacity()).unwrap(); // a directory where the file goes
    refresh(
        &r.state,
        &usage_file(&[("a", over(100.0, HOUR))]),
        &Ledger::default(),
        "statusline",
    );
    refresh_from_rotation(
        &r.root,
        &[(names(&["a"])[0].clone(), Rotated::Exhausted)],
        now(),
        false,
    );
    assert!(r.capacity().is_dir(), "left alone, no panic");
}

// ---------------------------------------------------------------------------
// Launch and recipe-run warnings
// ---------------------------------------------------------------------------

/// A root selected through the environment, auto-switch on, with `state`
/// planted. Hold [`lock`] while it lives.
struct Planted {
    _env: EnvGuard,
    r: Root,
    file: CapacityFile,
}

fn planted(state: Cap, toggle: &str) -> Planted {
    let r = root(&["a", "b"]);
    let env = EnvGuard::set([
        ("HOME", r.home.to_str().unwrap()),
        (crate::commands::profile::ROOT_ENV, r.root.to_str().unwrap()),
        (config::TOGGLE_ENV, toggle),
        ("TZ", "UTC"),
        ("AMPLIHACK_SESSION_DEPTH", "0"),
    ]);
    let t = now();
    let file = CapacityFile {
        version: 1,
        state,
        threshold: 90,
        over: vec!["a".into(), "b".into()],
        earliest_reset: Some(ts(t + 3 * HOUR)),
        written_at: ts(t),
        writer: "statusline".into(),
    };
    let bytes = serde_json::to_vec(&file).unwrap();
    super::fsx::write_private_atomic(&r.root.join(".auto-switch.d"), cap::FILE, &bytes).unwrap();
    Planted { _env: env, r, file }
}

fn line(p: &Planted) -> String {
    cap::format_line(p.file.state, p.file.threshold, p.file.earliest_reset).unwrap()
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn launch(a: &[&str]) -> String {
    let mut out = Vec::new();
    launch_warning(&args(a), &mut out);
    String::from_utf8(out).unwrap()
}

fn recipe() -> String {
    let mut out = Vec::new();
    recipe_warning(&mut out);
    String::from_utf8(out).unwrap()
}

#[test]
fn launch_warning_is_one_line_with_the_inspect_command() {
    let _l = lock();
    for state in [Cap::Low, Cap::Exhausted] {
        let p = planted(state, "1");
        assert_eq!(
            launch(&[]),
            format!("{} — run 'amplihack profile usage'\n", line(&p))
        );
        assert_eq!(
            launch(&["--model", "opus", "--resume"]),
            format!("{} — run 'amplihack profile usage'\n", line(&p))
        );
    }
}

#[test]
fn launch_warning_is_silent_for_print_mode_and_nested_sessions() {
    let _l = lock();
    let _p = planted(Cap::Exhausted, "1");
    assert_eq!(launch(&["-p", "hello"]), "");
    assert_eq!(launch(&["--print", "hello"]), "");
    assert_eq!(launch(&["--model", "x", "-p"]), "");
    let _nested = EnvGuard::set([("AMPLIHACK_SESSION_DEPTH", "1")]);
    assert_eq!(launch(&[]), "");
}

#[test]
fn launch_warning_is_silent_when_off_ok_missing_or_past_reset() {
    let _l = lock();
    {
        let _p = planted(Cap::Exhausted, "0");
        assert_eq!(launch(&[]), "", "off");
    }
    {
        let _p = planted(Cap::Ok, "1");
        assert_eq!(launch(&[]), "", "ok");
    }
    {
        let p = planted(Cap::Low, "1");
        fs::remove_file(p.r.capacity()).unwrap();
        assert_eq!(launch(&[]), "", "missing");
        fs::write(p.r.capacity(), b"{corrupt").unwrap();
        assert_eq!(launch(&[]), "", "corrupt");
    }
    {
        let p = planted(Cap::Low, "1");
        let mut f = p.file.clone();
        f.earliest_reset = Some(ts(now() - 1));
        super::fsx::write_private_atomic(
            &p.r.root.join(".auto-switch.d"),
            cap::FILE,
            &serde_json::to_vec(&f).unwrap(),
        )
        .unwrap();
        assert_eq!(launch(&[]), "", "past the reset");
    }
    {
        let _p = planted(Cap::Low, "1");
        let _bad = EnvGuard::set([(crate::commands::profile::ROOT_ENV, "relative")]);
        assert_eq!(launch(&[]), "", "unresolvable root");
    }
}

#[test]
fn recipe_warning_says_continuing_anyway() {
    let _l = lock();
    for state in [Cap::Low, Cap::Exhausted] {
        let p = planted(state, "1");
        assert_eq!(
            recipe(),
            format!(
                "amplihack recipe run: {} — run 'amplihack profile usage' (continuing anyway)\n",
                line(&p)
            )
        );
        assert!(recipe().contains("next reset "), "includes the reset time");
    }
}

#[test]
fn recipe_warning_is_silent_when_off_or_ok() {
    let _l = lock();
    {
        let _p = planted(Cap::Low, "0");
        assert_eq!(recipe(), "");
    }
    {
        let _p = planted(Cap::Ok, "1");
        assert_eq!(recipe(), "");
    }
}

// ---------------------------------------------------------------------------
// refresh_from_rotation on credits (issue #1606)
// ---------------------------------------------------------------------------

#[test]
fn on_overflow_overwrites_an_exhausted_file_with_ok() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "cred"]);
    let a = names(&["a"])[0].clone();
    let cred = names(&["cred"])[0].clone();
    let both = [(a, Rotated::Exhausted), (cred, Rotated::Exhausted)];
    refresh_from_rotation(&r.root, &both, now(), false);
    assert_eq!(cap::read(&r.root).expect("written").state, Cap::Exhausted);
    refresh_from_rotation(&r.root, &both, now(), true);
    let f = cap::read(&r.root).expect("still a file");
    assert_eq!(
        f.state,
        Cap::Ok,
        "the live login runs on overflow: work can go on"
    );
    assert_eq!(f.writer, "rotate");
}

#[test]
fn on_overflow_never_creates_the_file() {
    let _l = lock();
    let _on = on();
    let r = root(&["a", "cred"]);
    let both = [
        (names(&["a"])[0].clone(), Rotated::Exhausted),
        (names(&["cred"])[0].clone(), Rotated::Exhausted),
    ];
    refresh_from_rotation(&r.root, &both, now(), true);
    assert!(!r.capacity().exists());
}

#[test]
fn off_overflow_the_result_is_as_before() {
    let _l = lock();
    let _on = on();
    let r = root(&["a"]);
    let one = [(names(&["a"])[0].clone(), Rotated::Exhausted)];
    refresh_from_rotation(&r.root, &one, now(), false);
    assert_eq!(cap::read(&r.root).expect("written").state, Cap::Exhausted);
}

#[test]
fn on_overflow_writes_nothing_while_auto_switch_is_off() {
    let _l = lock();
    let _off = EnvGuard::set([(config::TOGGLE_ENV, "0")]);
    let r = root(&["a"]);
    refresh_from_rotation(
        &r.root,
        &[(names(&["a"])[0].clone(), Rotated::Exhausted)],
        now(),
        true,
    );
    assert!(!r.capacity().exists());
}
