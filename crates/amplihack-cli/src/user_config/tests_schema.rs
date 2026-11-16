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
            "orchestrator.max_depth",
            // Issue #1619: per-step model tiers and the small-diff combined
            // review. Documented in this position in
            // docs/reference/user-config.md#keys.
            "model.force",
            "model.tier_map",
            "model.step_tiers",
            "review.combined_small_diff",
            "review.combined_diff_threshold",
            "governor.enabled",
            "governor.max_concurrency",
            "governor.reserve_points",
            "governor.step",
            "governor.band_pct",
            "governor.eval_interval_secs",
            "governor.wait_secs",
            "governor.max_reading_age_secs",
            "sessions.daily_archive",
            "sessions.archive_days",
            "sessions.archive_dest",
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
fn user_only_keys_are_agent_home_node_log_dir_model_governor_and_sessions() {
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
            "recipe.log_dir",
            // Issue #1619: `model.force` names the model every launch asks
            // for, and the two `model.*` path keys name files that drive
            // amplihack's rewriting of recipe YAML, whose steps run shell
            // commands. A cloned repository must not choose any of the three
            // (the reason the review of #1647 gave for `agent.binary`).
            "model.force",
            "model.tier_map",
            "model.step_tiers",
            "governor.enabled",
            "governor.max_concurrency",
            "governor.reserve_points",
            "governor.step",
            "governor.band_pct",
            "governor.eval_interval_secs",
            "governor.wait_secs",
            "governor.max_reading_age_secs",
            "sessions.daily_archive",
            "sessions.archive_days",
            "sessions.archive_dest"
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

/// Issue #1811: the governor keys are user-only, so a repository cannot turn
/// the governor off or loosen it, and none has a resolver variable or flag:
/// the governor reads `AMPLIHACK_GOVERNOR` and `AMPLIHACK_GOVERNOR_WAIT`
/// itself.
#[test]
fn governor_keys_are_user_only_with_documented_ranges_and_defaults() {
    let cases: [(&str, Option<Value>, &str, &str); 8] = [
        (
            "governor.enabled",
            Some(Value::Bool(false)), // opt-in: the governor ships off
            "false",
            "maybe",
        ),
        ("governor.max_concurrency", Some(Value::Int(4)), "64", "0"),
        ("governor.reserve_points", Some(Value::Int(10)), "0", "101"),
        ("governor.step", Some(Value::Int(1)), "8", "9"),
        ("governor.band_pct", Some(Value::Int(10)), "50", "51"),
        (
            "governor.eval_interval_secs",
            Some(Value::Int(60)),
            "3600",
            "0",
        ),
        (
            "governor.wait_secs",
            Some(Value::Int(600)),
            "86400",
            "86401",
        ),
        (
            "governor.max_reading_age_secs",
            Some(Value::Int(3600)),
            "60",
            "59",
        ),
    ];
    for (name, default, ok, bad) in cases {
        let k = key_def(name).unwrap_or_else(|| panic!("{name} missing"));
        assert_eq!(k.scope, Scope::UserOnly, "{name}");
        assert_eq!(k.env, None, "{name}");
        assert!(!k.lower_only, "{name}");
        assert_eq!(k.default_value(), default, "{name}");
        assert!(parse_cli(name, ok).is_ok(), "{name} {ok}");
        assert!(parse_cli(name, bad).is_err(), "{name} {bad}");
    }
    assert!(parse_cli("governor.wait_secs", "0").is_ok());
    assert!(parse_cli("governor.reserve_points", "100").is_ok());
}

// =========================================================================
// Issue #1619 (TDD): the model-tier and combined-review settings.
//
// Every key here has a live consumer, which is the no-dead-config rule of
// issue #1606: `model.force` is read on every launch, the two path keys are
// read when the tier mapping is loaded, and the two `review.*` keys are read
// by the refactor-review phase's mode step.
// =========================================================================

#[test]
fn the_model_tier_keys_exist_with_the_documented_scope_and_variable() {
    let force = key_def("model.force").expect("model.force is a schema key");
    assert_eq!(force.scope, Scope::UserOnly);
    assert_eq!(force.env, Some("AMPLIHACK_FORCE_MODEL"));
    assert_eq!(force.default, None, "forcing a model is opt-in");

    for name in ["model.tier_map", "model.step_tiers"] {
        let key = key_def(name).unwrap_or_else(|| panic!("{name} is a schema key"));
        assert_eq!(key.scope, Scope::UserOnly, "{name}");
        assert_eq!(key.kind, ValueKind::AbsPath, "{name}");
        assert_eq!(
            key.env, None,
            "{name} names a file that drives recipe rewriting; it is set deliberately \
             in the user's own config, not picked up from an inherited environment"
        );
    }
}

#[test]
fn the_combined_review_keys_default_to_todays_behaviour() {
    let switch = key_def("review.combined_small_diff").expect("a schema key");
    assert_eq!(switch.scope, Scope::UserAndProject);
    assert_eq!(switch.kind, ValueKind::Bool);
    assert_eq!(
        switch.default_value(),
        Some(Value::Bool(false)),
        "off by default, so the three separate review passes run exactly as they did \
         before this feature"
    );

    let threshold = key_def("review.combined_diff_threshold").expect("a schema key");
    assert_eq!(threshold.scope, Scope::UserAndProject);
    assert_eq!(
        threshold.default_value(),
        Some(Value::Int(200)),
        "changed lines at or below which a diff counts as small"
    );
    assert!(
        matches!(threshold.kind, ValueKind::Int { min: 1, .. }),
        "a threshold of zero would mean no diff is ever small, which is what turning \
         the switch off already does: {:?}",
        threshold.kind
    );
}

#[test]
fn model_force_accepts_a_model_id_and_rejects_anything_that_could_act_as_an_argument() {
    // The value lands as the argv token right after `--model`, and both codex
    // and claude carry single-token sandbox-bypass flags
    // (`--yolo`, `--dangerously-bypass-approvals-and-sandbox`,
    // `--dangerously-skip-permissions`). It is validated on the raw input,
    // before any trimming.
    for good in [
        "claude-opus-5[1m]",
        "claude-sonnet-5-5",
        "claude-sonnet-5.5",
        "gpt-6.1-sol",
        "gpt-6.1-sol:high",
    ] {
        assert!(
            parse_cli("model.force", good).is_ok(),
            "`{good}` must be accepted"
        );
    }
    for bad in [
        "",
        " ",
        " claude-opus-5[1m]",
        "-s",
        "--yolo",
        "--dangerously-skip-permissions",
        "a b",
        "a\tb",
        "a\nb",
        "a/b",
        "a\\b",
        "a;b",
        "a$b",
        "a`b",
        "a'b",
        "a\"b",
        "a|b",
        &"x".repeat(129),
    ] {
        assert!(
            parse_cli("model.force", bad).is_err(),
            "`{bad:?}` must be rejected as a model id"
        );
    }
}

#[test]
fn the_model_tier_paths_are_held_to_the_same_path_rules_as_every_other_path_key() {
    for name in ["model.tier_map", "model.step_tiers"] {
        assert!(
            parse_cli(name, "/home/x/.amplihack/model-tiers.toml").is_ok(),
            "{name}"
        );
        for bad in ["relative/path.toml", "/a/../b", "", "/a\0b"] {
            assert!(parse_cli(name, bad).is_err(), "{name}: `{bad:?}`");
        }
    }
}

#[test]
fn no_project_file_can_name_the_file_that_rewrites_recipes() {
    // Stated separately from `no_project_scoped_key_names_a_path` because the
    // consequence is specific: a project-scoped path key here would let a
    // cloned repository choose the file that drives amplihack's rewriting of
    // recipe YAML, whose bash step bodies execute.
    for name in ["model.force", "model.tier_map", "model.step_tiers"] {
        assert_eq!(
            key_def(name).unwrap().scope,
            Scope::UserOnly,
            "{name} must never be project-scoped"
        );
    }
}
