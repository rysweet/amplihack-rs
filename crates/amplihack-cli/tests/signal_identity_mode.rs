//! TDD (RED): identity mode enum + not-yet-implemented gate (D2).
//!
//! Contract — `amplihack_cli::commands::signal::identity`:
//!
//! * `IdentityMode` is a clap `ValueEnum` with kebab-case names
//!   `linked-device` (default) and `dedicated-number`.
//! * `IdentityMode::default() == LinkedDevice`.
//! * It (de)serialises with serde in kebab-case, and old distribute-state that
//!   omits the field deserialises to `LinkedDevice` (see `#[serde(default)]`).
//! * `ensure_supported(DedicatedNumber)` returns the UNSUPPORTED error (exit 3)
//!   BEFORE any side effect; `LinkedDevice` is Ok.
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::identity::{IdentityMode, ensure_supported};

#[test]
fn default_is_linked_device() {
    assert_eq!(IdentityMode::default(), IdentityMode::LinkedDevice);
}

#[test]
fn linked_device_is_supported() {
    ensure_supported(IdentityMode::LinkedDevice).expect("linked-device is implemented");
}

#[test]
fn dedicated_number_is_not_yet_implemented_exit_3() {
    let err =
        ensure_supported(IdentityMode::DedicatedNumber).expect_err("dedicated-number is a stub");
    assert_eq!(
        err.exit_code(),
        3,
        "not-yet-implemented maps to UNSUPPORTED exit 3"
    );
}

#[test]
fn serde_uses_kebab_case() {
    let j = serde_json::to_string(&IdentityMode::LinkedDevice).unwrap();
    assert_eq!(j, "\"linked-device\"");
    let j = serde_json::to_string(&IdentityMode::DedicatedNumber).unwrap();
    assert_eq!(j, "\"dedicated-number\"");

    let back: IdentityMode = serde_json::from_str("\"dedicated-number\"").unwrap();
    assert_eq!(back, IdentityMode::DedicatedNumber);
}

#[test]
fn value_enum_parses_cli_tokens() {
    use clap::ValueEnum;
    let m = IdentityMode::from_str("linked-device", true).unwrap();
    assert_eq!(m, IdentityMode::LinkedDevice);
    let m = IdentityMode::from_str("dedicated-number", true).unwrap();
    assert_eq!(m, IdentityMode::DedicatedNumber);
    assert!(IdentityMode::from_str("bogus", true).is_err());
}
