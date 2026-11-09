//! Issue #2013 (D11): the hourly alert limit.
//!
//! Contract (`rotate/alert_limit.rs`, pure):
//! - `AlertSeen { last_logged_at: i64, seen_at: i64, suppressed: u32 }`,
//!   serde, kept in `RotateLedger.alert_log: BTreeMap<String, AlertSeen>`
//!   (serde default, omitted when empty).
//! - `note(&mut BTreeMap<String, AlertSeen>, key, t, line) -> Note`, with
//!   `Note::{Log(String), Suppress}`. The first occurrence logs `line`;
//!   repeats less than 3600 s after `last_logged_at` are counted; the first
//!   repeat 3600 s or more later logs
//!   `<line> (repeated <n> times since <last>)`, `<last>` being
//!   `last_logged_at` as `YYYY-MM-DDTHH:MM:SSZ`, and resets the count. A
//!   `last_logged_at` later than `t` logs at once. Every call sets
//!   `seen_at = t`.
//! - `prune(&mut map, cycle_started_at)` drops keys not seen in this cycle.
//! - At most `MAX_ENTRIES` (64) entries; the oldest `seen_at` goes first.
//! - Loading a ledger drops bad entries (key over 320 bytes or with control
//!   characters, negative time, time more than a day ahead) and keeps the
//!   64 newest; it never makes the ledger `RotateInconsistent`.
//! - `for_older_readers` clears `alert_log`.
//! - In a cycle, refusal lines (key `<kind>:<p>`) go through the limit; the
//!   ledger's `alert` field is still set on every refused cycle.

use super::alert_limit::{AlertSeen, MAX_ENTRIES, Note, note, prune};
use super::cycle::{CycleOutcome, Mode, run_cycle};
use super::ledger::{RotateLedger, for_older_readers};
use super::log::timestamp;
use super::observe::{Measurement, Snap, decide_measured};
use super::tests_issue_2013_common::{ctx_at, lines, rd, record, set_now, settings};
use super::tests_revalidate_cause::probes_for;
use super::tests_swap::{env, ino_at};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use std::collections::BTreeMap;
use std::fs;

const T0: i64 = 1_794_200_000;
const LINE: &str = "ALERT: swap refused (account-mismatch, profile work): x";

fn suffix(n: u32, last: i64) -> String {
    format!("{LINE} (repeated {n} times since {})", timestamp(last))
}

// --- The pure limiter -----------------------------------------------------------------

#[test]
fn the_documented_sequence_at_injected_times() {
    let mut map = BTreeMap::new();
    let k = "account-mismatch:work";
    assert_eq!(note(&mut map, k, T0, LINE), Note::Log(LINE.into()));
    assert_eq!(note(&mut map, k, T0 + 60, LINE), Note::Suppress);
    assert_eq!(note(&mut map, k, T0 + 3599, LINE), Note::Suppress);
    assert_eq!(map[k].suppressed, 2);
    assert_eq!(note(&mut map, k, T0 + 3600, LINE), Note::Log(suffix(2, T0)));
    assert_eq!(map[k].suppressed, 0);
    assert_eq!(map[k].last_logged_at, T0 + 3600);
    assert_eq!(note(&mut map, k, T0 + 3700, LINE), Note::Suppress);
    assert_eq!(map[k].seen_at, T0 + 3700);
    // A cycle without the cause clears the key.
    prune(&mut map, T0 + 3800);
    assert!(!map.contains_key(k));
    assert_eq!(note(&mut map, k, T0 + 3900, LINE), Note::Log(LINE.into()));
}

#[test]
fn the_interval_120_example_counts_29_repeats_per_hour() {
    let mut map = BTreeMap::new();
    let k = "account-mismatch:nnorth";
    let mut logged = Vec::new();
    let mut t = T0;
    while t <= T0 + 7200 {
        if let Note::Log(l) = note(&mut map, k, t, LINE) {
            logged.push(l);
        }
        t += 120;
    }
    assert_eq!(
        logged,
        [LINE.to_string(), suffix(29, T0), suffix(29, T0 + 3600)]
    );
}

