//! Issue #1721: step 6 of a rotate cycle revalidates only what the decision
//! depends on, and reconsiders once when only the target changed.
//!
//! Contract (see "Rotate revalidation" in docs/reference/profile-command.md):
//! - A change to the live login (`live`, `account`, `proto`,
//!   `home_no_oauth`), the pin, the overflow entries (`overflow`, `ignored`)
//!   or a rotate journal skips the cycle with
//!   `skip: state changed during measurement (<labels>)`, and never retries.
//! - For a swap, the target leaving the candidates is
//!   `target eligibility: <skip reason>` (or `gone`), and the target still a
//!   candidate with another account in its `.claude.json` is
//!   `target identity`. The cycle re-decides once under the lock, on the
//!   probes it already has, limited to the fresh candidates whose account is
//!   unchanged; a swap to such a candidate goes ahead after
//!   `revalidate: target <old> no longer eligible (<reason>); reconsidered`,
//!   where `<reason>` is the skip reason, `gone` or `account changed`.
//! - When `~/.claude.json` held an account in step 3 and none in step 6, and
//!   nothing but the live login and the target changed, the cycle logs
//!   `revalidate: ~/.claude.json held no account; read again`, calls
//!   `Ctx::pause` once and compares again. A different account, a journal,
//!   a pin or an overflow change is never read again.
//! - Any other profile's churn (parked, a session, a lost login) never
//!   cancels a swap, a stay or an ALERT.
//!
//! In process, on the fixture from `tests_revalidate_cause.rs`. Each test
//! that turns on a session has its own static flag, so parallel tests do not
//! share one. Logins are only renamed or replaced inside the temp dir.

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::damp::FAILED_PROBE_LIMIT;
use super::decide::{Decision, Probed};
use super::identify::{self, LiveId};
use super::ledger::{Phase, RotateJournal, RotateLedger, SWITCHED};
use super::log::msg;
use super::observe::{Measurement, Snap, decide_measured};
use super::pin::{self, PinState};
use super::revalidate::REREAD;
use super::switch::{Switched, switch_live};
use super::tests_revalidate_cause::{ctx_with, env3, log_of, ok, park, probes_for, spent};
use super::tests_swap::{CREDS, env, ino_at, json, n, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::config::{self, ReadMode};
use crate::commands::profile::auto_switch::kit::{Liveness, now};
use crate::commands::profile::overflow::OverflowEntry;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The labels of the last `skip: state changed during measurement` line.
fn changed_labels(log: &str) -> Vec<String> {
    let line = log
        .lines()
        .rev()
        .find(|l| l.contains(msg::CHANGED))
        .unwrap_or_else(|| panic!("no `{}` line:\n{log}", msg::CHANGED));
    let at = line.find(msg::CHANGED).unwrap() + msg::CHANGED.len();
    let inner = line[at..]
        .strip_prefix(" (")
        .and_then(|r| r.strip_suffix(')'))
        .unwrap_or_else(|| panic!("the skip line names no changed field: {line}"));
    inner.split(", ").map(str::to_string).collect()
}

fn assert_no_retry(log: &str) {
    assert!(!log.contains("revalidate:"), "no reconsideration:\n{log}");
}

fn sessions(flag: &AtomicBool, names: &[&str]) -> Liveness {
    let mut live = BTreeSet::new();
    if flag.load(Ordering::SeqCst) {
        live.extend(names.iter().map(|s| s.to_string()));
    }
    Liveness::Known(live)
}

fn last_outcome(e: &super::tests_swap::Env) -> serde_json::Value {
    json(&e.rs.ledger)["last_cycle"]["outcome"].clone()
}

// --- Test 2: the target loses its login, no other candidate ----------------

/// Only `spare2` is probed, so it is the target and nothing else can be.
/// `spare2` is used rather than `spare` because `spare`'s `.claude.json`
/// holds an API-key setting: without its login file it becomes an API-key
/// profile, which is an `overflow` change as well.
fn spare2_loses_login(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare2", ok(0.0, 43.0))]);
    let root = ctx.store.root();
    fs::rename(root.join("spare2").join(CREDS), root.join("aside")).unwrap();
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_target_that_loses_its_login_skips_and_names_the_reason() {
    let (e, s2, _) = env3();
    let mut ctx = ctx_with(&e, spare2_loses_login);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(changed_labels(&log), ["target eligibility: no login"]);
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t), "spare was not touched");
    assert_eq!(ino_at(&e.root.join("aside")), Some(s2), "moved, not lost");
    assert_no_retry(&log);
    assert_eq!(last_outcome(&e), "skipped");
}

