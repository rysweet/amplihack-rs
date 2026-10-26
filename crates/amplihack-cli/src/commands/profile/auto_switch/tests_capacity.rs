//! The no-capacity notice, writer side: `compute`, the write rules, the
//! utils-reader round trip, and the parity tables that keep amplihack-utils'
//! root and gate in step with this crate's. Also the launch and recipe-run
//! warning lines.
//!
//! Every root is a temporary directory (0700); `HOME` is temporary wherever it
//! matters. Tests that touch the environment hold `env_lock()`.

use super::capacity::{
    Evidence, Rotated, compute, launch_warning, recipe_warning, refresh, refresh_from_rotation,
};
use super::config::{self, Config};
use super::state::{HostEntry, Ledger, State, UsageFile};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::root_config;
use crate::test_support::{EnvGuard, env_lock};
use amplihack_utils::claude_capacity::{self as cap, CapacityFile, State as Cap};
use chrono::DateTime;
use std::collections::BTreeMap;
use std::ffi::OsString;
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
    refresh_from_rotation(&r.root, &both, now());
    let f = cap::read(&r.root).expect("written");
    assert_eq!((f.state, f.earliest_reset), (Cap::Exhausted, None));
    assert_eq!(f.writer, "rotate");

    let recent = backdate(&r, 10 * 60);
    refresh_from_rotation(&r.root, &both, now());
    assert_eq!(fs::read(r.capacity()).unwrap(), recent, "under 1h: skipped");

    backdate(&r, 2 * HOUR);
    refresh_from_rotation(&r.root, &both, now());
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
    );
    assert!(r.capacity().is_dir(), "left alone, no panic");
}

// ---------------------------------------------------------------------------
// Parity: utils' gate and root must give this crate's answers
// ---------------------------------------------------------------------------

#[test]
fn gate_parity_table() {
    let _l = lock();
    let r = root(&["a"]);
    let toggle = r.root.join(config::FILE);
    type Plant<'a> = Box<dyn Fn() + 'a>;
    let files: [(&str, Plant<'_>); 5] = [
        (
            "on",
            Box::new(|| {
                config::write(&r.root, &cfg(90, &[])).unwrap();
            }),
        ),
        (
            "off",
            Box::new(|| {
                let mut c = cfg(90, &[]);
                c.enabled = false;
                config::write(&r.root, &c).unwrap();
            }),
        ),
        ("missing", Box::new(|| {})),
        (
            "corrupt",
            Box::new(|| {
                fs::write(&toggle, b"enabled=maybe\n").unwrap();
                fs::set_permissions(&toggle, fs::Permissions::from_mode(0o600)).unwrap();
            }),
        ),
        (
            "symlink",
            Box::new(|| {
                let real = r.root.join("real-toggle");
                fs::write(&real, b"enabled=on\n").unwrap();
                fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
                std::os::unix::fs::symlink(&real, &toggle).unwrap();
            }),
        ),
    ];
    for env in [Some("1"), Some("0"), None, Some("garbage")] {
        for (why, plant) in &files {
            let _ = fs::remove_file(&toggle);
            let _ = fs::remove_file(r.root.join("real-toggle"));
            plant();
            let _env = match env {
                Some(v) => EnvGuard::set([(config::TOGGLE_ENV, v)]),
                None => EnvGuard::unset([config::TOGGLE_ENV]),
            };
            assert_eq!(
                cap::auto_switch_on(&r.root),
                config::effective(&r.root).on,
                "env {env:?}, file {why}"
            );
        }
    }
    // And the table is not trivially all-false.
    let _env = EnvGuard::unset([config::TOGGLE_ENV]);
    let _ = fs::remove_file(&toggle);
    config::write(&r.root, &cfg(90, &[])).unwrap();
    assert!(cap::auto_switch_on(&r.root));
}

fn env_of(value: Option<OsString>) -> impl Fn(&str) -> Option<OsString> {
    move |k| {
        if k == crate::commands::profile::ROOT_ENV {
            value.clone()
        } else {
            None
        }
    }
}

fn saved(home: &Path, bytes: &[u8], mode: u32) {
    let dir = root_config::config_dir(home);
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join(root_config::PROFILE_ROOT_FILE);
    fs::write(&f, bytes).unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn root_parity_table() {
    use std::os::unix::ffi::OsStringExt;
    type Plant = Box<dyn Fn(&Path)>;
    let none: Plant = Box::new(|_| {});
    let rows: Vec<(&str, Option<OsString>, Plant)> = vec![
        ("env absolute", Some("/srv/p".into()), Box::new(|_| {})),
        ("env relative", Some("srv/p".into()), Box::new(|_| {})),
        ("env dot-dot", Some("/srv/../p".into()), Box::new(|_| {})),
        ("env control", Some("/srv/\u{1b}p".into()), Box::new(|_| {})),
        (
            "env non-UTF-8",
            Some(OsString::from_vec(b"/srv/\xff".to_vec())),
            Box::new(|_| {}),
        ),
        ("env empty", Some("".into()), Box::new(|_| {})),
        ("default", None, none),
        (
            "saved ok",
            None,
            Box::new(|h| saved(h, b"/srv/saved\n", 0o600)),
        ),
        (
            "saved relative",
            None,
            Box::new(|h| saved(h, b"srv/saved\n", 0o600)),
        ),
        (
            "saved dot-dot",
            None,
            Box::new(|h| saved(h, b"/srv/../x\n", 0o600)),
        ),
        (
            "saved control",
            None,
            Box::new(|h| saved(h, b"/srv/\x07x\n", 0o600)),
        ),
        (
            "saved non-UTF-8",
            None,
            Box::new(|h| saved(h, b"/srv/\xffx\n", 0o600)),
        ),
        (
            "saved group-writable",
            None,
            Box::new(|h| saved(h, b"/srv/x\n", 0o660)),
        ),
        (
            "saved too large",
            None,
            Box::new(|h| {
                let mut big = b"/srv/".to_vec();
                big.extend(std::iter::repeat_n(b'x', 5000));
                saved(h, &big, 0o600);
            }),
        ),
        (
            "saved symlink",
            None,
            Box::new(|h| {
                let real = h.join("real");
                fs::write(&real, b"/srv/x\n").unwrap();
                fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
                let dir = root_config::config_dir(h);
                fs::create_dir_all(&dir).unwrap();
                std::os::unix::fs::symlink(&real, dir.join(root_config::PROFILE_ROOT_FILE))
                    .unwrap();
            }),
        ),
    ];
    for (why, env, plant) in rows {
        let home = tempfile::tempdir().unwrap();
        plant(home.path());
        let cli = root_config::resolve_with(env_of(env.clone()), Some(home.path()))
            .ok()
            .map(|(p, _)| p);
        let utils = cap::resolve_root_with(env_of(env), Some(home.path()));
        assert_eq!(utils, cli, "{why}");
    }
    assert_eq!(
        cap::resolve_root_with(env_of(None), None),
        root_config::resolve_with(env_of(None), None)
            .ok()
            .map(|(p, _)| p),
        "no HOME"
    );
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
