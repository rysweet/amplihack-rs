//! Issue #1880: failed and limited live logins in real cycles.
//!
//! The swap, journal, ledger and log are the real ones, under a temporary
//! HOME and profile root (`tests_swap::env()`: `work` live, `spare` stored,
//! plus the profiles a test adds). Probing is stubbed with `Ctx::measure`,
//! which calls the real decision (`decide_measured`), and `Ctx::reread`. No
//! test reads a real login, starts `claude`, uses the network or a tmux
//! server, or inherits the developer's profile root.
//!
//! Every cycle runs with a new `Ctx`, as a restart, a separate one-shot
//! `rotate` or a separate `--dry-run` process would: the count must come
//! from the ledger (D4). The ledger's `failed_live` field is read and seeded
//! through the file's JSON, so this file uses only what exists on `main`
//! and fails there for the reason the issue gives.
//!
//! Cases follow D3 of the final requirements: case A (unknown failure, an
//! alternative at 0%/14%), case B (a plan login at its spend limit with
//! 3,014 s of hold left, ffarmer at 90/26 and a third login at 91/26), and
//! one test per probe outcome.
//!
//! Root cause, from the host's `rotate.log` on 2026-11-03. The count was
//! not stuck below 3. For `mmorrison`, one live cycle failed (04:23:41Z,
//! `(1/3)`) before the hand switch at 04:26:18Z; the 04:25:26Z `would stay
//! (live probe failed (1/3))` line was a `--dry-run`, which on `main` built
//! a new context with a count of 0 and could print nothing but `(1/3)`. The
//! login was at its limit, yet the probe read `Failed`, so it needed three
//! cycles: case A. For `rssourcelabs`, the count reached 3 at 04:59:55Z,
//! and the post-swap hold from the 04:50:10Z swap turned the swap into
//! `held after last swap (3014s left)`: case B. On `main` the count was also
//! held in memory only, so a restart or a one-shot run started again at 0.