// --- Test 3: the target gets a session; a second candidate qualifies --------

static SPARE_SESSION_3: AtomicBool = AtomicBool::new(false);

fn spare_busy_while_measuring_3(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(
        snap,
        &[
            ("spare", ok(0.0, 10.0)),
            ("spare2", ok(0.0, 43.0)),
            ("third", spent()),
        ],
    );
    SPARE_SESSION_3.store(true, Ordering::SeqCst);
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_busy_target_is_reconsidered_once_and_the_next_candidate_gets_the_swap() {
    let (e, s2, _) = env3();
    let mut ctx = ctx_with(&e, spare_busy_while_measuring_3);
    ctx.scan = |_| sessions(&SPARE_SESSION_3, &["spare"]);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(s2), "spare2's login is live");
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t), "spare was not touched");
    assert_eq!(ino_at(&e.slot("work")), Some(e.l), "work's login was moved");
    let line = "revalidate: target spare no longer eligible (session running); reconsidered";
    assert!(log.contains(line), "{log}");
    assert!(log.contains("swap work->spare2 "), "{log}");
    assert!(
        log.find(line) < log.find("swap work->spare2 "),
        "the reconsidered line comes before the swap line:\n{log}"
    );
    assert!(!log.contains(msg::CHANGED), "{log}");
    assert_eq!(last_outcome(&e), "swapped");
    let ledger = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(ledger.live.as_deref(), Some("spare2"));
}

// --- Test 4: the target gets a session; no other candidate ------------------

static SPARE_SESSION_4: AtomicBool = AtomicBool::new(false);

fn spare_busy_while_measuring_4(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0))]);
    SPARE_SESSION_4.store(true, Ordering::SeqCst);
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_busy_target_with_no_other_candidate_skips() {
    let e = env();
    let mut ctx = ctx_with(&e, spare_busy_while_measuring_4);
    ctx.scan = |_| sessions(&SPARE_SESSION_4, &["spare"]);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(
        changed_labels(&log),
        ["target eligibility: session running"]
    );
    assert_no_retry(&log);
    e.assert_unchanged();
}

// --- Test 5: the live login changes (a `profile switch`) --------------------

fn switch_to_third(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 10.0)), ("spare2", ok(0.0, 43.0))]);
    match switch_live(&ctx.store, &ctx.home, &n("third")) {
        Ok(Ok(Switched::Moved { .. })) => {}
        other => panic!("switch during measurement failed: {other:?}"),
    }
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_switch_during_measurement_skips_without_a_retry_and_keeps_its_record() {
    let (e, _, th) = env3();
    let mut ctx = ctx_with(&e, switch_to_third);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    let labels = changed_labels(&log);
    assert!(labels.iter().any(|l| l == "live"), "{labels:?}");
    assert!(
        !labels
            .iter()
            .any(|l| l.contains("third") || l.contains('@')),
        "labels name no profile or account: {labels:?}"
    );
    assert_no_retry(&log);
    assert_eq!(ino_at(&e.live()), Some(th), "third stays live, as switched");
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t), "spare was not touched");
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("third")));
    assert_eq!(last_outcome(&e), SWITCHED, "the switch's record survives");
}

// --- Test 6: a rotate journal appears ---------------------------------------

fn journal_appears(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0))]);
    let home = ctx.home.clone();
    let ino = |p: &Path| ino_at(p).unwrap();
    ctx.rs
        .save_journal(&RotateJournal {
            version: 1,
            from: "work".into(),
            to: "spare".into(),
            adopt: false,
            phase: Phase::J0,
            home: home.to_str().unwrap().into(),
            live_ino: ino(&home.join(".claude").join(CREDS)),
            target_ino: ino(&ctx.store.root().join("spare").join(CREDS)),
            home_lock_ino: None,
            to_lock_ino: None,
            started_at: at,
            pid: std::process::id(),
        })
        .unwrap();
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_journal_written_during_measurement_skips() {
    let e = env();
    let mut ctx = ctx_with(&e, journal_appears);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(changed_labels(&log), ["journal"]);
    assert_no_retry(&log);
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
}

// --- Test 7: pin, overflow and an excluded target ---------------------------

fn pin_changes(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0))]);
    pin::write(&ctx.rs, &n("work")).unwrap();
    decide_measured(ctx, snap, spent(), probes, at)
}

