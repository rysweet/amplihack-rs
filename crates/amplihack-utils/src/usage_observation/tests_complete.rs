//! Complete-window validation must apply equally to publication and reads.
//! These tests use the real store and bounded parser, never a test validator.
use super::*;
use amplihack_utils::usage_observation::Record;
use serde_json::json;

#[test]
fn ordinary_partial_readings_are_refused_without_replacing_the_last_complete_record() {
    for missing_session in [false, true] {
        let f = Fixture::new();
        let old = f.capture("complete");
        f.publish(&old, &usage(10.0, 20.0));
        let before = fs::read(f.path()).unwrap();
        let mut partial = usage(30.0, 40.0);
        if missing_session {
            partial.session_used = None;
        } else {
            partial.week_used = None;
        }
        assert_eq!(
            f.publish(&f.capture("incomplete"), &partial),
            PublishOutcome::InvalidEvidence,
            "missing_session={missing_session}: missing windows cannot establish capacity"
        );
        assert_eq!(fs::read(f.path()).unwrap(), before);
        assert_eq!(
            f.store.lock().unwrap().accepted_ticket(&f.binding).unwrap(),
            Some(old.ticket),
            "a refused reading consumed the accepted ordering"
        );
    }
}

#[test]
fn bounded_reader_rejects_incomplete_ordinary_records() {
    let f = Fixture::new();
    for (session_used, week_used) in [(None, Some(10.0)), (Some(10.0), None)] {
        let record = Record {
            schema_version: 1,
            capture: f.capture(if session_used.is_none() {
                "week-only"
            } else {
                "session-only"
            }),
            evidence: Evidence {
                session_used,
                week_used,
                ..Evidence::default()
            },
            revision: 1,
        };
        assert_eq!(
            read_bounded_record(serde_json::to_vec(&record).unwrap().as_slice(), &f.binding)
                .unwrap(),
            ReadOutcome::Invalid,
            "a legacy partial record must not become a complete plan reading"
        );
    }
}

