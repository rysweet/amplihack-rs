//! Each case selects exactly one existing guarded scenario.
use super::*;

#[test]
fn independent_file() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("file");
}

#[test]
fn independent_injected() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("injected");
}

#[test]
fn independent_type() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("type");
}

#[test]
fn independent_path() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("path");
}

#[test]
fn independent_duplicate() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("duplicate");
}

#[test]
fn independent_transaction() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case(
        "transaction",
    );
}

#[test]
fn independent_home() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("home");
}

#[test]
fn independent_digest() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("digest");
}

#[test]
fn independent_shape() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("shape");
}

#[test]
fn independent_schema() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("schema");
}

#[test]
fn independent_null() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("null");
}

#[test]
fn independent_absolute() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("absolute");
}

#[test]
fn independent_hash() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("hash");
}

#[test]
fn independent_parent() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("parent");
}

#[test]
fn independent_ledger() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("ledger");
}

#[test]
fn independent_live() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("live");
}

#[test]
fn independent_hooks() {
    cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case("hooks");
}

#[test]
fn independent_full_backup() {
    cases::partial_backup_case(false);
}

#[test]
fn independent_proof_free_partial_backup() {
    cases::partial_backup_case(true);
}

#[cfg(unix)]
#[test]
fn independent_backup_link_original() {
    cases::backup_link_case(false);
}

#[cfg(unix)]
#[test]
fn independent_backup_link_substituted() {
    cases::backup_link_case(true);
}
