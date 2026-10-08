//! Each case selects exactly one existing guarded scenario.
use super::*;

#[test]
fn independent_config() {
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "config",
    );
}

#[test]
fn independent_hooks() {
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case("hooks");
}

#[test]
fn independent_package() {
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "package",
    );
}

#[test]
fn independent_backup() {
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "backup",
    );
}

#[test]
fn independent_marketplace() {
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "marketplace",
    );
}

#[test]
fn independent_ledger() {
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "ledger",
    );
}

#[cfg(unix)]
#[test]
fn independent_native_config_expected() {
    native_config_case(false);
}

#[cfg(unix)]
#[test]
fn independent_native_config_foreign() {
    native_config_case(true);
}