use super::cycle::{Ctx, CycleOutcome, MeasureFn, Mode, run_cycle};
use super::decide::{Probed, Reading, Win, Windows};
use super::fresh::RereadFn;
use super::ledger::RotateLedger;
use super::observe::{Measurement, Snap, decide_measured};
use super::settings;
use super::tests_issue_1870::{exh, rd};
use super::tests_revalidate_cause::{add_stored, ctx_with, log_of, probes_for};
use super::tests_swap::{Env, env, ino_at, json, n, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::config::{self, ReadMode};
use crate::commands::profile::auto_switch::kit::{euid, now};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::OverflowEntry;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::Duration;

/// Seconds since the last swap in case B: 3,014 s of a 3,600 s hold left.
const CASE_B_SWAPPED_AGO: i64 = 586;

// --- Fixture ---------------------------------------------------------------

/// The ledger file as JSON; the default ledger when there is none.
fn ledger_json(e: &Env) -> Value {
    match fs::read(&e.rs.ledger) {
        Ok(raw) => serde_json::from_slice(&raw).unwrap(),
        Err(_) => serde_json::to_value(RotateLedger::default()).unwrap(),
    }
}

/// Rewrites the ledger file with `f` applied, byte for byte, so a field this
/// binary does not know yet survives (`save_ledger` would drop it).
fn edit_ledger(e: &Env, f: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    let mut v = ledger_json(e);
    f(v.as_object_mut().unwrap());
    e.rs.write("ledger.json", v.to_string().as_bytes()).unwrap();
}

/// The stored count, as `(profile, count)`.
fn stored(e: &Env) -> Option<(String, u64)> {
    let v = ledger_json(e);
    let f = v.get("failed_live")?;
    Some((
        f["profile"].as_str().unwrap().to_string(),
        f["count"].as_u64().unwrap(),
    ))
}

fn seed_count(e: &Env, profile: &str, count: u64) {
    edit_ledger(e, |m| {
        m.insert(
            "failed_live".into(),
            json!({"profile": profile, "count": count}),
        );
    });
}

/// `work` became live `ago` seconds ago by a swap: the hold is active.
fn seed_hold(e: &Env, ago: i64) {
    let t = now() - ago;
    edit_ledger(e, |m| {
        m.insert("live".into(), json!("work"));
        m.insert("account".into(), json!("u-work"));
        m.insert("since".into(), json!(t));
        m.insert("swapped_at".into(), json!(t));
    });
}

/// The fixture keeps a key in `~/.claude.json`, which stops a failed-probe
/// swap (`damp::keep_for_home_key`); the failed-probe tests remove it.
fn take_key_from_home(e: &Env) {
    let path = e.home.join(".claude.json");
    let mut home = json(&path);
    home.as_object_mut().unwrap().remove("primaryApiKey");
    private(&path, &home.to_string());
}

fn mark_overflow(e: &Env, name: &str) {
    config::update(&e.root, ReadMode::Editable, |c| {
        c.overflow.push(OverflowEntry::new(n(name), 1));
        Ok(())
    })
    .unwrap();
}

/// One cycle with a new context, as a new process would run it.
fn cycle(e: &Env, measure: MeasureFn, mode: Mode) -> CycleOutcome {
    let mut c = ctx_with(e, measure);
    run_cycle(&mut c, mode).unwrap()
}

fn cycle_with(e: &Env, c: &mut Ctx, mode: Mode) -> CycleOutcome {
    let got = run_cycle(c, mode);
    got.unwrap_or_else(|err| panic!("{err}\n{}", log_of(e)))
}

fn swaps(log: &str) -> Vec<&str> {
    log.lines().filter(|l| l.contains(" swap ")).collect()
}

fn swapped_away(e: &Env) -> BTreeMap<String, i64> {
    e.rs.load_ledger()
        .unwrap()
        .map(|l| l.swapped_away)
        .unwrap_or_default()
}

/// Every file in the rotate state directory, with its bytes.
fn state_files(e: &Env) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(&e.rs.dir)
        .unwrap()
        .map(|d| d.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read(&p).unwrap())
        })
        .collect()
}

fn assert_no_secret(e: &Env) {
    for (name, bytes) in state_files(e) {
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("SECRET-KEY"), "{name} holds a key:\n{text}");
    }
}

// --- Probes ----------------------------------------------------------------

/// A plan login at its organisation's spend limit (reads `Failed`).
fn spend_limited() -> Probed {
    Probed {
        reading: Reading::Failed,
        windows: Windows::default(),
        spend_limit: true,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
    }
}

/// `claude -p /usage` failed or printed something unreadable.
fn errored() -> Probed {
    Probed {
        reading: Reading::Failed,
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
    }
}

/// The read had not finished when the cycle stopped waiting (#1698, #1844).
fn still_running() -> Probed {
    let mut p = Probed::failed();
    p.still_running = true;
    p
}

fn with_live(
    ctx: &Ctx,
    snap: &Snap,
    live: Probed,
    table: &[(&str, Probed)],
) -> Result<Measurement, ProfileError> {
    let at = now();
    decide_measured(ctx, snap, live, probes_for(snap, table), at)
}

/// Case A: the live probe times out; `spare` is at 0%/14%.
fn case_a_timeout(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(
        ctx,
        snap,
        Probed::failed(),
        &[("spare", rd(0.0, 14.0, None))],
    )
}

fn case_a_still_running(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(
        ctx,
        snap,
        still_running(),
        &[("spare", rd(0.0, 14.0, None))],
    )
}

fn case_a_error(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(ctx, snap, errored(), &[("spare", rd(0.0, 14.0, None))])
}

/// Case A, second variant: the live probe reports the spend limit.
fn case_a_spend(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(
        ctx,
        snap,
        spend_limited(),
        &[("spare", rd(0.0, 14.0, None))],
    )
}

