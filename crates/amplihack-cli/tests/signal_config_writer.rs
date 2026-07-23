//! TDD contract — `amplihack signal setup` config generation (#921).
//!
//! Run with: `cargo test -p amplihack-cli --features signal --test signal_config_writer`
//!
//! `commands::signal::config_writer::to_toml` produces the `signal-config.toml`
//! that onboarding writes to `~/.amplihack/signal-config.toml`. The critical
//! contract is that this output is consumed UNCHANGED by the existing
//! per-session channel, so these tests round-trip the generated TOML back
//! through the REAL `amplihack_signal::config::SignalConfig` resolver.
//!
//! Single-number linked-device rule: the operator's phone replies arrive as the
//! account's own synced messages, so the account number MUST be on its own
//! allowlist — i.e. `allowlist = [account]` exactly.
#![cfg(feature = "signal")]

use std::collections::HashMap;

use amplihack_cli::commands::signal::config_writer;
use amplihack_signal::config::SignalConfig;

const ENDPOINT: &str = "127.0.0.1:7583";
const ACCOUNT: &str = "+12062591306";

/// Resolve generated TOML through the real loader with an EMPTY environment, so
/// the file is the sole source of truth (proves it is self-sufficient).
fn resolve(toml: &str) -> SignalConfig {
    let empty_env: HashMap<String, String> = HashMap::new();
    SignalConfig::from_sources(&empty_env, Some(toml))
        .expect("generated TOML must resolve under the real amplihack-signal loader")
}

#[test]
fn generated_toml_round_trips_through_real_resolver() {
    let toml = config_writer::to_toml(ENDPOINT, ACCOUNT);
    let cfg = resolve(&toml);
    assert_eq!(cfg.endpoint, ENDPOINT);
    assert_eq!(cfg.account, ACCOUNT);
}

#[test]
fn allowlist_is_exactly_the_account_number() {
    // Single-number linked-device case: allowlist MUST equal [account].
    let toml = config_writer::to_toml(ENDPOINT, ACCOUNT);
    let cfg = resolve(&toml);
    assert_eq!(
        cfg.allowlist,
        vec![ACCOUNT.to_string()],
        "onboarding must emit allowlist=[account] so the operator's own synced replies are accepted"
    );
}

#[test]
fn generated_toml_never_emits_empty_or_wildcard_allowlist() {
    let toml = config_writer::to_toml(ENDPOINT, ACCOUNT);
    // Fail-closed: never an empty allowlist, never a wildcard.
    assert!(
        !toml.contains("allowlist = []"),
        "must not emit an empty (deny-all) allowlist"
    );
    assert!(!toml.contains('*'), "must never emit a wildcard allowlist");
    let cfg = resolve(&toml);
    assert!(
        !cfg.allowlist.is_empty(),
        "resolved allowlist must be non-empty"
    );
}

#[test]
fn invalid_account_is_rejected_before_write() {
    // Writer must validate E.164 rather than emit a config the loader rejects.
    let toml = config_writer::to_toml(ENDPOINT, "not-a-number");
    let empty_env: HashMap<String, String> = HashMap::new();
    assert!(
        SignalConfig::from_sources(&empty_env, Some(&toml)).is_err(),
        "a non-E.164 account must not yield a config the real loader accepts"
    );
}

#[test]
fn env_still_overrides_the_generated_file() {
    // Precedence guarantee: env > TOML. Onboarding must not break this.
    let toml = config_writer::to_toml(ENDPOINT, ACCOUNT);
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert(
        "AMPLIHACK_SIGNAL_ENDPOINT".to_string(),
        "127.0.0.1:9999".to_string(),
    );
    let cfg = SignalConfig::from_sources(&env, Some(&toml)).expect("valid");
    assert_eq!(
        cfg.endpoint, "127.0.0.1:9999",
        "environment variables must still override the generated TOML"
    );
}

#[test]
fn generated_toml_contains_only_expected_keys() {
    // Hygiene: no stray secrets (e.g. a link URI) must leak into the config.
    let toml = config_writer::to_toml(ENDPOINT, ACCOUNT);
    assert!(
        !toml.contains("sgnl://"),
        "config must not embed a link URI"
    );
    assert!(
        !toml.contains("tsdevice"),
        "config must not embed a link URI"
    );
    assert!(
        !toml.contains("pub_key"),
        "config must not embed link key material"
    );
}
