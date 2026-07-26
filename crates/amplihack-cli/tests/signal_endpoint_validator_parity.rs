//! TDD contract — F1 cross-crate validator parity.
//!
//! Written **first** (Step 7 TDD). After F1 there must be exactly ONE loopback
//! endpoint validator in the workspace. The CLI's
//! `signal::validate::validate_loopback_endpoint` and the signal crate's
//! `bridge::validate_endpoint` (with `unsafe_remote = false`) must agree on
//! *every* input — a value accepted by one and rejected by the other is a
//! validator-divergence, which is exactly the confinement gap this hardening
//! pass closes. This test locks the two entry points together so any future
//! drift is a conscious, reviewed decision.
//!
//! Run: `cargo test -p amplihack-cli --features signal --test
//! signal_endpoint_validator_parity`.
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::validate::validate_loopback_endpoint;
use amplihack_signal::bridge::validate_endpoint;

/// Corpus spanning the accept/reject boundary for loopback endpoints.
fn endpoint_corpus() -> Vec<&'static str> {
    vec![
        // loopback — accept
        "127.0.0.1:7583",
        "127.0.0.5:9000",
        "::1:7583",
        "[::1]:7583",
        "localhost:7583",
        // wildcard — reject
        "0.0.0.0:7583",
        "[::]:7583",
        "::",
        // routable — reject
        "10.0.0.5:7583",
        "93.184.216.34:7583",
        "[2606:4700:4700::1111]:7583",
        // DNS — reject
        "example.com:7583",
        "signal.local:7583",
        "localhost.evil.example:7583",
        // malformed / port boundaries — reject
        "127.0.0.1:0",
        "127.0.0.1:70000",
        "127.0.0.1:abc",
        "127.0.0.1:",
        "127.0.0.1",
        "",
    ]
}

#[test]
fn cli_and_runtime_loopback_validators_do_not_drift() {
    for endpoint in endpoint_corpus() {
        let cli_ok = validate_loopback_endpoint(endpoint).is_ok();
        let runtime_ok = validate_endpoint(endpoint, false).is_ok();
        assert_eq!(
            cli_ok, runtime_ok,
            "DRIFT for {endpoint:?}: CLI accepts={cli_ok}, runtime accepts={runtime_ok}"
        );
    }
}

#[test]
fn both_validators_accept_bare_ipv6_loopback() {
    assert!(validate_loopback_endpoint("::1:7583").is_ok());
    assert!(validate_endpoint("::1:7583", false).is_ok());
}

#[test]
fn both_validators_reject_wildcard_and_dns() {
    for bad in ["0.0.0.0:7583", "[::]:7583", "example.com:7583"] {
        assert!(
            validate_loopback_endpoint(bad).is_err(),
            "CLI must reject {bad:?}"
        );
        assert!(
            validate_endpoint(bad, false).is_err(),
            "runtime must reject {bad:?}"
        );
    }
}
