use super::*;

#[test]
fn fallback_publishes_one_event_after_ownership_and_cycle_state_are_committed() {
    let e = fixture();
    fs::create_dir(e.root.join("spare/.oauth_refresh.lock")).unwrap();
    let mut c = context(&e);
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    let event = json(&e.rs.dir.join("swap.json"));
    assert_eq!(event["seq"], 1);
    assert_eq!(event["from_login"], "work");
    assert_eq!(event["live_login"], "extra");
    assert_eq!(event["reading"]["session"], 25);
    assert_eq!(event["reading"]["measured_at"], T);
    let ledger = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(ledger.live.as_deref(), Some("extra"));
    assert_eq!(ledger.last_cycle.unwrap().outcome, "swapped");
    assert!(!e.rs.journal.exists());
}

#[test]
fn publication_failure_keeps_recoverable_completion_and_replays_without_another_swap() {
    let e = fixture();
    let mut c = context(&e);
    // Deterministic write failure, including when the test runs as root.
    let event_path = e.rs.dir.join("swap.json");
    fs::create_dir(&event_path).unwrap();
    let _ = drive(&e, &mut c);
    assert_eq!(
        ino_at(&e.live()),
        Some(e.t),
        "placement completed before event write"
    );
    assert!(
        e.rs.journal.exists(),
        "publication evidence must survive a failed event write"
    );
    fs::remove_dir(&event_path).unwrap();
    advance(120);
    sample(5.0, 10.0, 0, T + 10_000);
    let _ = drive(&e, &mut c);
    let event = json(&event_path);
    assert_eq!(event["seq"], 1);
    assert_eq!(event["live_login"], "spare");
    assert_eq!(
        event["reading"]["measured_at"], T,
        "original observation provenance survives recovery"
    );
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert!(!e.rs.journal.exists());
    advance(120);
    let _ = drive(&e, &mut c);
    assert_eq!(json(&event_path), event, "recovery publishes exactly once");
}

#[test]
fn recovered_duplicate_swap_publishes_measured_values_once() {
    let e = fixture();
    let bytes = fs::read(e.live()).unwrap();
    private(&e.slot("work"), std::str::from_utf8(&bytes).unwrap());
    let mut c = context(&e);
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    let event = json(&e.rs.dir.join("swap.json"));
    assert_eq!(event["seq"], 1);
    assert_eq!(event["live_login"], "spare");
    assert_eq!(event["reading"]["session"], 5);
    assert_eq!(event["reading"]["week"], 10);
    assert_eq!(event["reading"]["measured_at"], T);
}

#[test]
fn each_completion_interruption_replays_the_original_event_exactly_once() {
    for point in ["s1", "s2", "s3", "s4", "swap-event-published"] {
        let e = fixture();
        let mut c = context(&e);
        test_exchange::interrupted(point, || {
            let _ = drive(&e, &mut c);
        });
        assert!(e.rs.journal.exists());
        advance(120);
        sample(5.0, 10.0, 0, T + 10_000);
        let _ = drive(&e, &mut c);
        assert_eq!(ino_at(&e.live()), Some(e.t), "{point}: {}", log(&e));
        let event = json(&e.rs.dir.join("swap.json"));
        assert_eq!(event["seq"], 1);
        assert_eq!(event["reading"]["measured_at"], T);
        assert!(!e.rs.journal.exists());
        advance(120);
        let _ = drive(&e, &mut c);
        assert_eq!(json(&e.rs.dir.join("swap.json")), event);
    }
}
