//! Public v1 contracts. Production filesystem behavior is exercised by the
//! process-level Python suite; no proposed v2 API is required here.
use lbug_cache::{ARCHIVES, digest, selection_env};
use std::collections::BTreeMap;

#[test]
fn static_inventory_is_complete_and_excludes_the_shim() {
    assert_eq!(ARCHIVES.len(), 18);
    assert!(ARCHIVES.contains(&"lbug"));
    assert!(!ARCHIVES.contains(&"lbug_rs"));
}

#[test]
fn warm_selection_stays_stable_but_rejection_invalidates_it() {
    let env = BTreeMap::from([("CXXFLAGS".into(), "-O2".into())]);
    let verified = selection_env(&env, "verified:payload");
    assert_eq!(verified, selection_env(&env, "verified:payload"));
    assert_ne!(verified, selection_env(&env, "source"));
    assert_eq!(
        digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