/// Case B: `work` at its spend limit; `ffarmer` 90/26, `spare` 91/26.
fn case_b(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(
        ctx,
        snap,
        spend_limited(),
        &[
            ("ffarmer", rd(90.0, 26.0, None)),
            ("spare", rd(91.0, 26.0, None)),
        ],
    )
}

fn fresh_ok(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(
        ctx,
        snap,
        rd(10.0, 10.0, None),
        &[("spare", rd(0.0, 14.0, None))],
    )
}

fn carried_ok(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let mut live = rd(10.0, 10.0, None);
    live.carried = true;
    with_live(ctx, snap, live, &[("spare", rd(0.0, 14.0, None))])
}

/// After the case A swap `spare` is live, and its probe fails too.
fn spare_fails(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(
        ctx,
        snap,
        Probed::failed(),
        &[("work", rd(0.0, 20.0, None))],
    )
}

/// No candidate meets the margin: `spare` has 3 points of room.
fn failed_plan_room(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(
        ctx,
        snap,
        Probed::failed(),
        &[("spare", rd(97.0, 26.0, None))],
    )
}

/// Room only on the overflow profile `cred`.
fn failed_overflow_only(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(
        ctx,
        snap,
        Probed::failed(),
        &[("spare", exh(100.0, 100.0)), ("cred", rd(0.0, 0.0, None))],
    )
}

/// `work` is an overflow profile at its spend limit; no plan login has room.
fn overflow_spend(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(ctx, snap, spend_limited(), &[("spare", exh(100.0, 100.0))])
}

// --- Case A: unknown failure, three cycles, the hold active -----------------

/// Three cycles, each with a new `Ctx`, inside the hold: 1/3, 2/3, swap.
fn three_failed_cycles_swap_inside_the_hold(measure: MeasureFn, live_shown: &str) {
    let e = env();
    take_key_from_home(&e);
    seed_hold(&e, 60);

    assert_eq!(cycle(&e, measure, Mode::Watch), CycleOutcome::Stayed);
    let log = log_of(&e);
    assert!(log.contains("stay: live probe failed (1/3)"), "{log}");
    assert_eq!(stored(&e), Some(("work".into(), 1)), "{log}");

    assert_eq!(
        cycle(&e, measure, Mode::Watch),
        CycleOutcome::Stayed,
        "{}",
        log_of(&e)
    );
    let log = log_of(&e);
    assert!(
        log.contains("stay: live probe failed (2/3)"),
        "a restart must not lose the count:\n{log}"
    );
    assert_eq!(stored(&e), Some(("work".into(), 2)), "{log}");

    let got = cycle(&e, measure, Mode::Watch);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    assert!(
        log.contains(&format!(
            "swap work->spare live {live_shown} target s=0% w=14% reason=failed-probe"
        )),
        "{log}"
    );
    assert!(!log.contains("held after last swap"), "{log}");
    assert_eq!(stored(&e), None, "a swap clears the count");
    assert!(ledger_json(&e)["swapped_at"].as_i64().unwrap() > now() - 30);
}

#[test]
fn case_a_a_timed_out_live_probe_swaps_on_the_third_cycle_inside_the_hold() {
    three_failed_cycles_swap_inside_the_hold(case_a_timeout, "failed");
}

#[test]
fn case_a_a_read_still_running_counts_and_swaps_on_the_third_cycle() {
    three_failed_cycles_swap_inside_the_hold(case_a_still_running, "unknown, still running");
}

#[test]
fn case_a_an_unknown_error_swaps_on_the_third_cycle_inside_the_hold() {
    three_failed_cycles_swap_inside_the_hold(case_a_error, "failed");
}

/// `~/.claude` holds a login no profile matches (a hand `/login`): the live
/// identity is `LiveId::Adopt`, and the home file has no key.
fn adopted_live_login(e: &Env) {
    private(
        &e.home.join(".claude.json"),
        &json!({"oauthAccount": {
            "accountUuid": "u-new",
            "emailAddress": "new@example.com"
        }})
        .to_string(),
    );
}

