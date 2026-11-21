//! Issues #2890 and #2882: a step's spend-limit failure blocks the login it
//! ran under, and the rotator reads that block as "no room".
//!
//! Contract (these tests fail until it exists):
//! - `ledger::OverflowBlock::spend_limit_until(at, until) -> OverflowBlock`:
//!   reason `spend_limit`, the stated end rather than the monthly assumption.
//!   `OverflowBlock::spend_limit(at)` is unchanged.
//! - `step_block::note_spend_block(root: &Path, login: &ProfileName, at: i64,
//!   until: i64)` returns `()`. Under the rotate state lock it inserts
//!   `overflow_blocked[login] = spend_limit_until(at, until)` and writes one
//!   `ALERT: login <name> hit its spend limit; blocked until <time>` line to
//!   the rotate log. Order: ledger exists, span valid, lock, load, prune and
//!   active check, cardinality, save, log. It never creates a ledger, never
//!   extends an active block, never stores a block `valid_overflow` would
//!   refuse, never panics, and reports every failure on stderr and in the
//!   rotate log as `ALERT: login <name> spend limit not blocked (<why>)`
//!   with `<why>` one of `stated reset already passed`, `stated reset out
//!   of range`, `rotator busy`, `ledger unreadable`, `ledger not written`,
//!   `too many blocks`.
//! - `msg::alert_step_spend_limit(&ProfileName, until) -> String` and
//!   `msg::step_spend_limit_not_blocked(&ProfileName, why: &str) -> String`
//!   with the two texts above (without the `ALERT: ` prefix being optional:
//!   the prefix is part of the returned string).
//! - `overflow::decide_restricted`: a plan login (not an overflow profile)
//!   with an active block contributes `Reading::Exhausted { None, None }`
//!   and no windows, as the live login and as a candidate.
//! - `control::blocked_logins_line(overflow_shown, ledger, now) -> String`:
//!   active blocks on logins not in `overflow_shown`, in name order, each
//!   `<name> (blocked until <time>)`, joined by `, `; `-` for none and for
//!   an unreadable ledger (no `(blocks unknown)` suffix).

use super::control::blocked_logins_line;
use super::decide::{Decision, Measured, Probed, Reading, Reason, Windows};
use super::ledger::{
    MAX_BLOCK_SECS, MAX_BLOCKS, OverflowBlock, RotateLedger, RotateState, SPEND_LIMIT,
    active_blocks, prune_blocks,
};
use super::log::{msg, timestamp};
use super::overflow::{OverflowInput, decide_overflow};
use super::settings::Settings;
use super::step_block::note_spend_block;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

const NOW: i64 = 1_760_000_000;
const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

struct Root {
    _tmp: tempfile::TempDir,
    path: PathBuf,
    rs: RotateState,
}

/// A private root with an existing, empty rotate ledger.
fn root() -> Root {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = tmp.path().to_path_buf();
    let rs = RotateState::new(&path);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger {
        live: Some("wwest".into()),
        ..RotateLedger::default()
    })
    .unwrap();
    Root {
        _tmp: tmp,
        path,
        rs,
    }
}

fn loaded(r: &Root) -> RotateLedger {
    r.rs.load_ledger().unwrap().expect("a ledger")
}

fn log_text(r: &Root) -> String {
    fs::read_to_string(&r.rs.log).unwrap_or_default()
}

fn spend(at: i64, until: i64) -> OverflowBlock {
    OverflowBlock {
        at,
        until,
        reason: SPEND_LIMIT.into(),
    }
}

// --- the constructor -----------------------------------------------------------

#[test]
fn spend_limit_until_keeps_the_stated_end_and_the_reason() {
    let b = OverflowBlock::spend_limit_until(NOW, NOW + WEEK);
    assert_eq!(b, spend(NOW, NOW + WEEK));
    assert_eq!(b.reason, SPEND_LIMIT);
}

#[test]
fn the_overflow_constructor_keeps_the_monthly_assumption() {
    let b = OverflowBlock::spend_limit(NOW);
    assert_eq!(b.until, super::ledger::spend_reset(NOW));
}

// --- the writer -----------------------------------------------------------------