fn overflow_changes(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0)), ("third", spent())]);
    config::update(ctx.store.root(), ReadMode::Editable, |c| {
        c.overflow.push(OverflowEntry::new(n("spare2"), 1));
        Ok(())
    })
    .unwrap();
    decide_measured(ctx, snap, spent(), probes, at)
}

fn target_excluded(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0))]);
    config::update(ctx.store.root(), ReadMode::Editable, |c| {
        c.exclude.push(n("spare"));
        Ok(())
    })
    .unwrap();
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_pin_written_during_measurement_skips_and_says_pin() {
    let e = env();
    let mut ctx = ctx_with(&e, pin_changes);
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Skipped
    );
    let log = log_of(&e);
    assert_eq!(changed_labels(&log), ["pin"]);
    assert_no_retry(&log);
    e.assert_unchanged();
}

#[cfg(target_os = "linux")]
#[test]
fn an_overflow_mark_made_during_measurement_skips_and_says_overflow() {
    let (e, _, _) = env3();
    let mut ctx = ctx_with(&e, overflow_changes);
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Skipped
    );
    let log = log_of(&e);
    assert_eq!(changed_labels(&log), ["overflow"]);
    assert_no_retry(&log);
    assert_eq!(ino_at(&e.live()), Some(e.l));
}

#[cfg(target_os = "linux")]
#[test]
fn a_target_excluded_during_measurement_skips_and_says_excluded() {
    let e = env();
    let mut ctx = ctx_with(&e, target_excluded);
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Skipped
    );
    let log = log_of(&e);
    assert_eq!(changed_labels(&log), ["target eligibility: excluded"]);
    e.assert_unchanged();
}

// --- Test 8: one reconsideration at most -----------------------------------

static BOTH_BUSY_8: AtomicBool = AtomicBool::new(false);

/// `spare` and `spare2` are probed and both get sessions; `third` stays a
/// candidate but was never probed, so the re-decision has no reading for it
/// and must not move the login there.
fn both_targets_busy(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 10.0)), ("spare2", ok(0.0, 43.0))]);
    BOTH_BUSY_8.store(true, Ordering::SeqCst);
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_reconsideration_that_finds_no_fresh_target_skips_once() {
    let (e, s2, th) = env3();
    let mut ctx = ctx_with(&e, both_targets_busy);
    ctx.scan = |_| sessions(&BOTH_BUSY_8, &["spare", "spare2"]);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(
        log.matches(msg::CHANGED).count(),
        1,
        "one skip line:\n{log}"
    );
    assert_eq!(
        changed_labels(&log),
        ["target eligibility: session running"]
    );
    assert_no_retry(&log);
    assert!(!log.contains("swap "), "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l));
    for (p, i) in [("spare", e.t), ("spare2", s2), ("third", th)] {
        assert_eq!(ino_at(&e.slot(p)), Some(i), "{p} was not touched");
    }
}

// --- Test 9: a stay or an ALERT with unrelated churn ------------------------

fn stay_while_third_parks(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0)), ("third", spent())]);
    park(ctx.store.root(), "third");
    let m = decide_measured(ctx, snap, ok(10.0, 10.0), probes, at)?;
    assert_eq!(m.decision, Decision::Stay, "the fixture must decide a stay");
    Ok(m)
}

fn alert_while_third_parks(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(
        snap,
        &[("spare", spent()), ("spare2", spent()), ("third", spent())],
    );
    park(ctx.store.root(), "third");
    let m = decide_measured(ctx, snap, spent(), probes, at)?;
    assert_eq!(
        m.decision,
        Decision::Alert,
        "the fixture must decide an ALERT"
    );
    Ok(m)
}

#[cfg(target_os = "linux")]
#[test]
fn a_stay_is_not_cancelled_by_an_unrelated_profile() {
    let (e, _, _) = env3();
    let mut ctx = ctx_with(&e, stay_while_third_parks);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Stayed, "{log}");
    assert!(!log.contains(msg::CHANGED), "{log}");
    assert_eq!(last_outcome(&e), "stayed");
}

#[cfg(target_os = "linux")]
#[test]
fn an_alert_is_not_cancelled_by_an_unrelated_profile() {
    let (e, _, _) = env3();
    let mut ctx = ctx_with(&e, alert_while_third_parks);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Alerted, "{log}");
    assert!(!log.contains(msg::CHANGED), "{log}");
    assert!(
        log.contains("ALERT: no profile has capacity live=work ")
            && log.contains(" (candidates: 0 failed, 3 exhausted)"),
        "{log}"
    );
}

