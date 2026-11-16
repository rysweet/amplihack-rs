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
            "policy.deny_commands",
            "orchestrator.max_depth",
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
fn user_only_keys_are_agent_home_node_log_dir_policy_governor_and_sessions() {
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
            // Issue #2589: a cloned repository must not change what any
            // harness denies, so the instance additions are user-only.
            "policy.deny_commands",
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

// --- issue #2589: policy.deny_commands, a list-valued key -------------------
//
// Expected additions to the schema:
//   * `ValueKind::CommandList`
//   * `Value::List(Vec<String>)`
//   * `KeyDef { name: "policy.deny_commands", scope: UserOnly,
//               kind: ValueKind::CommandList, env: None, default: None }`
//   * `MAX_DENY_COMMANDS`, the bound on how many entries one file may add.

#[test]
fn the_deny_commands_key_is_user_only_list_valued_and_has_no_variable() {
    let def = key_def("policy.deny_commands").expect("the key is registered");
    assert_eq!(def.scope, Scope::UserOnly);
    assert_eq!(def.kind, ValueKind::CommandList);
    assert_eq!(
        def.env, None,
        "no variable: a deny list that one shell can unset is not one"
    );
    assert_eq!(
        def.default, None,
        "unset by default; the defaults are in code"
    );
    assert!(!def.lower_only);
}

#[test]
fn a_list_value_prints_its_entries_on_one_line_for_config_get() {
    let value = Value::List(vec!["pkill".into(), "tmux kill-server".into()]);
    assert_eq!(value.bare(), "pkill, tmux kill-server");
}

#[test]
fn a_list_value_renders_as_a_toml_array_with_escaped_strings() {
    let value = Value::List(vec!["a".into(), "b".into()]);
    assert_eq!(value.to_string(), "[\"a\", \"b\"]");
    assert_eq!(Value::List(Vec::new()).to_string(), "[]");
}

/// `config set` writes scalars only (`edit.rs`). Rather than half-write a
/// list, the command says plainly that the key is set by editing the user
/// file. Recorded as a follow-up, not a defect.
#[test]
fn a_list_key_cannot_be_set_from_the_command_line() {
    let err = parse_cli("policy.deny_commands", "pkill").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("policy.deny_commands"), "{text}");
    assert!(
        text.contains("user file") || text.contains("config.toml"),
        "the error must say where the key is set: {text}"
    );
    assert_eq!(err.exit_code(), 2, "a usage error");
}

#[test]
fn a_command_list_entry_is_validated_as_a_prefix() {
    use super::schema::{from_toml, parse_command_list};

    assert_eq!(
        parse_command_list(&["pkill".to_string(), "tmux kill-server".to_string()]).unwrap(),
        Value::List(vec!["pkill".into(), "tmux kill-server".into()])
    );
    for bad in [
        vec!["".to_string()],
        vec!["shell(x)".to_string()],
        vec!["a\u{0}b".to_string()],
        vec!["a\nb".to_string()],
        vec!["x".repeat(200)],
    ] {
        assert!(parse_command_list(&bad).is_err(), "{bad:?}");
    }

    // A TOML value of the wrong shape is rejected with the kind's own
    // requirement text, never accepted as a one-element list.
    let doc: toml_edit::DocumentMut = "x = \"pkill\"\ny = [1, 2]\n".parse().unwrap();
    for name in ["x", "y"] {
        let item = doc.as_table().get(name).unwrap().as_value().unwrap();
        assert!(from_toml(ValueKind::CommandList, item).is_err(), "{name}");
    }
}

#[test]
fn a_file_may_not_add_more_entries_than_the_bound() {
    use super::schema::{MAX_DENY_COMMANDS, parse_command_list};
    let at_limit: Vec<String> = (0..MAX_DENY_COMMANDS).map(|i| format!("cmd-{i}")).collect();
    assert!(parse_command_list(&at_limit).is_ok());
    let over: Vec<String> = (0..MAX_DENY_COMMANDS + 1)
        .map(|i| format!("cmd-{i}"))
        .collect();
    assert!(parse_command_list(&over).is_err());
}
