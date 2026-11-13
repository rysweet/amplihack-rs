//! #2029 attribution contract. Payload claims and current credential location
//! cannot replace independently captured session identity and generation.
#![cfg(unix)]

use amplihack_utils::session_attribution::{
    AttributionError, AttributionSnapshot, LeaseBinding, SessionBinding, resolve,
};
use amplihack_utils::usage_observation::{Binding, ObservationStore};
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn bindings() -> (tempfile::TempDir, Binding, Binding) {
    let tmp = tempfile::tempdir_in(fs::canonicalize("/tmp").unwrap()).unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    for profile in ["work", "spare"] {
        let dir = tmp.path().join(profile);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = ObservationStore::open(tmp.path()).unwrap();
    let work = store
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    let spare = store
        .lock()
        .unwrap()
        .authorised_login("spare", "account-spare")
        .unwrap();
    (tmp, work, spare)
}

fn context(work: &Binding, spare: &Binding) -> AttributionSnapshot {
    AttributionSnapshot {
        login_bindings: vec![work.clone(), spare.clone()],
        sessions: vec![SessionBinding {
            session_id: "session-work".into(),
            binding: work.clone(),
        }],
        leases: Vec::new(),
        live: Some(work.clone()),
    }
}

#[test]
fn captured_home_session_keeps_its_identity_after_another_login_becomes_live() {
    let (_tmp, work, spare) = bindings();
    let mut trusted = context(&work, &spare);
    trusted.live = Some(spare);
    assert_eq!(
        resolve(&json!({"session_id":"session-work"}), &trusted).unwrap(),
        work
    );
}

#[test]
fn hosted_session_publishes_for_its_captured_account_instead_of_the_host() {
    let (_tmp, work, spare) = bindings();
    let mut trusted = context(&work, &spare);
    trusted.leases.push(LeaseBinding {
        session_id: "session-work".into(),
        host_profile: "spare".into(),
        binding: work.clone(),
    });
    assert_eq!(
        resolve(&json!({"session_id":"session-work"}), &trusted).unwrap(),
        work
    );
}

#[test]
fn untrusted_payload_identity_alone_cannot_attribute_an_observation() {
    let (_tmp, work, spare) = bindings();
    let mut trusted = context(&work, &spare);
    trusted.sessions.clear();
    let payload = json!({"session_id":"unknown", "profile":work.profile, "account_id":work.account_id, "generation":work.generation});
    assert_eq!(
        resolve(&payload, &trusted),
        Err(AttributionError::MissingContext)
    );
}

#[test]
fn conflicting_trusted_session_and_lease_bindings_are_ambiguous() {
    let (_tmp, work, spare) = bindings();
    let mut trusted = context(&work, &spare);
    trusted.leases.push(LeaseBinding {
        session_id: "session-work".into(),
        host_profile: "work".into(),
        binding: spare,
    });
    assert_eq!(
        resolve(&json!({"session_id":"session-work"}), &trusted),
        Err(AttributionError::Ambiguous)
    );
}

#[test]
fn relogin_invalidates_the_generation_captured_by_an_existing_session() {
    let (tmp, work, spare) = bindings();
    let mut trusted = context(&work, &spare);
    let store = ObservationStore::open(tmp.path()).unwrap();
    trusted.login_bindings[0] = store
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    assert_eq!(
        resolve(&json!({"session_id":"session-work"}), &trusted),
        Err(AttributionError::RetiredGeneration)
    );
}

#[test]
fn forged_claims_cannot_redirect_an_attributed_session_to_another_profile() {
    let (_tmp, work, spare) = bindings();
    let trusted = context(&work, &spare);
    let payload = json!({"session_id":"session-work", "profile":"spare", "account_id":spare.account_id, "generation":spare.generation});
    assert_eq!(
        resolve(&payload, &trusted),
        Err(AttributionError::InvalidClaim)
    );
}

#[test]
fn relogin_does_not_rebind_an_existing_session_identifier_to_the_new_generation() {
    let (tmp, work, _) = bindings();
    let store = ObservationStore::open(tmp.path()).unwrap();
    let mut guard = store.lock().unwrap();
    guard.bind_session("persistent-session", &work).unwrap();
    guard.retire(&work).unwrap();
    let replacement = guard.authorised_login("work", "account-work").unwrap();
    assert!(
        guard
            .bind_session("persistent-session", &replacement)
            .is_err()
    );
    let trusted = AttributionSnapshot {
        login_bindings: guard.bindings().unwrap(),
        sessions: guard.sessions().unwrap(),
        ..Default::default()
    };
    assert_eq!(
        resolve(&json!({"session_id":"persistent-session"}), &trusted),
        Err(AttributionError::RetiredGeneration)
    );
}