#[test]
fn an_adopted_live_login_whose_probe_keeps_failing_swaps_on_the_third_cycle() {
    // Crusty round 1 on #1880: the count of an adopted login is stored under
    // `failed_store::ADOPTED`, so it reaches the failed-probe swap as a
    // profile's count does, and as the in-memory count on `main` did.
    let e = env();
    adopted_live_login(&e);
    for k in 1..=2 {
        let got = cycle(&e, case_a_timeout, Mode::Watch);
        let log = log_of(&e);
        assert_eq!(got, CycleOutcome::Stayed, "k={k}\n{log}");
        assert!(
            log.contains(&format!("stay: live probe failed ({k}/3)")),
            "a restart must not lose the count:\n{log}"
        );
        assert_eq!(stored(&e), Some(("-".into(), k)), "{log}");
    }
    let got = cycle(&e, case_a_timeout, Mode::Watch);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    let swapped = swaps(&log);
    assert_eq!(swapped.len(), 1, "{log}");
    assert!(
        swapped[0].contains("->spare") && swapped[0].contains("reason=failed-probe"),
        "{log}"
    );
    assert_eq!(stored(&e), None, "a swap clears the count");
}

#[test]
fn a_profile_count_does_not_carry_over_to_an_adopted_login() {
    let e = env();
    adopted_live_login(&e);
    seed_count(&e, "work", 2);
    assert_eq!(
        cycle(&e, case_a_timeout, Mode::Watch),
        CycleOutcome::Stayed,
        "{}",
        log_of(&e)
    );
    assert!(log_of(&e).contains("stay: live probe failed (1/3)"));
    assert_eq!(stored(&e), Some(("-".into(), 1)));
}

#[test]
fn case_a_a_spend_limit_swaps_on_the_first_cycle_inside_the_hold() {
    let e = env();
    take_key_from_home(&e);
    seed_hold(&e, 60);
    let got = cycle(&e, case_a_spend, Mode::Watch);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert!(
        log.contains("swap work->spare live spend limit target s=0% w=14% reason=spend-limit"),
        "{log}"
    );
    assert!(
        !log.contains("live probe failed"),
        "not a failed probe:\n{log}"
    );
    assert_eq!(stored(&e), None);
}

// --- Case B: a plan login at its spend limit, 3,014 s of hold left ----------

fn case_b_env() -> Env {
    let e = env();
    add_stored(&e, "ffarmer");
    seed_hold(&e, CASE_B_SWAPPED_AGO);
    e
}