#[test]
fn a_stated_reset_is_stored_for_the_login_with_one_alert() {
    let r = root();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + WEEK);
    let l = loaded(&r);
    assert_eq!(l.overflow_blocked.len(), 1);
    assert_eq!(l.overflow_blocked["wwest"], spend(NOW, NOW + WEEK));
    assert_eq!(l.live.as_deref(), Some("wwest"), "other fields kept");
    let log = log_text(&r);
    let expected = msg::alert_step_spend_limit(&n("wwest"), NOW + WEEK);
    assert_eq!(
        expected,
        format!(
            "ALERT: login wwest hit its spend limit; blocked until {}",
            timestamp(NOW + WEEK)
        )
    );
    assert_eq!(log.matches(&expected).count(), 1, "{log}");
}

#[test]
fn the_stored_block_is_active_until_the_reset_and_expires_with_it() {
    let r = root();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + WEEK);
    let l = loaded(&r);
    assert!(active_blocks(&l, NOW + WEEK - 1).contains_key("wwest"));
    assert!(!active_blocks(&l, NOW + WEEK).contains_key("wwest"));
    let mut l = l;
    prune_blocks(&mut l, NOW + WEEK);
    assert!(l.overflow_blocked.is_empty(), "no new lifecycle");
}

#[test]
fn a_step_written_ledger_still_loads_through_the_validator() {
    let r = root();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + MAX_BLOCK_SECS);
    assert!(r.rs.load_ledger().is_ok());
    assert_eq!(loaded(&r).overflow_blocked.len(), 1);
}

#[test]
fn an_active_block_is_neither_extended_nor_alerted_twice() {
    let r = root();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + WEEK);
    note_spend_block(&r.path, &n("wwest"), NOW + 60, NOW + 2 * WEEK);
    let l = loaded(&r);
    assert_eq!(l.overflow_blocked["wwest"], spend(NOW, NOW + WEEK));
    let first = msg::alert_step_spend_limit(&n("wwest"), NOW + WEEK);
    assert_eq!(log_text(&r).matches(&first).count(), 1);
    assert!(!log_text(&r).contains("not blocked"), "{}", log_text(&r));
}

#[test]
fn an_expired_block_is_replaced() {
    let r = root();
    let mut l = loaded(&r);
    l.overflow_blocked
        .insert("wwest".into(), spend(NOW - 3 * DAY, NOW - DAY));
    r.rs.save_ledger(&l).unwrap();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + WEEK);
    assert_eq!(loaded(&r).overflow_blocked["wwest"], spend(NOW, NOW + WEEK));
}

#[test]
fn a_second_login_gets_its_own_block() {
    let r = root();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + WEEK);
    note_spend_block(&r.path, &n("other"), NOW, NOW + 2 * DAY);
    let l = loaded(&r);
    assert_eq!(l.overflow_blocked.len(), 2);
    assert_eq!(l.overflow_blocked["other"].until, NOW + 2 * DAY);
}

#[test]
fn a_missing_ledger_is_not_created() {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    note_spend_block(tmp.path(), &n("wwest"), NOW, NOW + WEEK);
    let rs = RotateState::new(tmp.path());
    assert!(!rs.ledger.exists(), "the writer must not create a ledger");
}

#[test]
fn a_reset_in_the_past_or_out_of_range_is_reported_and_not_stored() {
    // The two cases read differently to whoever finds the line: a reset the
    // clock has already gone past is an ordinary limit that has lapsed, not
    // a reset too far ahead to store.
    for (label, until, why) in [
        ("past", NOW - 1, "stated reset already passed"),
        ("equal to now", NOW, "stated reset already passed"),
        ("minimum integer", i64::MIN, "stated reset already passed"),
        (
            "beyond the cap",
            NOW + MAX_BLOCK_SECS + 1,
            "stated reset out of range",
        ),
        ("maximum integer", i64::MAX, "stated reset out of range"),
    ] {
        let r = root();
        note_spend_block(&r.path, &n("wwest"), NOW, until);
        let l = loaded(&r);
        assert!(l.overflow_blocked.is_empty(), "{label}: stored");
        let line = msg::step_spend_limit_not_blocked(&n("wwest"), why);
        assert_eq!(
            line,
            format!("ALERT: login wwest spend limit not blocked ({why})")
        );
        assert!(log_text(&r).contains(&line), "{label}: {}", log_text(&r));
    }
}

