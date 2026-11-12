//! Behavioural red tests for #2136. Only temporary profiles; measurement is
//! directly injected. No CLI, network, process scan, timer or real HOME.
use super::super::cycle::{Ctx, MeasureFn, Mode, run_cycle};
use super::super::observe::{Measurement, Snap, decide_measured};
use super::super::tests_revalidate_cause::{add_stored, ctx_with, log_of, ok, probes_for};
use super::super::tests_swap::{CREDS, Env, env, ino_at, login, private};
use crate::commands::profile::ProfileError;
use serde_json::{Value, json};
use std::fs;

/// Measured as the cycle runs: a fixed past instant reads as stale.
fn at() -> i64 {
    crate::commands::profile::auto_switch::kit::now()
}

fn run(e: &Env, measure: MeasureFn) {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = crate::test_support::EnvGuard::set([
        ("HOME", e.home.to_str().unwrap()),
        ("AMPLIHACK_CLAUDE_PROFILE_ROOT", e.root.to_str().unwrap()),
    ]);
    run_cycle(&mut ctx_with(e, measure), Mode::Watch).unwrap();
}

fn metadata(e: &Env, name: &str, org: Value) {
    let body = json!({"oauthAccount": {
        "accountUuid": format!("u-{name}"),
        "emailAddress": format!("{name}@example.com"),
        "organizationUuid": org
    }})
    .to_string();
    private(&e.root.join(name).join(".claude.json"), &body);
    if name == "work" {
        private(&e.home.join(".claude.json"), &body);
        login(&e.home.join(".claude").join(CREDS));
    } else {
        login(&e.root.join(name).join(CREDS));
    }
}

fn fixture(source: Value, spare: Value, third: Value) -> Env {
    let e = env();
    add_stored(&e, "third");
    metadata(&e, "work", source);
    metadata(&e, "spare", spare);
    metadata(&e, "third", third);
    let mut ledger = e.rs.load_ledger().unwrap().unwrap();
    ledger.login_identity = Some(super::super::login_identity::Identity::new(
        &(super::super::cred_identity::DEFAULT_READ)(&e.home.join(".claude")).unwrap(),
    ));
    e.rs.save_ledger(&ledger).unwrap();
    e
}

fn measured(ctx: &Ctx, snap: &Snap, live: f64) -> Result<Measurement, ProfileError> {
    let probes = probes_for(snap, &[("spare", ok(40.0, 40.0)), ("third", ok(1.0, 1.0))]);
    decide_measured(ctx, snap, ok(live, live), probes, at())
}
fn threshold(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    measured(ctx, snap, 95.0)
}
fn upgrade(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    measured(ctx, snap, 60.0)
}
fn plan_room(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = probes_for(
        snap,
        &[("spare", ok(84.0, 89.0)), ("third", ok(84.0, 89.0))],
    );
    decide_measured(ctx, snap, ok(85.0, 90.0), probes, at())
}

#[test]
fn threshold_prefers_same_organisation_before_headroom() {
    let e = fixture(json!("org-a"), json!("org-a"), json!("org-b"));
    let wanted = ino_at(&e.root.join("spare").join(CREDS));
    run(&e, threshold);
    assert_eq!(
        ino_at(&e.home.join(".claude").join(CREDS)),
        wanted,
        "{}",
        log_of(&e)
    );
}

#[test]
fn same_organisation_candidates_keep_existing_headroom_ranking() {
    let e = fixture(json!("org-a"), json!("org-a"), json!("org-a"));
    let wanted = ino_at(&e.root.join("third").join(CREDS));
    run(&e, threshold);
    assert_eq!(ino_at(&e.home.join(".claude").join(CREDS)), wanted);
}

#[test]
fn upgrade_never_crosses() {
    let e = fixture(json!("org-a"), json!("org-b"), json!("org-b"));
    let before = ino_at(&e.home.join(".claude").join(CREDS));
    run(&e, upgrade);
    assert_eq!(
        ino_at(&e.home.join(".claude").join(CREDS)),
        before,
        "{}",
        log_of(&e)
    );
}

/// Unknown is not crossing: missing metadata does not stop an upgrade, and
/// the swap carries the "unresolved" warning.
#[test]
fn upgrade_onto_an_unknown_organisation_goes_ahead_with_a_risk_warning() {
    let e = fixture(json!("org-a"), Value::Null, Value::Null);
    let before = ino_at(&e.home.join(".claude").join(CREDS));
    run(&e, upgrade);
    let log = log_of(&e);
    assert_ne!(ino_at(&e.home.join(".claude").join(CREDS)), before, "{log}");
    assert!(log.contains("unresolved"), "{log}");
}

