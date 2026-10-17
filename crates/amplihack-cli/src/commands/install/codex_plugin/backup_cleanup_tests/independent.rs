//! Each case selects exactly one existing guarded scenario.
//!
//! Every case holds the crate's environment lock, as the looped originals do.
//! `recover_install` rejects a working directory that changed under it, and
//! tests that hold the lock move the process cwd, so an unlocked case failed
//! with "Codex invocation working directory changed" in a full `--lib` run.
use super::*;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

#[test]
fn independent_file() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("file");
}

#[test]
fn independent_injected() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("injected");
}

#[test]
fn independent_type() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("type");
}

#[test]
fn independent_path() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("path");
}

#[test]
fn independent_duplicate() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("duplicate");
}

#[test]
fn independent_transaction() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case(
        "transaction",
    );
}

#[test]
fn independent_home() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("home");
}

#[test]
fn independent_digest() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("digest");
}

#[test]
fn independent_shape() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("shape");
}

#[test]
fn independent_schema() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("schema");
}

#[test]
fn independent_null() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("null");
}

#[test]
fn independent_absolute() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("absolute");
}

#[test]
fn independent_hash() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("hash");
}

#[test]
fn independent_parent() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("parent");
}

#[test]
fn independent_ledger() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("ledger");
}

#[test]
fn independent_live() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("live");
}

#[test]
fn independent_hooks() {
    let _env_lock = env_lock();
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("hooks");
}

#[test]
fn independent_full_backup() {
    let _env_lock = env_lock();
    cases::partial_backup_case(false);
}

#[test]
fn independent_proof_free_partial_backup() {
    let _env_lock = env_lock();
    cases::partial_backup_case(true);
}

#[cfg(unix)]
#[test]
fn independent_backup_link_original() {
    let _env_lock = env_lock();
    cases::backup_link_case(false);
}

#[cfg(unix)]
#[test]
fn independent_backup_link_substituted() {
    let _env_lock = env_lock();
    cases::backup_link_case(true);
}