#[test]
fn a_reset_exactly_at_the_cap_is_accepted() {
    let r = root();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + MAX_BLOCK_SECS);
    assert_eq!(loaded(&r).overflow_blocked.len(), 1);
}

#[test]
fn a_non_positive_clock_is_reported_and_not_stored() {
    for at in [0, -5, i64::MIN] {
        let r = root();
        note_spend_block(&r.path, &n("wwest"), at, NOW + WEEK);
        assert!(loaded(&r).overflow_blocked.is_empty(), "at={at}");
    }
}

#[test]
fn a_full_ledger_stores_nothing_and_still_loads() {
    let r = root();
    let mut l = loaded(&r);
    for i in 0..MAX_BLOCKS {
        l.overflow_blocked
            .insert(format!("p{i}"), spend(NOW, NOW + WEEK));
    }
    r.rs.save_ledger(&l).unwrap();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + WEEK);
    let l = loaded(&r);
    assert_eq!(l.overflow_blocked.len(), MAX_BLOCKS);
    assert!(!l.overflow_blocked.contains_key("wwest"));
    assert!(
        log_text(&r).contains("ALERT: login wwest spend limit not blocked (too many blocks)"),
        "{}",
        log_text(&r)
    );
}

#[test]
fn the_largest_permitted_ledger_with_long_names_still_loads() {
    // 256 entries must stay under the ledger's document cap, or a write the
    // entry check allowed would produce a ledger the next load refuses.
    let r = root();
    let mut l = loaded(&r);
    let long = "x".repeat(60);
    for i in 0..MAX_BLOCKS - 1 {
        l.overflow_blocked
            .insert(format!("{long}{i:03}"), spend(NOW, NOW + MAX_BLOCK_SECS));
    }
    r.rs.save_ledger(&l).unwrap();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + MAX_BLOCK_SECS);
    let l = loaded(&r);
    assert_eq!(l.overflow_blocked.len(), MAX_BLOCKS);
}

#[test]
fn a_busy_rotator_loses_the_block_and_never_stalls_the_step() {
    let r = root();
    let state = State::new(&r.rs.root).unwrap();
    let _held = state
        .try_lock(Duration::from_secs(1))
        .unwrap()
        .expect("the test takes the lock first");
    let started = std::time::Instant::now();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + WEEK);
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "the writer waits a bounded time"
    );
    drop(_held);
    assert!(loaded(&r).overflow_blocked.is_empty());
}

#[test]
fn an_unreadable_ledger_is_left_alone() {
    let r = root();
    fs::write(&r.rs.ledger, b"{ not json").unwrap();
    fs::set_permissions(&r.rs.ledger, fs::Permissions::from_mode(0o600)).unwrap();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + WEEK);
    assert_eq!(fs::read(&r.rs.ledger).unwrap(), b"{ not json");
}

#[test]
fn a_symlinked_ledger_is_not_followed() {
    let r = root();
    let target = r.path.join("elsewhere.json");
    fs::write(&target, b"original").unwrap();
    fs::remove_file(&r.rs.ledger).unwrap();
    std::os::unix::fs::symlink(&target, &r.rs.ledger).unwrap();
    note_spend_block(&r.path, &n("wwest"), NOW, NOW + WEEK);
    assert_eq!(fs::read(&target).unwrap(), b"original");
}

#[test]
fn the_alert_lines_carry_only_a_name_a_time_and_a_closed_reason() {
    // The message the step read never reaches the log: the writer takes no
    // text, so an escape sequence or newline in agent output cannot appear.
    let line = msg::alert_step_spend_limit(&n("wwest"), NOW + WEEK);
    assert!(!line.contains('\n') && !line.contains('\u{1b}'));
    for why in [
        "stated reset already passed",
        "stated reset out of range",
        "rotator busy",
        "ledger unreadable",
        "ledger not written",
        "too many blocks",
    ] {
        let l = msg::step_spend_limit_not_blocked(&n("wwest"), why);
        assert_eq!(
            l,
            format!("ALERT: login wwest spend limit not blocked ({why})")
        );
    }
}