#[test]
fn same_organisation_plan_room_survives_an_empty_capacity_pool() {
    let e = fixture(json!("org-a"), json!("org-a"), json!("org-b"));
    let wanted = ino_at(&e.root.join("spare").join(CREDS));
    run(&e, plan_room);
    assert_eq!(
        ino_at(&e.home.join(".claude").join(CREDS)),
        wanted,
        "{}",
        log_of(&e)
    );
}

#[test]
fn plan_room_and_its_fallback_never_cross() {
    let e = fixture(json!("org-a"), json!("org-b"), json!("org-b"));
    let before = ino_at(&e.home.join(".claude").join(CREDS));
    run(&e, plan_room);
    assert_eq!(
        ino_at(&e.home.join(".claude").join(CREDS)),
        before,
        "{}",
        log_of(&e)
    );
}

#[test]
fn plan_room_onto_an_unknown_organisation_is_not_a_crossing() {
    let e = fixture(json!("org-a"), Value::Null, Value::Null);
    let before = ino_at(&e.home.join(".claude").join(CREDS));
    run(&e, plan_room);
    assert_ne!(
        ino_at(&e.home.join(".claude").join(CREDS)),
        before,
        "{}",
        log_of(&e)
    );
}

#[test]
fn unknown_source_upgrades_like_before_organisation_affinity() {
    let e = fixture(Value::Null, Value::Null, Value::Null);
    let before = ino_at(&e.home.join(".claude").join(CREDS));
    run(&e, upgrade);
    assert_ne!(
        ino_at(&e.home.join(".claude").join(CREDS)),
        before,
        "{}",
        log_of(&e)
    );
}

fn mark_overflow(e: &Env, name: &str) {
    use crate::commands::profile::auto_switch::config::{self, ReadMode};
    config::update(&e.root, ReadMode::Editable, |c| {
        c.overflow
            .push(crate::commands::profile::overflow::OverflowEntry::new(
                super::super::tests_swap::n(name),
                1,
            ));
        Ok(())
    })
    .unwrap();
}

/// The live overflow login has headroom; a plan login has capacity again.
fn plan_recovers(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    measured(ctx, snap, 10.0)
}

/// Crusty round 1 on #2295: a session on a paid overflow login whose
/// organisation was never recorded must still go back to a plan login once
/// one recovers. Before the fix it stayed on overflow until blocked.
#[test]
fn unknown_organisation_on_overflow_returns_to_the_recovered_plan_login() {
    for plan_org in [Value::Null, json!("org-a")] {
        let e = fixture(Value::Null, plan_org.clone(), plan_org);
        mark_overflow(&e, "work");
        let plan = [
            ino_at(&e.root.join("spare").join(CREDS)),
            ino_at(&e.root.join("third").join(CREDS)),
        ];
        run(&e, plan_recovers);
        let after = ino_at(&e.home.join(".claude").join(CREDS));
        assert!(
            plan.contains(&after),
            "a plan login is live: {}",
            log_of(&e)
        );
    }
}

/// Runs one cycle and names the profile whose login is live afterwards,
/// from each stored login's inode before the cycle (a swap moves files).
fn live_after(e: &Env, measure: MeasureFn) -> &'static str {
    let before: Vec<_> = ["work", "spare", "third"]
        .into_iter()
        .map(|p| {
            let path = if p == "work" {
                e.home.join(".claude").join(CREDS)
            } else {
                e.root.join(p).join(CREDS)
            };
            (p, ino_at(&path))
        })
        .collect();
    run(e, measure);
    let live = ino_at(&e.home.join(".claude").join(CREDS));
    before
        .into_iter()
        .find(|(_, ino)| *ino == live)
        .map_or("none", |(p, _)| p)
}

fn swap_affinity(e: &Env) -> Value {
    let v: Value = serde_json::from_slice(&fs::read(e.rs.dir.join("swap.json")).unwrap()).unwrap();
    v["affinity"].clone()
}

/// Crusty round 7 on #2295: leaving a paid overflow login is cost relief,
/// so with the overflow organisation recorded and no same-organisation plan
/// login with capacity, the session crosses back to a recovered plan login
/// rather than paying until the overflow login is blocked.
#[test]
fn known_organisation_overflow_crosses_back_to_a_recovered_plan_login() {
    let e = fixture(json!("org-b"), json!("org-a"), json!("org-a"));
    mark_overflow(&e, "work");
    let live = live_after(&e, plan_recovers);
    let log = log_of(&e);
    assert!(live == "spare" || live == "third", "{log}");
    assert_eq!(swap_affinity(&e), "crossing", "{log}");
}

