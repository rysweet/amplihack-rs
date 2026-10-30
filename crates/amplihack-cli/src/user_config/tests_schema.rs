//! Issue #1606 (TDD): schema invariants for `user_config::schema`.
//!
//! Wire with `#[cfg(test)] mod tests_schema;` in `user_config/mod.rs`.
//! Expected API: `KEYS: &[KeyDef]` with `name`, `scope: Scope`,
//! `kind: ValueKind`, `env: Option<&str>`; `parse_cli(key, raw) ->
//! Result<Value, ConfigError>`; `ConfigError::exit_code()`.

use super::schema::{KEYS, Scope, Value, ValueKind, key as key_def, parse_cli};

#[test]
fn keys_are_in_documented_order() {
    let names: Vec<&str> = KEYS.iter().map(|k| k.name).collect();
    assert_eq!(
        names,
        [
            "agent.binary",
            "paths.home",
            "node.heap_consent",
            "node.max_old_space_mb",
            "recipe.verbose",
            "recipe.step_timeout",
            "recipe.detach",
            "recipe.tmux_session_pattern",
            "recipe.log_dir",
            "recipe.usage_limit_wait",
            "recipe.usage_limit_max_wait_secs",
            "recipe.usage_limit_wait_budget_secs",
            "recipe.usage_limit_max_waits_per_step",
            "orchestrator.max_depth",
        ]
    );
}

#[test]
fn no_project_scoped_key_names_a_path() {
    // Trust rule: a cloned repository may tune behaviour but never point
    // amplihack at a path or executable.
    for key in KEYS {
        if key.scope == Scope::UserAndProject {
            assert!(
                !matches!(key.kind, ValueKind::AbsPath),
                "{} is project-scoped and path-valued",
                key.name
            );
        }
    }
}

#[test]
fn user_only_keys_are_agent_home_node_and_log_dir() {
    let user_only: Vec<&str> = KEYS
        .iter()
        .filter(|k| k.scope == Scope::UserOnly)
        .map(|k| k.name)
        .collect();
    assert_eq!(
        user_only,
        [
            "agent.binary",
            "paths.home",
            "node.heap_consent",
            "node.max_old_space_mb",
            "recipe.log_dir"
        ]
    );
}

#[test]
fn every_default_passes_its_own_validation() {
    for key in KEYS {
        let _ = key.default_value();
    }
    assert_eq!(
        key_def("orchestrator.max_depth").unwrap().default_value(),
        Some(Value::Int(u64::from(
            crate::commands::session_tree::state::DEFAULT_MAX_DEPTH
        )))
    );
}

#[test]
fn session_patterns_allow_placeholders_and_safe_characters_only() {
    use super::schema::validate_session_pattern;
    for good in ["amp-{recipe}-{repo}-{ts}", "run_{ts}", "x"] {
        assert!(validate_session_pattern(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "a.b",
        "a:b",
        "a b",
        "{user}",
        "$(id)",
        "a'b",
        &"x".repeat(65),
    ] {
        assert!(validate_session_pattern(bad).is_err(), "{bad}");
    }
}

#[test]
fn detach_accepts_tmux_and_none_only() {
    assert!(parse_cli("recipe.detach", "tmux").is_ok());
    assert!(parse_cli("recipe.detach", "none").is_ok());
    assert!(parse_cli("recipe.detach", "screen").is_err());
}

#[test]
fn env_vars_match_existing_consumers() {
    let env = |name: &str| KEYS.iter().find(|k| k.name == name).unwrap().env;
    assert_eq!(env("agent.binary"), Some("AMPLIHACK_AGENT_BINARY"));
    assert_eq!(env("paths.home"), Some("AMPLIHACK_HOME"));
    assert_eq!(env("recipe.step_timeout"), Some("AMPLIHACK_STEP_TIMEOUT"));
    assert_eq!(env("orchestrator.max_depth"), Some("AMPLIHACK_MAX_DEPTH"));
    assert_eq!(env("node.heap_consent"), None);
    assert_eq!(env("recipe.verbose"), None);
    assert_eq!(
        env("recipe.usage_limit_wait_budget_secs"),
        Some("AMPLIHACK_USAGE_LIMIT_WAIT_BUDGET_SECS")
    );
    assert_eq!(
        env("recipe.usage_limit_max_waits_per_step"),
        Some("AMPLIHACK_USAGE_LIMIT_MAX_WAITS_PER_STEP")
    );
}

#[test]
fn max_depth_range_tracks_the_session_tree_ceiling() {
    use crate::commands::session_tree::state::MAX_DEPTH_CEILING;
    assert!(parse_cli("orchestrator.max_depth", "0").is_ok());
    assert!(parse_cli("orchestrator.max_depth", &MAX_DEPTH_CEILING.to_string()).is_ok());
    assert!(
        parse_cli(
            "orchestrator.max_depth",
            &(MAX_DEPTH_CEILING + 1).to_string()
        )
        .is_err()
    );
}

#[test]
fn invalid_values_are_usage_errors() {
    for (key, raw) in [
        ("orchestrator.max_depth", "33"),
        ("node.max_old_space_mb", "8191"),
        ("node.max_old_space_mb", "32769"),
        ("recipe.step_timeout", "0"),
        ("recipe.usage_limit_wait_budget_secs", "-1"),
        (
            "recipe.usage_limit_wait_budget_secs",
            "18446744073709551616",
        ),
        ("recipe.usage_limit_max_waits_per_step", "0"),
        ("recipe.usage_limit_max_waits_per_step", "101"),
        ("recipe.verbose", "TRUE1"),
        ("agent.binary", "claude;id"),
        ("paths.home", "rel"),
        ("paths.home", "/a/\n/b"),
        ("unknown.key", "x"),
    ] {
        let err = parse_cli(key, raw).expect_err(&format!("{key}={raw:?} accepted"));
        assert_eq!(err.exit_code(), 2, "{key}={raw:?}");
    }
}

#[test]
fn agent_binary_is_canonicalised() {
    assert_eq!(
        parse_cli("agent.binary", "Claude").unwrap().to_string(),
        "\"claude\""
    );
}

#[test]
fn paths_home_rejects_overlong_values() {
    let long = format!("/{}", "a".repeat(4096));
    assert!(parse_cli("paths.home", &long).is_err());
}

/// Issue #1761: the run budget and the waits per step. Both may be set in a
/// project file, which can only lower them; the 1..=100 range of the waits
/// per step is a safety limit.
#[test]
fn the_usage_limit_budget_keys_are_lower_only_with_their_ranges_and_defaults() {
    let budget = key_def("recipe.usage_limit_wait_budget_secs").unwrap();
    let waits = key_def("recipe.usage_limit_max_waits_per_step").unwrap();
    for k in [budget, waits] {
        assert_eq!(k.scope, Scope::UserAndProject, "{}", k.name);
        assert!(k.lower_only, "{} must be lower only", k.name);
    }
    assert_eq!(budget.default_value(), Some(Value::Int(21_600)));
    assert_eq!(waits.default_value(), Some(Value::Int(5)));
    assert_eq!(
        budget.kind,
        ValueKind::Int {
            min: 0,
            max: u64::MAX
        }
    );
    assert_eq!(waits.kind, ValueKind::Int { min: 1, max: 100 });
    for raw in ["0", "21600", "18446744073709551615"] {
        assert!(parse_cli(budget.name, raw).is_ok(), "{raw}");
    }
    for raw in ["1", "5", "100"] {
        assert!(parse_cli(waits.name, raw).is_ok(), "{raw}");
    }
}