fn assert_case_b_swapped(e: &Env, got: CycleOutcome) {
    let log = log_of(e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert!(
        log.contains("cycle live=work work:spend limit ffarmer:s=90%,w=26% spare:s=91%,w=26%"),
        "{log}"
    );
    assert!(
        log.contains("swap work->ffarmer live spend limit target s=90% w=26% reason=spend-limit"),
        "{log}"
    );
    assert_eq!(swaps(&log).len(), 1, "{log}");
    assert_eq!(ledger_json(e)["live"], json!("ffarmer"));
    assert_eq!(stored(e), None);
    assert!(
        swapped_away(e).contains_key("work"),
        "the spent login is not swapped straight back to: {:?}",
        swapped_away(e)
    );
}

#[test]
fn case_b_a_plan_spend_limit_swaps_on_the_first_cycle_with_default_settings() {
    let e = case_b_env();
    let got = cycle(&e, case_b, Mode::Watch);
    assert_case_b_swapped(&e, got);
}

#[test]
fn case_b_a_plan_spend_limit_swaps_on_the_first_cycle_at_session_at_70() {
    let e = case_b_env();
    let mut c = ctx_with(&e, case_b);
    let tune = super::TuneFlags {
        session_at: Some("70".into()),
        ..super::TuneFlags::default()
    };
    c.settings = settings::resolve(&tune, None, &|_| None).unwrap();
    assert_eq!(c.settings.session_at, 70);
    let got = cycle_with(&e, &mut c, Mode::Watch);
    assert_case_b_swapped(&e, got);
}

#[test]
fn case_b_the_home_key_does_not_stop_a_spend_limit_swap() {
    // D8: the fixture's home key stays; the limit belongs to the account.
    let e = case_b_env();
    assert!(
        json(&e.home.join(".claude.json"))
            .get("primaryApiKey")
            .is_some()
    );
    let got = cycle(&e, case_b, Mode::Watch);
    assert_case_b_swapped(&e, got);
}

// --- Persistence (D4) --------------------------------------------------------

#[test]
fn a_stored_count_of_two_swaps_on_the_next_restart() {
    let e = env();
    take_key_from_home(&e);
    seed_count(&e, "work", 2);
    let got = cycle(&e, case_a_timeout, Mode::Watch);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert!(log.contains("reason=failed-probe"), "{log}");
}

#[test]
fn a_count_stored_for_another_profile_reads_as_zero() {
    let e = env();
    take_key_from_home(&e);
    seed_count(&e, "spare", 2);
    let got = cycle(&e, case_a_timeout, Mode::Watch);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Stayed, "{log}");
    assert!(log.contains("stay: live probe failed (1/3)"), "{log}");
    assert_eq!(stored(&e), Some(("work".into(), 1)));
}

#[test]
fn one_shot_runs_count_across_invocations() {
    let e = env();
    take_key_from_home(&e);
    let outcomes: Vec<CycleOutcome> = (0..3)
        .map(|_| cycle(&e, case_a_timeout, Mode::OneShot))
        .collect();
    assert_eq!(
        outcomes,
        [
            CycleOutcome::Stayed,
            CycleOutcome::Stayed,
            CycleOutcome::Swapped
        ],
        "{}",
        log_of(&e)
    );
}

#[test]
fn the_ledger_stays_private_after_the_count_is_written() {
    let e = env();
    take_key_from_home(&e);
    cycle(&e, case_a_timeout, Mode::Watch);
    assert_eq!(stored(&e), Some(("work".into(), 1)));
    let meta = fs::metadata(&e.rs.ledger).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(meta.uid(), euid());
}

// --- Resets (D5) ---------------------------------------------------------------

#[test]
fn a_fresh_reading_clears_the_count() {
    let e = env();
    seed_count(&e, "work", 2);
    let got = cycle(&e, fresh_ok, Mode::Watch);
    assert_eq!(got, CycleOutcome::Stayed, "{}", log_of(&e));
    assert_eq!(stored(&e), None);
}

#[test]
fn a_carried_reading_keeps_the_count() {
    let e = env();
    seed_count(&e, "work", 2);
    let got = cycle(&e, carried_ok, Mode::Watch);
    assert_eq!(got, CycleOutcome::Stayed, "{}", log_of(&e));
    assert_eq!(
        stored(&e),
        Some(("work".into(), 2)),
        "a carried reading neither counts nor resets"
    );
}

#[test]
fn a_spend_limit_on_a_live_overflow_profile_keeps_the_count() {
    let e = env();
    mark_overflow(&e, "work");
    seed_count(&e, "work", 2);
    let got = cycle(&e, overflow_spend, Mode::Watch);
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "precondition: no swap\n{log}");
    assert_eq!(stored(&e), Some(("work".into(), 2)), "{log}");
}

#[test]
fn after_a_failed_probe_swap_the_new_login_needs_three_failures_of_its_own() {
    let e = env();
    take_key_from_home(&e);
    seed_count(&e, "work", 2);
    assert_eq!(
        cycle(&e, case_a_timeout, Mode::Watch),
        CycleOutcome::Swapped
    );
    for k in 1..=2 {
        let got = cycle(&e, spare_fails, Mode::Watch);
        let log = log_of(&e);
        assert_eq!(got, CycleOutcome::Stayed, "k={k}\n{log}");
        assert!(
            log.contains(&format!("stay: live probe failed ({k}/3)")),
            "{log}"
        );
        assert_eq!(stored(&e), Some(("spare".into(), k)));
    }
    assert_eq!(swaps(&log_of(&e)).len(), 1, "{}", log_of(&e));
}

