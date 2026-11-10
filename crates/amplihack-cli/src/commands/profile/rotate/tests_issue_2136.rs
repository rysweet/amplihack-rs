//! Behavioural red tests for #2136. Only temporary profiles; measurement is
//! directly injected. No CLI, network, process scan, timer or real HOME.
use super::super::cycle::{Ctx, MeasureFn, Mode, run_cycle};
use super::super::observe::{Measurement, Snap, decide_measured};
use super::super::tests_revalidate_cause::{add_stored, ctx_with, log_of, ok, probes_for};
use super::super::tests_swap::{CREDS, Env, env, ino_at, login, private};
use crate::commands::profile::ProfileError;
use serde_json::{Value, json};
use std::fs;

const AT: i64 = 1_792_592_000;

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
    decide_measured(ctx, snap, ok(live, live), probes, AT)
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
    decide_measured(ctx, snap, ok(85.0, 90.0), probes, AT)
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
fn upgrade_never_crosses_or_treats_unknown_as_same() {
    for org in [json!("org-b"), Value::Null] {
        let e = fixture(json!("org-a"), org.clone(), org);
        let before = ino_at(&e.home.join(".claude").join(CREDS));
        run(&e, upgrade);
        assert_eq!(
            ino_at(&e.home.join(".claude").join(CREDS)),
            before,
            "{}",
            log_of(&e)
        );
    }
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
fn plan_room_and_its_fallback_require_confirmed_affinity() {
    for org in [json!("org-b"), Value::Null] {
        let e = fixture(json!("org-a"), org.clone(), org);
        let before = ino_at(&e.home.join(".claude").join(CREDS));
        run(&e, plan_room);
        assert_eq!(
            ino_at(&e.home.join(".claude").join(CREDS)),
            before,
            "{}",
            log_of(&e)
        );
    }
}

#[test]
fn unknown_source_cannot_upgrade_even_to_another_unknown() {
    let e = fixture(Value::Null, Value::Null, Value::Null);
    let before = ino_at(&e.home.join(".claude").join(CREDS));
    run(&e, upgrade);
    assert_eq!(ino_at(&e.home.join(".claude").join(CREDS)), before);
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
    decide_measured(ctx, snap, ok(100.0, 100.0), probes, AT)
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
