//! Exact independent selection for each foreign-state branch.
use super::*;

#[test]
fn independent_rollback_foreign_config() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_recovery_preserves_foreign_snapshots_packages_and_raw_journal_case(false, "config");
}

#[test]
fn independent_rollback_foreign_package() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_recovery_preserves_foreign_snapshots_packages_and_raw_journal_case(false, "package");
}

#[test]
fn independent_rollback_foreign_backup() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_recovery_preserves_foreign_snapshots_packages_and_raw_journal_case(false, "backup");
}

#[test]
fn independent_committed_foreign_config() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_recovery_preserves_foreign_snapshots_packages_and_raw_journal_case(true, "config");
}

#[test]
fn independent_committed_foreign_package() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_recovery_preserves_foreign_snapshots_packages_and_raw_journal_case(true, "package");
}

#[test]
fn independent_committed_foreign_backup() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_recovery_preserves_foreign_snapshots_packages_and_raw_journal_case(true, "backup");
}

#[test]
fn independent_owned_config_link() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_recovery_rejects_owned_directory_and_snapshot_links_without_target_changes_case("config");
}

#[test]
fn independent_owned_market_link() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_recovery_rejects_owned_directory_and_snapshot_links_without_target_changes_case("market");
}

#[test]
fn independent_exact_clone_alias_barrier_replacement() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_retarget_or_anchor_replacement_during_barrier_retains_both_journals_case("alias");
}

#[test]
fn independent_exact_clone_anchor_barrier_replacement() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_retarget_or_anchor_replacement_during_barrier_retains_both_journals_case("anchor");
}