#[test]
fn the_existing_overflow_message_is_left_alone() {
    // Its text says "overflow profile", which is wrong for a plan login.
    let line = msg::spend_limit_not_blocked(&n("cred"), "x");
    assert!(line.contains("overflow profile cred"), "{line}");
}

// --- the rotator reads the block as "no room" -------------------------------------

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

fn healthy(session: f64, week: f64) -> Probed {
    Probed {
        reading: Reading::Measured { session, week },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
        passive: false,
    }
}

fn cand(name: &str, p: &Probed) -> Measured {
    Measured {
        name: n(name),
        reading: p.reading,
        windows: p.windows.clone(),
        spend_limit: p.spend_limit,
        unknown: p.unknown,
    }
}

fn cycle(
    live: &str,
    live_p: &Probed,
    cands: &[Measured],
    blocks: &BTreeMap<String, OverflowBlock>,
    now: i64,
) -> Decision {
    let live_name = n(live);
    decide_overflow(&OverflowInput {
        settings: &settings(),
        live_name: Some(&live_name),
        live: live_p,
        candidates: cands,
        unmeasured_plan: &[],
        refused: &[],
        overflow: &[],
        blocks,
        platform_block: None,
        swapped_at: None,
        now,
    })
    .decision
}

fn one_block(name: &str, until: i64) -> BTreeMap<String, OverflowBlock> {
    BTreeMap::from([(name.to_string(), spend(NOW - 60, until))])
}

#[test]
fn a_healthy_live_login_with_no_block_stays() {
    let live = healthy(10.0, 10.0);
    let c = [cand("b", &healthy(20.0, 20.0))];
    assert_eq!(cycle("a", &live, &c, &BTreeMap::new(), NOW), Decision::Stay);
}

#[test]
fn a_blocked_live_login_swaps_to_a_login_with_room() {
    // The probe cannot see the spend limit, so the reading is healthy.
    let live = healthy(10.0, 10.0);
    let c = [cand("b", &healthy(20.0, 20.0))];
    let d = cycle("a", &live, &c, &one_block("a", NOW + WEEK), NOW);
    assert!(
        matches!(&d, Decision::Swap { to, .. } if to.as_str() == "b"),
        "{d:?}"
    );
}

#[test]
fn a_blocked_candidate_is_not_chosen_even_with_the_most_room() {
    let live = healthy(90.0, 50.0);
    let c = [
        cand("blocked", &healthy(0.0, 0.0)),
        cand("free", &healthy(40.0, 40.0)),
    ];
    let d = cycle("a", &live, &c, &one_block("blocked", NOW + WEEK), NOW);
    assert!(
        matches!(&d, Decision::Swap { to, .. } if to.as_str() == "free"),
        "{d:?}"
    );
}

#[test]
fn the_blocked_login_is_chosen_again_only_after_its_reset() {
    let live = healthy(90.0, 50.0);
    let c = [cand("blocked", &healthy(0.0, 0.0))];
    let blocks = one_block("blocked", NOW + WEEK);
    let before = cycle("a", &live, &c, &blocks, NOW + WEEK - 1);
    assert!(
        !matches!(&before, Decision::Swap { to, .. } if to.as_str() == "blocked"),
        "{before:?}"
    );
    let after = cycle("a", &live, &c, &blocks, NOW + WEEK);
    assert!(
        matches!(&after, Decision::Swap { to, .. } if to.as_str() == "blocked"),
        "{after:?}"
    );
}

#[test]
fn an_expired_block_on_the_live_login_changes_nothing() {
    let live = healthy(10.0, 10.0);
    let c = [cand("b", &healthy(20.0, 20.0))];
    let d = cycle("a", &live, &c, &one_block("a", NOW - 1), NOW);
    assert_eq!(d, Decision::Stay);
}