// --- Security: the retry never moves a login the gates now refuse ------------

static SPARE_SESSION_R1: AtomicBool = AtomicBool::new(false);

/// `spare` gets a session and `spare2`'s login loses OAuth (#1700), so the
/// only other probed profile is now `NotOAuth`.
fn spare_busy_spare2_not_oauth(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 10.0)), ("spare2", ok(0.0, 43.0))]);
    SPARE_SESSION_R1.store(true, Ordering::SeqCst);
    let slot = ctx.store.root().join("spare2").join(CREDS);
    fs::remove_file(&slot).unwrap();
    private(&slot, "{}");
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn the_retry_never_swaps_to_a_profile_that_lost_oauth() {
    let (e, _, _) = env3();
    let mut ctx = ctx_with(&e, spare_busy_spare2_not_oauth);
    ctx.scan = |_| sessions(&SPARE_SESSION_R1, &["spare"]);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "nothing was moved");
    assert_no_retry(&log);
    assert!(!log.contains("swap "), "{log}");
}

static SPARE_SESSION_F1: AtomicBool = AtomicBool::new(false);
static SPARE_SESSION_F2: AtomicBool = AtomicBool::new(false);

fn live_failed(flag: &AtomicBool, ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 10.0)), ("spare2", ok(0.0, 43.0))]);
    flag.store(true, Ordering::SeqCst);
    let m = decide_measured(ctx, snap, Probed::failed(), probes, at)?;
    assert!(
        matches!(m.decision, Decision::Swap { .. }),
        "the fixture must decide a failed-probe swap: {:?}",
        m.decision
    );
    Ok(m)
}

#[cfg(target_os = "linux")]
#[test]
fn a_skipped_retry_counts_the_failed_live_probe_once() {
    let e = env();
    let mut ctx = ctx_with(&e, |c, s| live_failed(&SPARE_SESSION_F1, c, s));
    ctx.scan = |_| sessions(&SPARE_SESSION_F1, &["spare"]);
    ctx.failed_live = FAILED_PROBE_LIMIT - 1;
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    assert_eq!(outcome, CycleOutcome::Skipped, "{}", log_of(&e));
    assert_eq!(
        ctx.failed_live, FAILED_PROBE_LIMIT,
        "counted once, not twice"
    );
    e.assert_unchanged();
}

#[cfg(target_os = "linux")]
#[test]
fn a_retry_after_failed_live_probes_still_swaps() {
    let (e, s2, _) = env3();
    let mut ctx = ctx_with(&e, |c, s| live_failed(&SPARE_SESSION_F2, c, s));
    ctx.scan = |_| sessions(&SPARE_SESSION_F2, &["spare"]);
    ctx.failed_live = FAILED_PROBE_LIMIT - 1;
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(
        outcome,
        CycleOutcome::Swapped,
        "the re-decision starts from the count before this cycle, so it \
         reaches the limit again rather than damping to a stay:\n{log}"
    );
    assert_eq!(ino_at(&e.live()), Some(s2));
    assert_eq!(ctx.failed_live, 0, "a swap resets the count");
}

// --- D8: a held cycle's spend-limit check ignores unrelated churn -----------

fn never(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    panic!("a held cycle must not measure")
}

fn spent_while_third_parks(
    ctx: &Ctx,
    _: &crate::commands::profile::name::ProfileName,
) -> Result<bool, ProfileError> {
    park(ctx.store.root(), "third");
    Ok(true)
}

#[cfg(target_os = "linux")]
#[test]
fn a_pinned_spend_limit_is_blocked_despite_an_unrelated_profile_parking() {
    let (e, _, _) = env3();
    config::update(&e.root, ReadMode::Editable, |c| {
        c.overflow.push(OverflowEntry::new(n("spare"), 1));
        Ok(())
    })
    .unwrap();
    match switch_live(&e.store, &e.home, &n("spare")) {
        Ok(Ok(Switched::Moved { .. })) => {}
        other => panic!("switch to spare failed: {other:?}"),
    }
    let mut ctx = ctx_with(&e, never);
    ctx.probe_held = Some(spent_while_third_parks);
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Held
    );
    let log = log_of(&e);
    assert!(!log.contains(msg::CHANGED), "{log}");
    let ledger = json(&e.rs.ledger);
    assert_eq!(
        ledger["overflow_blocked"]["spare"]["reason"], "spend_limit",
        "{ledger}"
    );
}