// --- Home key (D8) -------------------------------------------------------------

#[test]
fn the_home_key_still_stops_a_failed_probe_swap_at_the_limit() {
    let e = env();
    seed_count(&e, "work", 2);
    let got = cycle(&e, case_a_timeout, Mode::Watch);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Stayed, "{log}");
    assert!(
        log.contains("stay: live probe failed; home key (primaryApiKey set)"),
        "{log}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(stored(&e), Some(("work".into(), 3)), "the count goes on");
}

// --- Plan room and overflow (D7) -------------------------------------------------

#[test]
fn a_failed_live_login_at_the_limit_reaches_the_plan_room_swap() {
    let e = env();
    take_key_from_home(&e);
    seed_count(&e, "work", 2);
    let got = cycle(&e, failed_plan_room, Mode::Watch);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert!(
        log.contains("swap work->spare live failed target s=97% w=26% reason=failed-probe"),
        "{log}"
    );
}

#[test]
fn below_the_limit_a_failed_live_login_does_not_reach_plan_room() {
    let e = env();
    take_key_from_home(&e);
    let got = cycle(&e, failed_plan_room, Mode::Watch);
    assert_ne!(got, CycleOutcome::Swapped, "{}", log_of(&e));
    assert_eq!(ino_at(&e.live()), Some(e.l));
}

#[test]
fn a_failed_live_login_never_moves_onto_overflow_at_the_limit() {
    let e = env();
    take_key_from_home(&e);
    add_stored(&e, "cred");
    mark_overflow(&e, "cred");
    seed_count(&e, "work", 2);
    let got = cycle(&e, failed_overflow_only, Mode::Watch);
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
    assert!(swaps(&log).is_empty(), "{log}");
}

// --- Freshness gate (D8) ----------------------------------------------------------

fn reread_spare(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    assert_eq!(name, &n("spare"), "only the stale target is read again");
    rd(0.0, 50.0, None)
}

/// `spare` looks best on an old reading; `third`, read now, beats `spare`'s
/// fresh figures.
fn spend_stale_target(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    with_live(
        ctx,
        snap,
        spend_limited(),
        &[
            ("spare", rd(0.0, 10.0, Some(at - 30))),
            ("third", rd(0.0, 30.0, None)),
        ],
    )
}

#[test]
fn the_freshness_gate_covers_a_spend_limit_swap() {
    let e = env();
    let third = add_stored(&e, "third");
    let mut c = ctx_with(&e, spend_stale_target);
    c.reread = Some(reread_spare as RereadFn);
    let got = cycle_with(&e, &mut c, Mode::Watch);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert!(
        log.contains("reread: spare reading stale s=0% w=10%; fresh s=0% w=50%"),
        "{log}"
    );
    assert_eq!(ino_at(&e.live()), Some(third));
    assert!(log.contains("swap work->third "), "{log}");
    assert!(log.contains("reason=spend-limit"), "{log}");
}

// --- Dry run (D6) ----------------------------------------------------------------

#[test]
fn a_dry_run_reports_the_stored_count_plus_one_and_writes_nothing() {
    let e = env();
    take_key_from_home(&e);
    seed_count(&e, "work", 1);
    let before = state_files(&e);
    let mtime = fs::metadata(&e.rs.ledger).unwrap().modified().unwrap();
    assert_eq!(
        cycle(&e, case_a_timeout, Mode::DryRun),
        CycleOutcome::Stayed
    );
    assert_eq!(state_files(&e), before, "nothing written, no lock file");
    assert_eq!(
        fs::metadata(&e.rs.ledger).unwrap().modified().unwrap(),
        mtime
    );
}

