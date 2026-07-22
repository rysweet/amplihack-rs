//! TDD contract — Signal onboarding security invariants (#921/#923).
//!
//! Run with: `cargo test -p amplihack-cli --features signal --test signal_security`
//!
//! The fleet path fans commands out over `azlin connect <vm> --resource-group
//! <rg> ... -- '<cmd>'`, so VM and resource-group names are an injection
//! surface. Contract (`commands::signal::validate`):
//!   * VM / resource-group names are VALIDATED-AND-REJECTED at the boundary
//!     (never silently stripped into a different valid target).
//!   * The account is validated as E.164.
//!   * The daemon endpoint MUST bind loopback only (127.0.0.1 / ::1 /
//!     localhost); wildcard or routable binds are refused (DAEMON failure).
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::validate;

// ---------------------------------------------------------------------------
// VM / resource-group name validation (injection defense)
// ---------------------------------------------------------------------------

#[test]
fn accepts_well_formed_vm_names() {
    for name in ["ia2", "azlin-01", "vm_prod", "web-server-3"] {
        assert!(
            validate::validate_vm_name(name).is_ok(),
            "{name:?} should be a valid VM name"
        );
    }
}

#[test]
fn rejects_shell_metacharacters_in_vm_names() {
    for bad in [
        "vm;rm -rf /",
        "vm && curl evil",
        "vm`whoami`",
        "vm$(id)",
        "vm|nc",
        "vm\nwhoami",
        "vm with space",
        "",
    ] {
        assert!(
            validate::validate_vm_name(bad).is_err(),
            "{bad:?} MUST be rejected (validate-and-reject, never silently stripped)"
        );
    }
}

#[test]
fn rejects_shell_metacharacters_in_resource_group() {
    assert!(validate::validate_resource_group("rg-prod_1").is_ok());
    for bad in ["rg;drop", "rg$(x)", "rg space", ""] {
        assert!(
            validate::validate_resource_group(bad).is_err(),
            "{bad:?} resource group must be rejected"
        );
    }
}

// ---------------------------------------------------------------------------
// Account (E.164) validation
// ---------------------------------------------------------------------------

#[test]
fn validates_account_as_e164() {
    assert!(validate::validate_account("+12062591306").is_ok());
    for bad in ["12062591306", "+", "+abc", "+1 206 259", "++1206"] {
        assert!(
            validate::validate_account(bad).is_err(),
            "{bad:?} must fail E.164 validation"
        );
    }
}

// ---------------------------------------------------------------------------
// Loopback-only daemon endpoint enforcement
// ---------------------------------------------------------------------------

#[test]
fn accepts_loopback_endpoints() {
    for ep in ["127.0.0.1:7583", "localhost:7583", "[::1]:7583"] {
        assert!(
            validate::validate_loopback_endpoint(ep).is_ok(),
            "{ep:?} is loopback and must be accepted"
        );
    }
}

#[test]
fn rejects_non_loopback_or_wildcard_endpoints() {
    for ep in [
        "0.0.0.0:7583",
        "[::]:7583",
        "192.168.1.10:7583",
        "10.0.0.5:7583",
        "example.com:7583",
    ] {
        assert!(
            validate::validate_loopback_endpoint(ep).is_err(),
            "{ep:?} is NOT loopback and MUST be refused (never forward the daemon port)"
        );
    }
}

#[test]
fn rejects_malformed_endpoints() {
    for ep in ["127.0.0.1", "127.0.0.1:99999", "127.0.0.1:0", ":7583", ""] {
        assert!(
            validate::validate_loopback_endpoint(ep).is_err(),
            "{ep:?} is malformed and must be refused"
        );
    }
}
