//! Each case selects exactly one existing guarded scenario.
use super::*;

#[test]
fn independent_home() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    rollback_rejects_foreign_directory_links_before_native_or_restore_case("home");
}

#[test]
fn independent_market() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    rollback_rejects_foreign_directory_links_before_native_or_restore_case("market");
}

#[test]
fn independent_agents() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    rollback_rejects_foreign_directory_links_before_native_or_restore_case("agents");
}

#[test]
fn independent_ancestor() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    rollback_rejects_foreign_directory_links_before_native_or_restore_case("ancestor");
}
