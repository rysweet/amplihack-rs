//! TDD contract — validator parity & boundary defense (SR4/#921/#923).
//!
//! Run with:
//! `cargo test -p amplihack-cli --features signal --test signal_validator_parity`
//!
//! The Signal fleet path fans commands out over
//! `azlin connect <vm> --resource-group <rg> ... -- '<cmd>'`. VM names are an
//! injection surface shared with the `fleet` command family, so the two VM-name
//! validators MUST NOT DRIFT: a name accepted by one and rejected by the other
//! is a latent security gap.
//!
//! `fleet::reasoning_validation::validate_vm_name` is `pub(super)` and cannot be
//! imported here, so we lock `signal::validate::validate_vm_name` to the exact
//! *specified* fleet rule expressed as an independent oracle:
//!
//!   first char ASCII-alphanumeric; remaining chars in `[A-Za-z0-9_-]`;
//!   total length 1..=64.
//!
//! If either implementation changes its rule, this oracle must be updated in
//! lockstep, forcing the drift to be a conscious, reviewed decision.
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::validate;

/// Independent re-statement of the shared VM-name rule (the parity oracle).
fn vm_name_oracle(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false; // empty
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    name.len() <= 64 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Corpus spanning the accept/reject boundary and known injection payloads.
fn vm_name_corpus() -> Vec<String> {
    let mut cases: Vec<String> = vec![
        // valid
        "a".into(),
        "ia2".into(),
        "azlin-01".into(),
        "vm_prod".into(),
        "web-server-3".into(),
        "A1".into(),
        "9".into(),
        // invalid — structure
        String::new(),
        "-leading".into(),
        "_leading".into(),
        ".dotstart".into(),
        "has space".into(),
        "tab\tsep".into(),
        "new\nline".into(),
        "dot.in.name".into(),
        "unicodé".into(),
        // invalid — shell metacharacters (injection)
        "vm;rm -rf /".into(),
        "vm && curl evil".into(),
        "vm`whoami`".into(),
        "vm$(id)".into(),
        "vm|nc".into(),
        "vm>out".into(),
    ];
    // Length boundary: exactly 64 (valid) and 65 (invalid).
    cases.push("a".repeat(64));
    cases.push("a".repeat(65));
    cases
}

#[test]
fn signal_vm_name_validator_matches_fleet_oracle() {
    for name in vm_name_corpus() {
        let accepted = validate::validate_vm_name(&name).is_ok();
        let expected = vm_name_oracle(&name);
        assert_eq!(
            accepted, expected,
            "DRIFT: validate_vm_name({name:?}) = {accepted}, fleet oracle = {expected}"
        );
    }
}

#[test]
fn vm_name_length_boundary_is_inclusive_64() {
    assert!(validate::validate_vm_name(&"a".repeat(64)).is_ok());
    assert!(validate::validate_vm_name(&"a".repeat(65)).is_err());
}

// ---------------------------------------------------------------------------
// Device name — passed to `signal-cli link -n <name>` and interpolated into the
// remote `--device-name amplihack-<name>` string. Defense-in-depth allowlist.
// ---------------------------------------------------------------------------

#[test]
fn device_name_allows_safe_charset_and_rejects_metacharacters() {
    for ok in ["amplihack-host", "vm_01", "node.1", "A-b_c.9"] {
        assert!(
            validate::validate_device_name(ok).is_ok(),
            "{ok:?} should be an accepted device name"
        );
    }
    for bad in [
        "",
        "has space",
        "semi;colon",
        "quote'name",
        "back`tick`",
        &"x".repeat(65),
    ] {
        assert!(
            validate::validate_device_name(bad).is_err(),
            "{bad:?} MUST be rejected as a device name"
        );
    }
}

// ---------------------------------------------------------------------------
// Resource group — max length 90, allows an extra `.` vs VM names, still rejects
// shell metacharacters. Locks the documented rule so it cannot silently widen.
// ---------------------------------------------------------------------------

#[test]
fn resource_group_charset_and_length_boundary() {
    for ok in ["rg-prod_1", "my.rg", "A9"] {
        assert!(
            validate::validate_resource_group(ok).is_ok(),
            "{ok:?} should be an accepted resource group"
        );
    }
    for bad in ["", "-lead", "rg;drop", "rg$(x)", "rg space", "rg/slash"] {
        assert!(
            validate::validate_resource_group(bad).is_err(),
            "{bad:?} resource group MUST be rejected"
        );
    }
    assert!(validate::validate_resource_group(&"a".repeat(90)).is_ok());
    assert!(validate::validate_resource_group(&"a".repeat(91)).is_err());
}

// ---------------------------------------------------------------------------
// Account (E.164) — must match amplihack-signal's config loader rule exactly so
// the writer never emits a config the real loader would reject.
// ---------------------------------------------------------------------------

#[test]
fn account_e164_boundary() {
    assert!(validate::validate_account("+12065551234").is_ok());
    assert!(validate::validate_account("+1").is_ok());
    assert!(validate::validate_account(&format!("+{}", "1".repeat(15))).is_ok());
    for bad in [
        "12065551234",                   // missing '+'
        "+",                             // no digits
        "+abc",                          // non-digits
        "+1 206",                        // space
        &format!("+{}", "1".repeat(16)), // 16 digits > 15
    ] {
        assert!(
            validate::validate_account(bad).is_err(),
            "{bad:?} MUST fail E.164 validation"
        );
    }
}