#[test]
fn prune_keeps_the_keys_seen_in_this_cycle() {
    let mut map = BTreeMap::new();
    note(&mut map, "a:x", T0, LINE);
    note(&mut map, "b:x", T0 + 100, LINE);
    prune(&mut map, T0 + 100);
    assert_eq!(map.keys().collect::<Vec<_>>(), ["b:x"]);
}

#[test]
fn a_last_logged_time_in_the_future_logs_at_once() {
    let mut map = BTreeMap::new();
    map.insert(
        "k:x".to_string(),
        AlertSeen {
            last_logged_at: T0 + 500,
            seen_at: T0 + 500,
            suppressed: 7,
        },
    );
    match note(&mut map, "k:x", T0, LINE) {
        Note::Log(l) => assert!(l.starts_with(LINE), "{l}"),
        Note::Suppress => panic!("a clock step back must not silence the line"),
    }
    assert_eq!(map["k:x"].last_logged_at, T0);
}

#[test]
fn keys_are_independent() {
    let mut map = BTreeMap::new();
    assert!(matches!(note(&mut map, "a:x", T0, LINE), Note::Log(_)));
    assert!(matches!(note(&mut map, "b:x", T0 + 1, LINE), Note::Log(_)));
    assert_eq!(note(&mut map, "a:x", T0 + 2, LINE), Note::Suppress);
}

#[test]
fn the_map_holds_at_most_64_entries_and_drops_the_oldest_seen_first() {
    assert_eq!(MAX_ENTRIES, 64);
    let mut map = BTreeMap::new();
    for i in 0..70 {
        note(&mut map, &format!("k{i:02}:x"), T0 + i, LINE);
    }
    assert_eq!(map.len(), 64);
    assert!(!map.contains_key("k00:x"));
    assert!(!map.contains_key("k05:x"));
    assert!(map.contains_key("k06:x"));
    assert!(map.contains_key("k69:x"));
}

#[test]
fn extreme_times_do_not_overflow() {
    let mut map = BTreeMap::new();
    note(&mut map, "k:x", i64::MIN, LINE);
    assert!(matches!(
        note(&mut map, "k:x", i64::MAX, LINE),
        Note::Log(_)
    ));
}

// --- The ledger field ---------------------------------------------------------------

#[test]
fn alert_log_round_trips_through_the_ledger() {
    let e = env();
    let mut l = e.rs.load_ledger().unwrap().unwrap();
    let t = now();
    l.alert_log.insert(
        "repair:work".into(),
        AlertSeen {
            last_logged_at: t - 10,
            seen_at: t,
            suppressed: 3,
        },
    );
    e.rs.save_ledger(&l).unwrap();
    let back = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(back.alert_log, l.alert_log);
}

#[test]
fn an_empty_alert_log_is_not_written_and_an_older_ledger_without_it_loads() {
    let e = env();
    let raw = fs::read_to_string(&e.rs.ledger).unwrap();
    assert!(!raw.contains("alert_log"), "{raw}");
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert!(l.alert_log.is_empty());
}

fn write_raw_ledger(e: &super::tests_swap::Env, alert_log: serde_json::Value) {
    let l = e.rs.load_ledger().unwrap().unwrap();
    let mut v = serde_json::to_value(&l).unwrap();
    v["alert_log"] = alert_log;
    e.rs.write("ledger.json", serde_json::to_vec(&v).unwrap().as_slice())
        .unwrap();
}

#[test]
fn bad_entries_are_dropped_on_load_and_never_make_the_ledger_inconsistent() {
    let e = env();
    let t = now();
    let good = serde_json::json!({"last_logged_at": t - 5, "seen_at": t, "suppressed": 1});
    let mut m = serde_json::Map::new();
    m.insert("repair:work".into(), good.clone());
    m.insert("bad\u{1b}key:work".into(), good.clone());
    m.insert("x".repeat(321), good.clone());
    m.insert(
        "neg:work".into(),
        serde_json::json!({"last_logged_at": -1, "seen_at": t, "suppressed": 0}),
    );
    m.insert(
        "ahead:work".into(),
        serde_json::json!({"last_logged_at": t, "seen_at": t + 2 * 86_400, "suppressed": 0}),
    );
    m.insert("shape:work".into(), serde_json::json!("not an object"));
    write_raw_ledger(&e, serde_json::Value::Object(m));
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(
        l.alert_log.keys().collect::<Vec<_>>(),
        ["repair:work"],
        "{:?}",
        l.alert_log
    );
}

