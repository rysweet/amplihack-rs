//! TDD contract — zero-step per-session wiring via a default config path (#921).
//!
//! `amplihack signal setup` writes `~/.amplihack/signal-config.toml`. For the
//! promised "zero further steps" wiring, the existing per-session channel's
//! config loader must FALL BACK to that default path when the
//! `AMPLIHACK_SIGNAL_CONFIG` env var is unset. This closes the small gap
//! between onboarding output and the SessionStart integration.
//!
//! Resolution order (unchanged precedence, extended with a default file):
//!   env vars  >  AMPLIHACK_SIGNAL_CONFIG file  >  ~/.amplihack/signal-config.toml  >  error
//!
//! These tests define the two seams the loader change relies on, kept pure /
//! home-injectable so no real `$HOME` or process env is mutated.
#![cfg(feature = "signal")]

use std::collections::HashMap;
use std::path::Path;

use amplihack_signal::config::{self, SignalConfig};

const SAMPLE_TOML: &str = r#"
endpoint = "127.0.0.1:7583"
account = "+12062591306"
allowlist = ["+12062591306"]
"#;

#[test]
fn default_config_path_is_under_amplihack_home() {
    let home = Path::new("/home/operator");
    let p = config::default_config_path_in(home);
    assert_eq!(
        p,
        Path::new("/home/operator/.amplihack/signal-config.toml"),
        "default config path must be <home>/.amplihack/signal-config.toml"
    );
}

#[test]
fn resolve_source_prefers_env_config_path_over_default() {
    let dir = tempfile::tempdir().unwrap();
    let env_file = dir.path().join("explicit.toml");
    let default_file = dir.path().join(".amplihack/signal-config.toml");
    std::fs::create_dir_all(default_file.parent().unwrap()).unwrap();
    std::fs::write(&env_file, "endpoint = \"127.0.0.1:1111\"\n").unwrap();
    std::fs::write(&default_file, SAMPLE_TOML).unwrap();

    let mut env: HashMap<String, String> = HashMap::new();
    env.insert(
        config::ENV_CONFIG_PATH.to_string(),
        env_file.display().to_string(),
    );

    let toml = config::resolve_config_source(&env, &default_file)
        .expect("resolve")
        .expect("some source");
    assert!(
        toml.contains("127.0.0.1:1111"),
        "explicit AMPLIHACK_SIGNAL_CONFIG file must win over the default path"
    );
}

#[test]
fn resolve_source_falls_back_to_default_path_when_env_unset() {
    let dir = tempfile::tempdir().unwrap();
    let default_file = dir.path().join(".amplihack/signal-config.toml");
    std::fs::create_dir_all(default_file.parent().unwrap()).unwrap();
    std::fs::write(&default_file, SAMPLE_TOML).unwrap();

    let env: HashMap<String, String> = HashMap::new(); // no AMPLIHACK_SIGNAL_CONFIG

    let toml = config::resolve_config_source(&env, &default_file)
        .expect("resolve")
        .expect("default file should be used");
    // Prove the fallback file actually drives a valid resolved config.
    let cfg = SignalConfig::from_sources(&env, Some(&toml)).expect("valid");
    assert_eq!(cfg.account, "+12062591306");
    assert_eq!(cfg.allowlist, vec!["+12062591306".to_string()]);
}

#[test]
fn resolve_source_is_none_when_neither_env_nor_default_exists() {
    let dir = tempfile::tempdir().unwrap();
    let missing_default = dir.path().join(".amplihack/signal-config.toml");
    let env: HashMap<String, String> = HashMap::new();

    // Absent default file → None (channel stays disabled), NOT an error.
    let src = config::resolve_config_source(&env, &missing_default).expect("resolve ok");
    assert!(
        src.is_none(),
        "with no env var and no default file, resolution yields None (disabled)"
    );
}
