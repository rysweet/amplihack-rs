//! Real publication boundaries for explicit collection and running sessions.
use super::*;
use amplihack_utils::usage_observation::{Binding, ReadOutcome};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn fixture() -> (
    tempfile::TempDir,
    ProfileStore,
    ObservationStore,
    Binding,
    PathBuf,
) {
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let root = tmp.path().join("profiles");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(root.join("work")).unwrap();
    fs::set_permissions(root.join("work"), fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.join("work/.credentials.json");
    fs::write(&path, b"opaque inode fixture").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let observations = ObservationStore::open(&root).unwrap();
    let binding = observations
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    (tmp, ProfileStore::new(root), observations, binding, path)
}

#[test]
fn an_explicit_probe_publishes_only_while_its_captured_location_is_unchanged() {
    for moved in [false, true] {
        let (_tmp, store, observations, binding, path) = fixture();
        let capture = capture(&store, &ProfileName::parse("work").unwrap(), false).unwrap();
        if moved {
            let destination = store.root().join("moved");
            fs::rename(&path, &destination).unwrap();
            observations
                .lock()
                .unwrap()
                .moved(&path, &destination)
                .unwrap();
        }
        let report = crate::commands::profile::usage_parse::parse_usage(
            "Current session: 10% used\nCurrent week (all models): 20% used",
            "",
            1_790_000_000,
        );
        let probe = Probe {
            report,
            error: None,
            raw: None,
            source: Some(Source::Fallback),
            direct_failure: None,
            spend: crate::commands::profile::usage_source::Spend::Unknown,
            extra_shape: None,
        };
        publish(store.root(), &capture, &probe);
        let outcome = observations.lock().unwrap().read(&binding).unwrap();
        assert_eq!(matches!(outcome, ReadOutcome::Usable(_)), !moved);
        if moved {
            assert_eq!(outcome, ReadOutcome::Missing);
        }
    }
}

#[test]
fn session_limit_evidence_keeps_its_original_owner_after_authorised_movement() {
    let (_tmp, store, observations, binding, path) = fixture();
    let capture = capture(&store, &ProfileName::parse("work").unwrap(), false).unwrap();
    let destination = store.root().join("moved");
    fs::rename(&path, &destination).unwrap();
    observations
        .lock()
        .unwrap()
        .moved(&path, &destination)
        .unwrap();
    let intervening = {
        let mut guard = observations.lock().unwrap();
        let fresh = guard.capture_new(&binding, 1_790_000_000).unwrap();
        guard
            .publish(
                &fresh,
                &Evidence {
                    session_window: amplihack_utils::usage_observation::SessionWindow::Present,
                    session_used: Some(10.0),
                    week_used: Some(20.0),
                    ..Default::default()
                },
            )
            .unwrap();
        fresh
    };
    publish_limit(store.root(), &capture);
    let ReadOutcome::Usable(record) = observations.lock().unwrap().read(&binding).unwrap() else {
        panic!("running session lost its logical owner");
    };
    assert_eq!(record.capture.binding, binding);
    assert!(record.evidence.limit_reached);
    assert!(
        record.capture.ticket > intervening.ticket,
        "a new limit event reused the session's older collection ticket"
    );
}