/// The crossing is a last resort: a same-organisation plan login with
/// capacity is taken even when a cross-organisation one has more headroom.
#[test]
fn overflow_exit_prefers_a_same_organisation_plan_login() {
    // spare (org-b) at 40%, third (org-a) at 1%.
    let e = fixture(json!("org-b"), json!("org-b"), json!("org-a"));
    mark_overflow(&e, "work");
    assert_eq!(live_after(&e, plan_recovers), "spare", "{}", log_of(&e));
}

/// Crusty round 7 on #2295, minor: a blocked overflow login whose only way
/// off is a cross-organisation plan login with room (no strict capacity)
/// moves there; it does not alert and stay on the blocked login.
#[test]
fn blocked_overflow_crosses_to_a_plan_login_with_room() {
    for blocked in [false, true] {
        let e = fixture(json!("org-b"), json!("org-a"), json!("org-a"));
        mark_overflow(&e, "work");
        if blocked {
            let mut ledger = e.rs.load_ledger().unwrap().unwrap();
            ledger.overflow_blocked.insert(
                "work".into(),
                super::super::ledger::OverflowBlock::spend_limit(at()),
            );
            e.rs.save_ledger(&ledger).unwrap();
        }
        let live = live_after(&e, plan_room);
        let log = log_of(&e);
        assert!(
            live == "spare" || live == "third",
            "blocked={blocked}: {log}"
        );
        assert_eq!(swap_affinity(&e), "crossing", "blocked={blocked}: {log}");
    }
}

#[test]
fn necessary_crossing_records_both_organisations_and_warns() {
    let e = fixture(json!("org-a"), json!("org-b"), json!("org-b"));
    run(&e, threshold);
    let v: Value = serde_json::from_slice(&fs::read(e.rs.dir.join("swap.json")).unwrap()).unwrap();
    assert_eq!(v["from_org"], "org-a");
    assert_eq!(v["to_org"], "org-b");
    assert_eq!(v["affinity"], "crossing");
    let log = log_of(&e);
    assert!(
        log.contains("earlier thinking") && log.contains("next turn"),
        "{log}"
    );
}

#[test]
fn necessary_unknown_swap_reports_uncertainty_not_a_confirmed_crossing() {
    let e = fixture(json!("org-a"), Value::Null, Value::Null);
    run(&e, threshold);
    let v: Value = serde_json::from_slice(&fs::read(e.rs.dir.join("swap.json")).unwrap()).unwrap();
    assert_eq!(v["affinity"], "unresolved");
    let log = log_of(&e);
    assert!(
        log.contains("unresolved") && log.contains("earlier thinking"),
        "{log}"
    );
}

fn org_changes_after_measurement(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let measured = measured(ctx, snap, 60.0)?;
    // Only metadata changes: the fake login remains the same inode/identity.
    let state = crate::commands::profile::auto_switch::kit::State::new(ctx.store.root())?;
    state.write_sub_json(
        "expected",
        "third",
        &json!({"account":"u-third", "org":"org-b"}),
    )?;
    private(&ctx.store.root().join("third/.claude.json"), &json!({
        "oauthAccount":{"accountUuid":"u-third", "emailAddress":"third@example.com", "organizationUuid":"org-b"}
    }).to_string());
    Ok(measured)
}

#[test]
fn affinity_is_revalidated_before_completing_an_upgrade() {
    let e = fixture(json!("org-a"), json!("org-a"), json!("org-a"));
    let before = ino_at(&e.home.join(".claude").join(CREDS));
    run(&e, org_changes_after_measurement);
    assert_eq!(
        ino_at(&e.home.join(".claude").join(CREDS)),
        before,
        "{}",
        log_of(&e)
    );
}

#[test]
fn same_org_swap_records_explicit_affinity_without_a_thinking_loss_warning() {
    let e = fixture(json!("org-a"), json!("org-a"), json!("org-a"));
    run(&e, threshold);
    let v: Value = serde_json::from_slice(&fs::read(e.rs.dir.join("swap.json")).unwrap()).unwrap();
    assert_eq!(v["affinity"], "same");
    assert!(!log_of(&e).contains("earlier thinking"));
}

fn threshold_boundary(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = probes_for(
        snap,
        &[("spare", ok(95.0, 95.0)), ("third", ok(10.0, 10.0))],
    );
    decide_measured(ctx, snap, ok(100.0, 100.0), probes, at())
}

#[test]
fn issue_2136_same_org_room_above_threshold_does_not_hide_relief() {
    let e = fixture(json!("org-a"), json!("org-a"), json!("org-b"));
    let wanted = ino_at(&e.root.join("third").join(CREDS));
    run(&e, threshold_boundary);
    assert_eq!(
        ino_at(&e.home.join(".claude").join(CREDS)),
        wanted,
        "{}",
        log_of(&e)
    );
}