#[test]
fn more_than_64_entries_load_as_the_64_newest() {
    let e = env();
    let t = now();
    let mut m = serde_json::Map::new();
    for i in 0..70i64 {
        m.insert(
            format!("k{i:02}:work"),
            serde_json::json!({"last_logged_at": t - 1000 + i, "seen_at": t - 1000 + i, "suppressed": 0}),
        );
    }
    write_raw_ledger(&e, serde_json::Value::Object(m));
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.alert_log.len(), 64);
    assert!(!l.alert_log.contains_key("k05:work"));
    assert!(l.alert_log.contains_key("k06:work"));
}

#[test]
fn an_older_reader_never_sees_alert_log() {
    let mut l = RotateLedger::default();
    l.alert_log.insert(
        "repair:work".into(),
        AlertSeen {
            last_logged_at: 1,
            seen_at: 1,
            suppressed: 0,
        },
    );
    assert!(for_older_readers(&l, true).alert_log.is_empty());
    assert_eq!(for_older_readers(&l, false).alert_log, l.alert_log);
}

// --- In a cycle -------------------------------------------------------------------------

fn measure_ok(ctx: &super::cycle::Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("spare", rd(0.0, 10.0, None))]);
    decide_measured(
        ctx,
        snap,
        super::tests_revalidate_cause::spent(),
        probes,
        at,
    )
}

fn refusals(log: &[String]) -> Vec<String> {
    log.iter()
        .filter(|l| l.starts_with("ALERT: swap refused ("))
        .cloned()
        .collect()
}

/// The record for `work` names another account than the verified login in
/// `~/.claude`: every cycle refuses. The refusal is logged at `t`, counted
/// at `t+60` and `t+3599`, logged with its count at `t+3600`, and counted
/// at `t+3700`. Once the record is put right the swap goes ahead on the
/// next normal cycle, and the key is cleared.
#[cfg(target_os = "linux")]
#[test]
fn a_refusal_repeats_at_most_once_an_hour_with_a_count_and_the_swap_retries_when_it_clears() {
    let e = env();
    record(&e.root, "work", "u-SENTINEL-recorded", "other@example.com");
    let t = now();
    let mut c = ctx_at(&e, measure_ok, settings(), t);
    for at in [t, t + 60, t + 3599, t + 3600, t + 3700] {
        set_now(at);
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Alerted,
            "{:?}",
            lines(&e)
        );
        let l = e.rs.load_ledger().unwrap().unwrap();
        assert_eq!(
            l.alert.as_deref(),
            Some("account_mismatch:work"),
            "the ledger shows the state on every cycle"
        );
    }
    let got = refusals(&lines(&e));
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(
        got[0].starts_with("ALERT: swap refused (account-mismatch, profile work): "),
        "{}",
        got[0]
    );
    assert!(
        got[1].ends_with(&format!(" (repeated 2 times since {})", timestamp(t))),
        "{}",
        got[1]
    );
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.alert_log["account-mismatch:work"].suppressed, 1);

    // The cause clears: the next normal cycle swaps, with no retry loop.
    fs::remove_file(e.root.join(".auto-switch.d/expected/work.json")).unwrap();
    set_now(t + 3800);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        lines(&e)
    );
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert!(
        !l.alert_log.contains_key("account-mismatch:work"),
        "{:?}",
        l.alert_log
    );
}

/// The one-shot command still prints and returns every refusal; only the
/// log line is limited.
#[cfg(target_os = "linux")]
#[test]
fn a_one_shot_run_returns_the_refusal_every_time() {
    let e = env();
    record(&e.root, "work", "u-SENTINEL-recorded", "other@example.com");
    let t = now();
    let mut c = ctx_at(&e, measure_ok, settings(), t);
    for at in [t, t + 60] {
        set_now(at);
        assert!(matches!(
            run_cycle(&mut c, Mode::OneShot),
            Err(ProfileError::RotateRefused(_))
        ));
    }
    assert_eq!(refusals(&lines(&e)).len(), 1, "{:?}", lines(&e));
}
