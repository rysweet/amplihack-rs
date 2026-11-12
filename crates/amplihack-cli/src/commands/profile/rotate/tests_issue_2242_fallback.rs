use super::*;

#[test]
fn an_api_key_refusal_preserves_its_actionable_alert_once_across_cycles() {
    let e = fixture();
    private(&e.slot("spare"), "{}");
    let key_inode = ino_at(&e.slot("spare"));
    private(
        &e.root.join(".auto-switch"),
        "enabled=off\noverflow=spare\nkind=spare:platform\n",
    );
    let mut c = context(&e);
    sample(100.0, 100.0, 0, T + 10_000);
    c.measure = Some(|ctx, snap| {
        let full = SAMPLE.with(|p| p.borrow().clone());
        let probes = snap
            .candidates
            .iter()
            .map(|name| (name.clone(), full.clone()))
            .collect();
        decide_measured(ctx, snap, full, probes, clock())
    });
    c.key_check = |_| Err("fake failure".into());
    for _ in 0..2 {
        assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    }
    let lines = log(&e);
    assert_eq!(
        lines
            .matches("key_check_refused spare: fake failure")
            .count(),
        1,
        "{lines}"
    );
    assert_eq!(
        lines
            .matches("ALERT: API key profile spare failed its login check")
            .count(),
        1,
        "{lines}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), key_inode);
    assert!(!e.rs.journal.exists());
    assert!(!e.rs.dir.join("swap.json").exists());
}
use std::os::unix::fs::PermissionsExt;

#[test]
fn safe_refusal_tries_next_ranked_candidate_without_measuring_again() {
    let e = fixture();
    let extra = ino_at(&e.slot("extra")).unwrap();
    fs::create_dir(e.root.join("spare/.oauth_refresh.lock")).unwrap();
    let mut c = context(&e);
    let result = drive(&e, &mut c);
    assert_eq!(result.unwrap(), CycleOutcome::Swapped, "{}", log(&e));
    assert_eq!(ino_at(&e.live()), Some(extra));
    assert_eq!(
        measurements(),
        1,
        "fallback reuses this cycle's observations"
    );
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    let log = log(&e);
    let refusal = log
        .lines()
        .position(|l| l.contains("spare") && l.contains("refus"));
    let completion = log.lines().position(|l| l.contains("swap work->extra"));
    assert!(refusal.is_some() && refusal < completion, "{log}");
}

#[test]
fn a_busy_candidate_does_not_count_as_a_different_account_refusal() {
    let e = fixture();
    private(
        &e.slot("work"),
        std::str::from_utf8(&fs::read(e.slot("extra")).unwrap()).unwrap(),
    );
    fs::create_dir(e.root.join("spare/.oauth_refresh.lock")).unwrap();
    let mut c = context(&e);
    assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert!(!e.rs.dir.join("swap.json").exists());
}

#[test]
fn exhausting_candidates_records_observations_and_a_new_cycle_retries() {
    let e = fixture();
    for name in ["spare", "extra"] {
        fs::create_dir(e.root.join(name).join(".oauth_refresh.lock")).unwrap();
    }
    let mut c = context(&e);
    assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    assert_eq!(ino_at(&e.live()), Some(e.l));
    let l = e.rs.load_ledger().unwrap().unwrap();
    let last = l
        .last_cycle
        .expect("a refusal must record the completed cycle");
    assert_eq!(last.measured_at, Some(T));
    assert!(
        log(&e)
            .lines()
            .any(|l| l.contains("extra") && l.contains("refus")),
        "{}",
        log(&e)
    );
    fs::remove_dir(e.root.join("spare/.oauth_refresh.lock")).unwrap();
    advance(120);
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    assert_eq!(
        measurements(),
        2,
        "one measurement per explicitly driven cycle"
    );
    assert_eq!(ino_at(&e.live()), Some(e.t));
}

#[test]
fn fallback_cannot_use_a_busy_profile_even_when_it_has_room() {
    let e = fixture();
    fs::create_dir(e.root.join("spare/.oauth_refresh.lock")).unwrap();
    let mut c = context(&e);
    c.scan = |_| {
        crate::commands::profile::auto_switch::kit::Liveness::Known(
            ["extra".to_owned()].into_iter().collect(),
        )
    };
    let extra = ino_at(&e.slot("extra"));
    assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("extra")), extra);
    assert_eq!(measurements(), 1);
}

#[test]
fn unsafe_target_is_refused_before_fallback_and_keeps_its_credential() {
    let e = fixture();
    let mut c = context(&e);
    let extra = ino_at(&e.slot("extra"));
    // Change at the measurement boundary, after candidate discovery.
    c.measure = Some(|c, snap| {
        let measured = measure(c, snap)?;
        fs::set_permissions(
            c.store.root().join("spare/.credentials.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        Ok(measured)
    });
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    assert_eq!(ino_at(&e.live()), extra);
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
}