#[test]
fn repeated_dry_runs_at_a_stored_count_of_two_each_report_the_swap() {
    let e = env();
    take_key_from_home(&e);
    seed_count(&e, "work", 2);
    let before = state_files(&e);
    for i in 0..3 {
        assert_eq!(
            cycle(&e, case_a_timeout, Mode::DryRun),
            CycleOutcome::Swapped,
            "dry run {i}: (3/3) means the next cycle swaps"
        );
    }
    assert_eq!(state_files(&e), before);
    assert_eq!(ino_at(&e.live()), Some(e.l), "a dry run moves nothing");
    assert_eq!(stored(&e), Some(("work".into(), 2)));
}

// --- Credentials (requirement 7) ---------------------------------------------------

#[test]
fn no_key_reaches_the_ledger_log_or_state_files() {
    let e = case_b_env();
    // A malformed count holding a key-like value is dropped without a trace.
    edit_ledger(&e, |m| {
        m.insert(
            "failed_live".into(),
            json!({"profile": "work", "count": "SECRET-KEY-X"}),
        );
    });
    cycle(&e, case_b, Mode::DryRun);
    assert_eq!(cycle(&e, case_b, Mode::Watch), CycleOutcome::Swapped);
    assert_no_secret(&e);
    let log = log_of(&e);
    assert!(!log.contains("SECRET-KEY"), "{log}");
}

// --- Resume (D9) -----------------------------------------------------------------

#[test]
fn the_limit_resume_hook_sees_capacity_on_another_login_after_a_spend_limit() {
    // The hook (`amplihack-hooks limit-resume`) wakes a session stopped at
    // the limit menu when the ledger records a fresh cycle with capacity; it
    // never answers the menu itself. It must wake onto a different login,
    // not the one at its spend limit.
    let e = case_b_env();
    let limit_at = now() - 1;
    let got = cycle(&e, case_b, Mode::Watch);
    assert_case_b_swapped(&e, got);
    let status = amplihack_utils::rotator_status::read_cycle(&e.root, now()).unwrap();
    assert!(status.fresh_capacity(limit_at), "{status:?}");
    assert_eq!(status.live.as_deref(), Some("ffarmer"));
}

#[test]
fn the_limit_resume_hook_reads_a_ledger_holding_the_count() {
    let e = env();
    take_key_from_home(&e);
    assert_eq!(cycle(&e, case_a_timeout, Mode::Watch), CycleOutcome::Stayed);
    assert_eq!(stored(&e), Some(("work".into(), 1)));
    assert!(
        amplihack_utils::rotator_status::read_cycle(&e.root, now()).is_some(),
        "the hook's reader must accept the new field"
    );
}

// --- A spend-limited plan login and overflow (review finding A) ---------------

/// An exhausted probe whose session and week both reset `in_secs` from now.
fn exh_resetting(in_secs: i64) -> Probed {
    let at = Some(now() + in_secs);
    Probed {
        reading: Reading::Exhausted {
            session: Some(100.0),
            week: Some(100.0),
        },
        windows: Windows {
            session: Some(Win {
                used: 100.0,
                resets_at: at,
            }),
            week: Some(Win {
                used: 100.0,
                resets_at: at,
            }),
            no_session_limit: false,
        },
        ..exh(100.0, 100.0)
    }
}

/// `work` is a plan login at its spend limit; `spare`, the only other plan
/// login, is exhausted until `in_secs` from now; `cred` has room.
fn spend_overflow(in_secs: i64) -> impl Fn(&Ctx, &Snap) -> Result<Measurement, ProfileError> {
    move |ctx, snap| {
        with_live(
            ctx,
            snap,
            spend_limited(),
            &[
                ("spare", exh_resetting(in_secs)),
                ("cred", rd(0.0, 0.0, None)),
            ],
        )
    }
}

