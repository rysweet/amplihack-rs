//! Each case selects exactly one existing guarded scenario.
use super::*;

#[test]
fn independent_home() {
    rollback_rejects_foreign_directory_links_before_native_or_restore_case("home");
}

#[test]
fn independent_market() {
    rollback_rejects_foreign_directory_links_before_native_or_restore_case("market");
}

#[test]
fn independent_agents() {
    rollback_rejects_foreign_directory_links_before_native_or_restore_case("agents");
}

#[test]
fn independent_ancestor() {
    rollback_rejects_foreign_directory_links_before_native_or_restore_case("ancestor");
}