#[test]
fn a_blocked_live_login_with_no_login_to_go_to_alerts_and_does_not_panic() {
    let live = healthy(10.0, 10.0);
    let d = cycle("a", &live, &[], &one_block("a", NOW + WEEK), NOW);
    assert!(matches!(d, Decision::Alert | Decision::Stay), "{d:?}");
    assert!(!matches!(d, Decision::Swap { .. }));
}

#[test]
fn a_blocked_live_login_is_not_swapped_onto_another_blocked_login() {
    let live = healthy(10.0, 10.0);
    let c = [cand("b", &healthy(0.0, 0.0))];
    let mut blocks = one_block("a", NOW + WEEK);
    blocks.insert("b".into(), spend(NOW - 60, NOW + WEEK));
    let d = cycle("a", &live, &c, &blocks, NOW);
    assert!(!matches!(d, Decision::Swap { .. }), "{d:?}");
}

#[test]
fn the_swap_off_a_blocked_login_is_not_held_by_the_swap_hold() {
    // The login cannot be used at all, so the usual hold after a swap
    // must not keep the cycle on it. (`swapped_at` is recent here.)
    let live_name = n("a");
    let live = healthy(10.0, 10.0);
    let c = [cand("b", &healthy(20.0, 20.0))];
    let blocks = one_block("a", NOW + WEEK);
    let d = decide_overflow(&OverflowInput {
        settings: &settings(),
        live_name: Some(&live_name),
        live: &live,
        candidates: &c,
        unmeasured_plan: &[],
        refused: &[],
        overflow: &[],
        blocks: &blocks,
        platform_block: None,
        swapped_at: Some(NOW - 10),
        now: NOW,
    })
    .decision;
    assert!(
        matches!(&d, Decision::Swap { to, reason } if to.as_str() == "b" && *reason != Reason::OverflowIn),
        "{d:?}"
    );
}

// --- rotate status ----------------------------------------------------------------

fn ledger_with(blocks: &[(&str, i64)]) -> RotateLedger {
    let mut l = RotateLedger::default();
    for (name, until) in blocks {
        l.overflow_blocked
            .insert((*name).to_string(), spend(NOW - 60, *until));
    }
    l
}

#[test]
fn no_active_block_prints_a_dash() {
    let l = ledger_with(&[]);
    assert_eq!(blocked_logins_line(&[], Ok(Some(&l)), NOW), "-");
    assert_eq!(blocked_logins_line(&[], Ok(None), NOW), "-");
}

#[test]
fn an_active_block_is_listed_with_its_reset() {
    let l = ledger_with(&[("wwest", NOW + WEEK)]);
    assert_eq!(
        blocked_logins_line(&[], Ok(Some(&l)), NOW),
        format!("wwest (blocked until {})", timestamp(NOW + WEEK))
    );
}

#[test]
fn blocks_are_listed_in_name_order_and_expired_ones_are_left_out() {
    let l = ledger_with(&[("zed", NOW + DAY), ("amy", NOW + 2 * DAY), ("old", NOW - 1)]);
    assert_eq!(
        blocked_logins_line(&[], Ok(Some(&l)), NOW),
        format!(
            "amy (blocked until {}), zed (blocked until {})",
            timestamp(NOW + 2 * DAY),
            timestamp(NOW + DAY)
        )
    );
}

#[test]
fn a_login_the_overflow_field_already_shows_is_not_repeated() {
    let l = ledger_with(&[("cred", NOW + DAY), ("wwest", NOW + WEEK)]);
    let shown = [n("cred")];
    assert_eq!(
        blocked_logins_line(&shown, Ok(Some(&l)), NOW),
        format!("wwest (blocked until {})", timestamp(NOW + WEEK))
    );
    let only = ledger_with(&[("cred", NOW + DAY)]);
    assert_eq!(blocked_logins_line(&shown, Ok(Some(&only)), NOW), "-");
}

#[test]
fn an_unreadable_ledger_prints_a_dash_without_the_overflow_suffix() {
    let e = ProfileError::RotateInconsistent("x".into());
    let line = blocked_logins_line(&[], Err(&e), NOW);
    assert_eq!(line, "-");
    assert!(!line.contains("blocks unknown"));
}