fn spend_overflow_far(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    // Two hours: beyond the default overflow wait of 1,800 s.
    spend_overflow(7_200)(ctx, snap)
}

fn spend_overflow_near(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    // Ten minutes: inside the wait.
    spend_overflow(600)(ctx, snap)
}

/// The home key is removed: it holds every per-token move on its own.
fn spend_overflow_env() -> Env {
    let e = env();
    take_key_from_home(&e);
    add_stored(&e, "cred");
    mark_overflow(&e, "cred");
    e
}

#[test]
fn spend_limited_plan_login_moves_to_overflow_when_resets_are_far() {
    let e = spend_overflow_env();
    seed_count(&e, "work", 2);
    let mut c = ctx_with(&e, spend_overflow_far);
    assert_eq!(c.settings.overflow_wait, 1800, "precondition");
    let got = cycle_with(&e, &mut c, Mode::Watch);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert!(log.contains("swap work->cred "), "{log}");
    assert!(log.contains("reason=overflow-in"), "{log}");
    assert_eq!(swaps(&log).len(), 1, "{log}");
    assert_eq!(ledger_json(&e)["live"], json!("cred"));
    assert_eq!(stored(&e), None, "the swap clears the count");
}

#[test]
fn spend_limited_plan_login_stays_when_a_reset_is_near() {
    let e = spend_overflow_env();
    let got = cycle(&e, spend_overflow_near, Mode::Watch);
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert!(swaps(&log).is_empty(), "{log}");
    assert!(!log.contains("overflow-in"), "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
}

/// `work` is an overflow profile at its spend limit; `spare` is exhausted
/// with no readable reset, so the wait gate holds (`--overflow-unknown-reset`
/// is off); `cred`, a second credit profile, has room.
fn overflow_spend_with_cred(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    with_live(
        ctx,
        snap,
        spend_limited(),
        &[("spare", exh(100.0, 100.0)), ("cred", rd(0.0, 0.0, None))],
    )
}

#[test]
fn spend_limited_live_overflow_profile_stays_blocked() {
    assert_eq!(
        super::live_limit::classify(&spend_limited(), true),
        super::live_limit::LiveOutcome::OverflowBlocked
    );
    let e = spend_overflow_env();
    mark_overflow(&e, "work");
    seed_count(&e, "work", 2);
    let got = cycle(&e, overflow_spend_with_cred, Mode::Watch);
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert!(!log.contains("overflow-in"), "{log}");
    assert!(!log.contains("swap work->cred"), "{log}");
    assert_eq!(
        stored(&e),
        Some(("work".into(), 2)),
        "the count is unchanged\n{log}"
    );
}

// --- An unreadable ledger (review finding B) ------------------------------------

#[test]
fn unreadable_ledger_reports_an_error_and_does_not_swap() {
    // `failed_store::load` reads an unreadable ledger as a count of 0 and
    // logs nothing; the cycle's own `load_ledger` must report the error and
    // stop. Had the ledger loaded, the count of 2 would have swapped.
    let e = env();
    take_key_from_home(&e);
    seed_count(&e, "work", 2);
    let mut broken = ledger_json(&e).to_string();
    broken.truncate(broken.len() - 1);
    broken.push_str(",\"SECRET-KEY-Y\"");
    e.rs.write("ledger.json", broken.as_bytes()).unwrap();
    assert!(
        e.rs.load_ledger().is_err(),
        "precondition: the ledger is unreadable"
    );

    let mut c = ctx_with(&e, case_a_timeout);
    let got = run_cycle(&mut c, Mode::Watch);
    let log = log_of(&e);
    let err = got.expect_err("the cycle must report the unreadable ledger");
    assert!(
        !err.to_string().contains("SECRET-KEY"),
        "the error quotes no ledger content: {err}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
    assert!(swaps(&log).is_empty(), "{log}");
    assert!(!log.contains("SECRET-KEY"), "{log}");
}
