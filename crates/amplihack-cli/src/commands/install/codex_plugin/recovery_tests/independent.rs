//! Each case selects exactly one existing guarded scenario.
use super::*;

#[test]
fn independent_config() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "config",
    );
}

#[test]
fn independent_hooks() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case("hooks");
}

#[test]
fn independent_package() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "package",
    );
}

#[test]
fn independent_backup() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "backup",
    );
}

#[test]
fn independent_marketplace() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "marketplace",
    );
}

#[test]
fn independent_ledger() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes_case(
        "ledger",
    );
}

#[cfg(unix)]
#[test]
fn independent_native_config_expected() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    native_config_case(false);
}

#[cfg(unix)]
#[test]
fn independent_native_config_foreign() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    native_config_case(true);
}