// --- Target identity: the target holds another account (#1721 review) -------

/// Gives `name`'s `.claude.json` the account `uuid`, keeping its other keys,
/// as `profile login` or a replaced profile directory would.
fn relog(root: &Path, name: &str, uuid: &str) {
    let path = root.join(name).join(".claude.json");
    let mut v = json(&path);
    v["oauthAccount"] =
        serde_json::json!({"accountUuid": uuid, "emailAddress": format!("{uuid}@example.com")});
    private(&path, &v.to_string());
}

fn spare_relogged(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0))]);
    relog(ctx.store.root(), "spare", "u-intruder");
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_target_logged_in_to_another_account_during_measurement_is_not_swapped_in() {
    let e = env();
    let mut ctx = ctx_with(&e, spare_relogged);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(changed_labels(&log), ["target identity"]);
    assert!(!log.contains("intruder"), "no account in the log:\n{log}");
    assert_no_retry(&log);
    e.assert_unchanged();
    assert_eq!(last_outcome(&e), "skipped");
}

/// A probe may rewrite the target's `.claude.json` with the same account
/// (a counter, a refreshed token elsewhere): a new file, not a new account.
fn spare_rewritten_same_account(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0))]);
    let path = ctx.store.root().join("spare").join(".claude.json");
    let mut v = json(&path);
    v["numStartups"] = serde_json::json!(4);
    fs::remove_file(&path).unwrap();
    private(&path, &v.to_string());
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_target_whose_claude_json_is_rewritten_with_the_same_account_still_swaps() {
    let e = env();
    let mut ctx = ctx_with(&e, spare_rewritten_same_account);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    assert!(!log.contains(msg::CHANGED), "{log}");
}

fn spare_relogged_spare2_kept(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(
        snap,
        &[
            ("spare", ok(0.0, 10.0)),
            ("spare2", ok(0.0, 43.0)),
            ("third", spent()),
        ],
    );
    relog(ctx.store.root(), "spare", "u-intruder");
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_relogged_target_is_reconsidered_to_a_candidate_whose_account_is_unchanged() {
    let (e, s2, _) = env3();
    let mut ctx = ctx_with(&e, spare_relogged_spare2_kept);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Swapped, "{log}");
    let line = "revalidate: target spare no longer eligible (account changed); reconsidered";
    assert!(log.contains(line), "{log}");
    assert!(log.contains("swap work->spare2 "), "{log}");
    assert_eq!(ino_at(&e.live()), Some(s2), "spare2's login is live");
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t), "spare was not touched");
}

/// `spare` and `spare2` are both logged in to other accounts; `third` is a
/// candidate but was never probed.
fn both_relogged(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 10.0)), ("spare2", ok(0.0, 43.0))]);
    relog(ctx.store.root(), "spare", "u-intruder");
    relog(ctx.store.root(), "spare2", "u-intruder2");
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn the_retry_never_swaps_to_a_candidate_whose_account_changed_too() {
    let (e, s2, th) = env3();
    let mut ctx = ctx_with(&e, both_relogged);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(changed_labels(&log), ["target identity"]);
    assert_no_retry(&log);
    assert_eq!(ino_at(&e.live()), Some(e.l));
    for (p, i) in [("spare", e.t), ("spare2", s2), ("third", th)] {
        assert_eq!(ino_at(&e.slot(p)), Some(i), "{p} was not touched");
    }
}

// --- An unreadable `~/.claude.json` is read once more (#1721 review) --------

/// Moves `~/.claude.json` aside and leaves a cut-short copy in its place,
/// which is what a read during a non-atomic rewrite finds.
fn cut_home_short(home: &Path) {
    fs::rename(home.join(".claude.json"), home.join("claude.json.aside")).unwrap();
    private(&home.join(".claude.json"), "{");
}

fn put_home_back(home: &Path) {
    fs::rename(home.join("claude.json.aside"), home.join(".claude.json")).unwrap();
}

fn home_cut_short(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0))]);
    cut_home_short(&ctx.home);
    decide_measured(ctx, snap, spent(), probes, at)
}

fn never_paused(_: &Ctx) {
    panic!("this change must not be read again")
}

/// The live profile `identify::live` reports right now.
fn live_now(ctx: &Ctx) -> LiveId {
    identify::live(&ctx.store, &ctx.rs, &ctx.home).unwrap().0
}