#[test]
fn explicitly_absent_session_window_round_trips_without_inventing_usage() {
    let f = Fixture::new();
    let evidence: Evidence = serde_json::from_value(json!({
        "session_window": "absent", "session_used": null, "session_reset_at": null,
        "week_used": 40.0, "week_reset_at": null, "limit_reached": false
    }))
    .expect("the evidence schema must distinguish absent and unknown session limits");
    assert!(matches!(
        f.publish(&f.capture("no-session-limit"), &evidence),
        PublishOutcome::Accepted { .. }
    ));
    let ReadOutcome::Usable(record) = f.read() else {
        panic!("explicitly absent session limit was not retained")
    };
    assert_eq!(record.evidence.session_used, None);
    assert_eq!(
        serde_json::to_value(record.evidence).unwrap()["session_window"],
        "absent"
    );
    let bytes = fs::read(f.path()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_ne!(
        value["schema_version"], 1,
        "extended evidence requires a new schema version"
    );
}

#[test]
fn unknown_session_availability_does_not_mean_no_session_limit() {
    let f = Fixture::new();
    for availability in ["unknown", "present"] {
        let evidence: Evidence = serde_json::from_value(json!({
            "session_window": availability, "session_used": null, "session_reset_at": null,
            "week_used": 40.0, "week_reset_at": null, "limit_reached": false
        }))
        .expect("recognised session-window state");
        assert_eq!(
            f.publish(&f.capture(availability), &evidence),
            PublishOutcome::InvalidEvidence,
            "{availability} session availability cannot establish capacity"
        );
    }
}

#[test]
fn a_limit_flag_does_not_excuse_contradictory_absent_window_fields() {
    let f = Fixture::new();
    for (index, (used, reset)) in [(Some(10.0), None), (None, Some(NOW))]
        .into_iter()
        .enumerate()
    {
        let evidence: Evidence = serde_json::from_value(json!({
            "session_window": "absent", "session_used": used, "session_reset_at": reset,
            "week_used": 40.0, "week_reset_at": null, "limit_reached": true
        }))
        .expect("shape validation is separate from evidence validation");
        assert_eq!(
            f.publish(&f.capture(&format!("contradiction-{index}")), &evidence),
            PublishOutcome::InvalidEvidence
        );
    }
}

#[test]
fn a_shared_path_probe_is_rejected_after_its_login_moves_even_with_the_same_generation() {
    let f = Fixture::new();
    let credentials = f.root.join("work/.credentials.json");
    // These synthetic bytes are only an inode fixture; no code reads them.
    fs::write(&credentials, b"synthetic fixture").unwrap();
    fs::set_permissions(&credentials, fs::Permissions::from_mode(0o600)).unwrap();
    let mut guard = f.store.lock().unwrap();
    guard.set_location(&f.binding, &credentials).unwrap();
    let location = guard
        .probe_location(&f.binding, &credentials)
        .unwrap()
        .unwrap();
    let destination = f.root.join("moved");
    fs::rename(&credentials, &destination).unwrap();
    guard.moved(&credentials, &destination).unwrap();
    assert_eq!(
        guard.binding_at(&destination).unwrap(),
        Some(f.binding.clone())
    );
    assert_eq!(guard.binding("work").unwrap(), Some(f.binding.clone()));
    assert!(
        !guard
            .probe_location_unchanged(&f.binding, &location)
            .unwrap()
    );
    assert!(
        guard
            .probe_location(&f.binding, &destination)
            .unwrap()
            .is_some()
    );
    fs::rename(&destination, &credentials).unwrap();
    guard.moved(&destination, &credentials).unwrap();
    assert!(
        !guard
            .probe_location_unchanged(&f.binding, &location)
            .unwrap(),
        "moving away and back cannot restore a shared-path probe's ownership"
    );
}

#[test]
fn status_evidence_distinguishes_explicit_absence_from_omission() {
    use amplihack_utils::session_attribution::status_evidence;
    use amplihack_utils::usage_observation::SessionWindow;
    let f = Fixture::new();
    let absent = status_evidence(&json!({"rate_limits":{
        "five_hour":null,"seven_day":{"used_percentage":40}
    }}))
    .unwrap();
    assert_eq!(absent.session_window, SessionWindow::Absent);
    assert!(matches!(
        f.publish(&f.capture("absent"), &absent),
        PublishOutcome::Accepted { .. }
    ));
    let omitted =
        status_evidence(&json!({"rate_limits":{"seven_day":{"used_percentage":40}}})).unwrap();
    assert_eq!(omitted.session_window, SessionWindow::Unknown);
    assert_eq!(
        f.publish(&f.capture("omitted"), &omitted),
        PublishOutcome::InvalidEvidence
    );
}

#[test]
fn an_existing_readable_profile_root_keeps_owner_only_observation_state() {
    let f = Fixture::new();
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o755)).unwrap();
    let store = ObservationStore::open(&f.root).unwrap();
    assert_eq!(
        fs::metadata(store.directory())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        store.lock().unwrap().binding("work").unwrap(),
        Some(f.binding)
    );
}

#[test]
fn malformed_status_resets_are_errors_instead_of_unknown_resets() {
    use amplihack_utils::session_attribution::status_evidence;
    for reset in [
        json!("not a timestamp"),
        json!(true),
        json!({"wrong":"shape"}),
    ] {
        assert!(
            status_evidence(&json!({"rate_limits":{
                "five_hour":{"used_percentage":10,"resets_at":reset},
                "seven_day":{"used_percentage":20}
            }}))
            .is_err()
        );
    }
}

#[test]
fn a_shared_path_probe_survives_its_own_authorised_refresh_without_relabelling() {
    use amplihack_utils::usage_observation::{capture_refresh, publish_refresh};
    let f = Fixture::new();
    let path = f.root.join("work/.credentials.json");
    fs::write(&path, b"opaque fixture").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut guard = f.store.lock().unwrap();
    guard.set_location(&f.binding, &path).unwrap();
    let location = guard.probe_location(&f.binding, &path).unwrap().unwrap();
    let receipt = capture_refresh(&path).unwrap();
    let replacement = f.root.join("work/replacement");
    fs::write(&replacement, b"opaque replacement fixture").unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(&replacement, &path).unwrap();
    publish_refresh(receipt).unwrap();
    assert_eq!(
        guard.reconcile_bindings().unwrap(),
        vec![(f.binding.clone(), None)]
    );
    assert!(
        guard
            .probe_location_unchanged(&f.binding, &location)
            .unwrap()
    );
    assert_eq!(guard.binding("work").unwrap(), Some(f.binding.clone()));
    let capture = guard.capture_new(&f.binding, NOW).unwrap();
    assert!(matches!(
        guard.publish(&capture, &usage(10.0, 20.0)).unwrap(),
        PublishOutcome::Accepted { .. }
    ));
}
