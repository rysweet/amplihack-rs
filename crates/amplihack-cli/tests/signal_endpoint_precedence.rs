//! TDD (RED): endpoint resolution precedence + loopback choke-point (D1).
//!
//! Contract under test — `amplihack_cli::commands::signal::endpoint`:
//!
//! * `resolve_endpoint(port, env_port, endpoint, env_endpoint)` applies the
//!   precedence `--port > --endpoint > AMPLIHACK_SIGNAL_PORT > env endpoint >
//!   127.0.0.1:7583` (explicit CLI args outrank ambient env) and is a pure
//!   function (no process env / no I/O).
//! * `--port` / env-port always bind loopback (`127.0.0.1:<port>`).
//! * `resolve_endpoint` funnels every candidate through the SINGLE canonical
//!   loopback choke-point (`commands::signal::validate::validate_loopback_endpoint`,
//!   covered by `signal_security`); a non-loopback host is rejected with exit
//!   code 6 (daemon/port) and NO side effects.
//!
//! These tests compile to nothing until the `signal` feature and the module
//! exist; run them with `cargo test -p amplihack-cli --features signal`.
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::endpoint::{
    DEFAULT_ENDPOINT, DEFAULT_SIGNAL_PORT, resolve_endpoint,
};

#[test]
fn default_when_nothing_supplied() {
    let got = resolve_endpoint(None, None, None, None).expect("default is valid");
    assert_eq!(got, DEFAULT_ENDPOINT);
    assert_eq!(DEFAULT_SIGNAL_PORT, 7583);
    assert!(DEFAULT_ENDPOINT.starts_with("127.0.0.1:"));
}

#[test]
fn port_flag_wins_over_everything_and_is_loopback() {
    let got = resolve_endpoint(
        Some(9000),
        Some(9001),
        Some("127.0.0.1:9002"),
        Some("127.0.0.1:9003"),
    )
    .expect("valid loopback");
    assert_eq!(got, "127.0.0.1:9000");
}

#[test]
fn endpoint_flag_wins_over_env_port() {
    // Explicit CLI --endpoint must outrank the ambient AMPLIHACK_SIGNAL_PORT env,
    // so a user's inherited env port never silently overrides an explicit flag.
    let got =
        resolve_endpoint(None, Some(9001), Some("127.0.0.1:9002"), None).expect("valid loopback");
    assert_eq!(got, "127.0.0.1:9002");
}

#[test]
fn env_port_wins_over_env_endpoint_and_default() {
    // With no CLI args, AMPLIHACK_SIGNAL_PORT still outranks the env endpoint.
    let got =
        resolve_endpoint(None, Some(9001), None, Some("127.0.0.1:9003")).expect("valid loopback");
    assert_eq!(got, "127.0.0.1:9001");
}

#[test]
fn endpoint_flag_wins_over_env_endpoint() {
    let got = resolve_endpoint(None, None, Some("127.0.0.1:8000"), Some("127.0.0.1:8001"))
        .expect("valid loopback");
    assert_eq!(got, "127.0.0.1:8000");
}

#[test]
fn env_endpoint_used_as_last_non_default_source() {
    let got = resolve_endpoint(None, None, None, Some("127.0.0.1:8001")).expect("valid loopback");
    assert_eq!(got, "127.0.0.1:8001");
}

#[test]
fn non_loopback_endpoint_flag_is_rejected_exit_6() {
    let err = resolve_endpoint(None, None, Some("0.0.0.0:7583"), None)
        .expect_err("non-loopback must be rejected");
    assert_eq!(err.exit_code(), 6);
}

#[test]
fn routable_host_env_endpoint_is_rejected_exit_6() {
    let err = resolve_endpoint(None, None, None, Some("10.0.0.5:7583"))
        .expect_err("routable host must be rejected");
    assert_eq!(err.exit_code(), 6);
}

#[test]
fn dns_host_endpoint_is_rejected_exit_6() {
    let err = resolve_endpoint(None, None, Some("example.com:7583"), None)
        .expect_err("dns host must be rejected");
    assert_eq!(err.exit_code(), 6);
}