fn assert_read_again_then_swapped(e: &super::tests_swap::Env, log: &str) {
    assert_eq!(log.matches(REREAD).count(), 1, "one reread:\n{log}");
    assert!(log.contains("swap work->spare "), "{log}");
    assert!(
        log.find(REREAD) < log.find("swap work->spare "),
        "the reread line comes before the swap line:\n{log}"
    );
    assert!(!log.contains(msg::CHANGED), "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
}

#[cfg(target_os = "linux")]
#[test]
fn a_home_cut_short_during_measurement_is_read_again_and_the_swap_goes_ahead() {
    // The steady state: the ledger names `work` and its account, so the cut
    // file reads as `HomeUnreadable`.
    let e = env();
    let ledger = RotateLedger {
        live: Some("work".into()),
        account: Some("u-work".into()),
        ..RotateLedger::default()
    };
    e.rs.save_ledger(&ledger).unwrap();
    let mut ctx = ctx_with(&e, home_cut_short);
    ctx.pause = |ctx| {
        assert_eq!(live_now(ctx), LiveId::HomeUnreadable(n("work")));
        put_home_back(&ctx.home);
    };
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Swapped, "{log}");
    assert_read_again_then_swapped(&e, &log);
}

#[cfg(target_os = "linux")]
#[test]
fn a_home_cut_short_before_any_ledger_is_read_again_too() {
    // No ledger yet: the cut file reads as a login to adopt.
    let e = env();
    let mut ctx = ctx_with(&e, home_cut_short);
    ctx.pause = |ctx| {
        assert_eq!(live_now(ctx), LiveId::Adopt);
        put_home_back(&ctx.home);
    };
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Swapped, "{log}");
    assert_read_again_then_swapped(&e, &log);
}

static PAUSES_STAYS_CUT: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "linux")]
#[test]
fn a_home_that_stays_cut_short_is_read_once_more_then_skips() {
    let e = env();
    let mut ctx = ctx_with(&e, home_cut_short);
    ctx.pause = |_| {
        PAUSES_STAYS_CUT.fetch_add(1, Ordering::SeqCst);
    };
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(PAUSES_STAYS_CUT.load(Ordering::SeqCst), 1, "one pause");
    assert_eq!(log.matches(REREAD).count(), 1, "{log}");
    assert_eq!(changed_labels(&log), ["live"]);
    assert_eq!(ino_at(&e.live()), Some(e.l), "nothing was moved");
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert_eq!(last_outcome(&e), "skipped");
}

fn home_to_another_account(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0))]);
    let path = ctx.home.join(".claude.json");
    let mut v = json(&path);
    v["oauthAccount"] = serde_json::json!({"accountUuid": "u-other"});
    private(&path, &v.to_string());
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn a_home_with_another_account_is_not_read_again() {
    let e = env();
    let mut ctx = ctx_with(&e, home_to_another_account);
    ctx.pause = never_paused;
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(changed_labels(&log), ["live"]);
    assert!(!log.contains(REREAD), "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "nothing was moved");
}

fn home_cut_short_and_journal(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let m = journal_appears(ctx, snap)?;
    cut_home_short(&ctx.home);
    Ok(m)
}

#[cfg(target_os = "linux")]
#[test]
fn a_journal_is_never_read_again_even_with_a_home_cut_short() {
    let e = env();
    let mut ctx = ctx_with(&e, home_cut_short_and_journal);
    ctx.pause = never_paused;
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Skipped, "{log}");
    assert_eq!(changed_labels(&log), ["journal", "live"]);
    assert!(!log.contains(REREAD), "{log}");
}

static SPARE_SESSION_RR: AtomicBool = AtomicBool::new(false);

fn home_cut_short_and_spare_busy(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 10.0)), ("spare2", ok(0.0, 43.0))]);
    SPARE_SESSION_RR.store(true, Ordering::SeqCst);
    cut_home_short(&ctx.home);
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn after_a_reread_a_target_change_is_still_reconsidered_once() {
    let (e, s2, _) = env3();
    let mut ctx = ctx_with(&e, home_cut_short_and_spare_busy);
    ctx.scan = |_| sessions(&SPARE_SESSION_RR, &["spare"]);
    ctx.pause = |ctx| put_home_back(&ctx.home);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Swapped, "{log}");
    assert_eq!(log.matches(REREAD).count(), 1, "{log}");
    let line = "revalidate: target spare no longer eligible (session running); reconsidered";
    assert!(log.contains(line), "{log}");
    assert!(log.find(REREAD) < log.find(line), "{log}");
    assert_eq!(ino_at(&e.live()), Some(s2), "spare2's login is live");
}
