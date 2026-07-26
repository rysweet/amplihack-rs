use super::*;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[test]
fn env_only_minimal_valid_config() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001,+15551230002"),
    ]);
    let cfg = SignalConfig::from_sources(&e, None).expect("valid config");
    assert_eq!(cfg.endpoint, "127.0.0.1:7583");
    assert_eq!(cfg.account, "+15551230000");
    assert_eq!(cfg.allowlist, vec!["+15551230001", "+15551230002"]);
    assert_eq!(cfg.own_device_id, None);
    assert!(!cfg.reuse_rolling_group);
    assert_eq!(cfg.rolling_group_id, None);
}

#[test]
fn own_device_id_parsed_from_env() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
        (ENV_OWN_DEVICE_ID, "3"),
    ]);
    let cfg = SignalConfig::from_sources(&e, None).unwrap();
    assert_eq!(cfg.own_device_id, Some(3));
}

#[test]
fn own_device_id_below_two_is_error() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
        (ENV_OWN_DEVICE_ID, "1"),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(
        matches!(err, ConfigError::InvalidNumber { key, .. } if key == ENV_OWN_DEVICE_ID),
        "expected InvalidNumber for own_device_id, got {err:?}"
    );
}

#[test]
fn toml_own_device_id_below_two_is_error() {
    let toml = r#"
        endpoint = "127.0.0.1:7583"
        account  = "+15551230000"
        allowlist = ["+15551230001"]
        own_device_id = -1
    "#;
    let err = SignalConfig::from_sources(&HashMap::new(), Some(toml)).unwrap_err();
    assert!(
        matches!(err, ConfigError::InvalidNumber { key, .. } if key == TOML_OWN_DEVICE_ID),
        "expected InvalidNumber keyed to the TOML source for own_device_id, got {err:?}"
    );
}

#[test]
fn non_integer_toml_own_device_id_is_type_error() {
    // A present-but-wrong-type own_device_id must report a TOML type error, not
    // masquerade as a numeric-parse failure.
    let toml = r#"
        endpoint = "127.0.0.1:7583"
        account  = "+15551230000"
        allowlist = ["+15551230001"]
        own_device_id = true
    "#;
    let err = SignalConfig::from_sources(&HashMap::new(), Some(toml)).unwrap_err();
    assert!(
        matches!(err, ConfigError::Toml(_)),
        "expected Toml type error for non-integer own_device_id, got {err:?}"
    );
}

#[test]
fn missing_endpoint_is_error() {
    let e = env(&[
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(matches!(err, ConfigError::MissingRequired("endpoint")));
}

#[test]
fn missing_account_is_error() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ALLOWLIST, "+15551230001"),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(matches!(err, ConfigError::MissingRequired("account")));
}

#[test]
fn blank_env_allowlist_shadows_populated_toml_fail_closed() {
    // Precedence lock-in: a set-but-blank env allowlist wins over a populated
    // TOML allowlist, collapsing to deny-all (fail-closed). This is the
    // documented behavior; resolve_allowlist emits a warn when it happens.
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "   "),
    ]);
    let toml = r#"
        allowlist = ["+15551230001", "+15551230002"]
    "#;
    let cfg = SignalConfig::from_sources(&e, Some(toml)).expect("valid: env wins, deny-all");
    assert!(
        cfg.allowlist.is_empty(),
        "blank env allowlist must shadow TOML and deny all"
    );
}

#[test]
fn absent_allowlist_is_error_no_silent_default() {
    // The allowlist key is required to be *present*. Absence must error —
    // never silently default to "allow everyone".
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(matches!(err, ConfigError::MissingRequired("allowlist")));
}

#[test]
fn present_but_empty_allowlist_is_valid_fail_closed() {
    // Explicitly empty is a *valid* deliberate config: "accept no inbound".
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, ""),
    ]);
    let cfg = SignalConfig::from_sources(&e, None).expect("empty allowlist is valid");
    assert!(cfg.allowlist.is_empty());
}

#[test]
fn invalid_e164_account_is_error() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "5551230000"), // missing '+'
        (ENV_ALLOWLIST, "+15551230001"),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidE164(_)));
}

#[test]
fn invalid_e164_in_allowlist_is_error() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001,not-a-number"),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidE164(_)));
}

#[test]
fn invalid_endpoint_is_error() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1"), // no port
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidEndpoint(_)));
}

#[test]
fn endpoint_rejects_unbracketed_ipv6_and_port_zero() {
    for endpoint in ["::1", "fe80::1", "127.0.0.1:0", "127.0.0.1:+7583"] {
        let e = env(&[
            (ENV_ENDPOINT, endpoint),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidEndpoint(_)),
            "{endpoint} should be rejected, got {err:?}"
        );
    }
}

#[test]
fn endpoint_accepts_bracketed_ipv6() {
    let e = env(&[
        (ENV_ENDPOINT, "[::1]:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
    ]);
    let cfg = SignalConfig::from_sources(&e, None).expect("bracketed IPv6 endpoint is valid");
    assert_eq!(cfg.endpoint, "[::1]:7583");
}

#[test]
fn toml_supplies_values_when_env_absent() {
    let toml = r#"
        endpoint = "10.0.0.5:7583"
        account  = "+15551230000"
        allowlist = ["+15551230001", "+15551230002"]
        own_device_id = 2
        reuse_rolling_group = true
        rolling_group_id = "grp-rolling=="
    "#;
    let cfg = SignalConfig::from_sources(&HashMap::new(), Some(toml)).expect("valid toml");
    assert_eq!(cfg.endpoint, "10.0.0.5:7583");
    assert_eq!(cfg.account, "+15551230000");
    assert_eq!(cfg.allowlist, vec!["+15551230001", "+15551230002"]);
    assert_eq!(cfg.own_device_id, Some(2));
    assert!(cfg.reuse_rolling_group);
    assert_eq!(cfg.rolling_group_id.as_deref(), Some("grp-rolling=="));
}

#[test]
fn toml_own_device_id_string_uses_numeric_validation() {
    let toml = r#"
        endpoint = "127.0.0.1:7583"
        account  = "+15551230000"
        allowlist = ["+15551230001"]
        own_device_id = "3"
    "#;
    let cfg = SignalConfig::from_sources(&HashMap::new(), Some(toml)).expect("valid toml");
    assert_eq!(cfg.own_device_id, Some(3));
}

#[test]
fn string_settings_reject_non_string_toml_scalars() {
    for (setting, toml) in [
        (
            "endpoint",
            r#"
            endpoint = 7583
            account  = "+15551230000"
            allowlist = ["+15551230001"]
        "#,
        ),
        (
            "account",
            r#"
            endpoint = "127.0.0.1:7583"
            account  = true
            allowlist = ["+15551230001"]
        "#,
        ),
        (
            "rolling_group_id",
            r#"
            endpoint = "127.0.0.1:7583"
            account  = "+15551230000"
            allowlist = ["+15551230001"]
            reuse_rolling_group = true
            rolling_group_id = 123
        "#,
        ),
    ] {
        let err = SignalConfig::from_sources(&HashMap::new(), Some(toml)).unwrap_err();
        assert!(
            matches!(err, ConfigError::Toml(_)),
            "expected Toml type error for non-string {setting}, got {err:?}"
        );
    }
}

#[test]
fn toml_allowlist_must_be_an_array() {
    let toml = r#"
        endpoint = "127.0.0.1:7583"
        account  = "+15551230000"
        allowlist = "+15551230001"
    "#;
    let err = SignalConfig::from_sources(&HashMap::new(), Some(toml)).unwrap_err();
    assert!(matches!(err, ConfigError::Toml(_)));
}

#[test]
fn toml_allowlist_entries_must_be_strings() {
    let toml = r#"
        endpoint = "127.0.0.1:7583"
        account  = "+15551230000"
        allowlist = ["+15551230001", 123]
    "#;
    let err = SignalConfig::from_sources(&HashMap::new(), Some(toml)).unwrap_err();
    assert!(matches!(err, ConfigError::Toml(_)));
}

#[test]
fn env_overrides_toml_per_setting() {
    let toml = r#"
        endpoint = "10.0.0.5:7583"
        account  = "+15550000000"
        allowlist = ["+15550000001"]
    "#;
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
    ]);
    let cfg = SignalConfig::from_sources(&e, Some(toml)).unwrap();
    // env wins for endpoint + account; allowlist falls back to TOML.
    assert_eq!(cfg.endpoint, "127.0.0.1:7583");
    assert_eq!(cfg.account, "+15551230000");
    assert_eq!(cfg.allowlist, vec!["+15550000001"]);
}

#[test]
fn reuse_rolling_group_truthy_env_values() {
    for v in ["1", "true"] {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
            (ENV_REUSE_ROLLING_GROUP, v),
            (ENV_ROLLING_GROUP_ID, "grp-rolling=="),
        ]);
        let cfg = SignalConfig::from_sources(&e, None).unwrap();
        assert!(cfg.reuse_rolling_group, "value {v:?} should be truthy");
        assert_eq!(cfg.rolling_group_id.as_deref(), Some("grp-rolling=="));
    }
}

#[test]
fn reuse_rolling_group_falsy_env_values_are_per_session() {
    // Fail-closed: explicit false tokens and empty values must resolve to
    // per-session isolation, never shared.
    for v in ["0", "false", "no", "off", "", "  "] {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
            (ENV_REUSE_ROLLING_GROUP, v),
        ]);
        let cfg = SignalConfig::from_sources(&e, None).unwrap();
        assert!(
            !cfg.reuse_rolling_group,
            "value {v:?} must resolve to per-session (reuse=false)"
        );
    }
}

#[test]
fn unknown_reuse_rolling_group_env_value_is_error() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
        (ENV_REUSE_ROLLING_GROUP, "treu"),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(
        matches!(err, ConfigError::InvalidBool { key, .. } if key == ENV_REUSE_ROLLING_GROUP),
        "expected env-keyed InvalidBool, got {err:?}"
    );
}

#[test]
fn non_boolean_toml_reuse_rolling_group_is_error() {
    let toml = r#"
        endpoint = "127.0.0.1:7583"
        account  = "+15551230000"
        allowlist = ["+15551230001"]
        reuse_rolling_group = "tru"
    "#;
    let err = SignalConfig::from_sources(&HashMap::new(), Some(toml)).unwrap_err();
    assert!(matches!(err, ConfigError::Toml(_)));
}

#[test]
fn absent_reuse_setting_defaults_to_per_session() {
    // With neither env nor TOML specifying the flag, the default MUST be
    // per-session (reuse=false) with no bound rolling group id.
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
    ]);
    let cfg = SignalConfig::from_sources(&e, None).unwrap();
    assert!(!cfg.reuse_rolling_group);
    assert_eq!(cfg.rolling_group_id, None);

    // Same via a TOML file that omits the key entirely.
    let toml = r#"
        endpoint = "127.0.0.1:7583"
        account  = "+15551230000"
        allowlist = ["+15551230001"]
    "#;
    let cfg = SignalConfig::from_sources(&HashMap::new(), Some(toml)).unwrap();
    assert!(!cfg.reuse_rolling_group);
    assert_eq!(cfg.rolling_group_id, None);
}

#[test]
fn reuse_rolling_group_opt_in_via_toml() {
    // Opt-in path: explicit reuse=true + a bound group id in TOML is honored.
    let toml = r#"
        endpoint = "127.0.0.1:7583"
        account  = "+15551230000"
        allowlist = ["+15551230001"]
        reuse_rolling_group = true
        rolling_group_id = "grp-shared=="
    "#;
    let cfg = SignalConfig::from_sources(&HashMap::new(), Some(toml)).unwrap();
    assert!(cfg.reuse_rolling_group);
    assert_eq!(cfg.rolling_group_id.as_deref(), Some("grp-shared=="));
}

#[test]
fn reuse_rolling_group_requires_rolling_group_id() {
    // Without a pinned group id, "rolling" mode cannot actually roll
    // across sessions; it would create a fresh group and skip quitGroup.
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
        (ENV_REUSE_ROLLING_GROUP, "1"),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::MissingRequired("rolling_group_id")
    ));
}

#[test]
fn empty_rolling_group_id_does_not_enable_reuse() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
        (ENV_REUSE_ROLLING_GROUP, "true"),
        (ENV_ROLLING_GROUP_ID, "  "),
    ]);
    let err = SignalConfig::from_sources(&e, None).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::MissingRequired("rolling_group_id")
    ));
}

#[test]
fn malformed_toml_is_error() {
    let err = SignalConfig::from_sources(&HashMap::new(), Some("this = = broken")).unwrap_err();
    assert!(matches!(err, ConfigError::Toml(_)));
}

#[test]
fn whitespace_wrapped_endpoint_and_account_are_trimmed() {
    // File-based secrets / Kubernetes `envFrom` routinely append a trailing
    // newline. Such values must be trimmed and accepted, not rejected as
    // malformed.
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583\n"),
        (ENV_ACCOUNT, "  +15551230000\t"),
        (ENV_ALLOWLIST, "+15551230001"),
    ]);
    let cfg = SignalConfig::from_sources(&e, None).expect("whitespace-wrapped values are valid");
    assert_eq!(cfg.endpoint, "127.0.0.1:7583");
    assert_eq!(cfg.account, "+15551230000");
}

#[test]
fn whitespace_wrapped_toml_string_settings_are_trimmed() {
    let toml = r#"
        endpoint = "127.0.0.1:7583\n"
        account  = " +15551230000 "
        allowlist = ["+15551230001"]
        reuse_rolling_group = true
        rolling_group_id = "  grp-rolling==\n"
    "#;
    let cfg = SignalConfig::from_sources(&HashMap::new(), Some(toml)).expect("valid toml");
    assert_eq!(cfg.endpoint, "127.0.0.1:7583");
    assert_eq!(cfg.account, "+15551230000");
    assert_eq!(cfg.rolling_group_id.as_deref(), Some("grp-rolling=="));
}

#[test]
fn blank_endpoint_env_reports_missing_not_invalid() {
    // A whitespace-only (effectively empty) required value must surface as a
    // clear MissingRequired rather than a confusing "invalid endpoint" for a
    // blank string.
    for blank in ["", "  ", "\n", "\t"] {
        let e = env(&[
            (ENV_ENDPOINT, blank),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(
            matches!(err, ConfigError::MissingRequired("endpoint")),
            "blank endpoint {blank:?} should be MissingRequired, got {err:?}"
        );
    }
}

#[test]
fn blank_account_env_reports_missing_not_invalid() {
    for blank in ["", "  ", "\n"] {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, blank),
            (ENV_ALLOWLIST, "+15551230001"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(
            matches!(err, ConfigError::MissingRequired("account")),
            "blank account {blank:?} should be MissingRequired, got {err:?}"
        );
    }
}

#[test]
fn empty_own_device_id_env_is_treated_as_unset() {
    // An empty/whitespace env override must not hard-fail as an unparseable
    // number; it should be treated as unset (own_device_id is optional).
    for blank in ["", "  ", "\n"] {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
            (ENV_OWN_DEVICE_ID, blank),
        ]);
        let cfg = SignalConfig::from_sources(&e, None).unwrap_or_else(|err| {
            panic!("blank own_device_id {blank:?} should be unset, got {err:?}")
        });
        assert_eq!(cfg.own_device_id, None, "blank {blank:?} should yield None");
    }
}

#[test]
fn empty_own_device_id_env_falls_through_to_toml() {
    // An empty env override is "unset", so a valid TOML value still applies.
    let toml = r#"
        own_device_id = 3
    "#;
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
        (ENV_OWN_DEVICE_ID, ""),
    ]);
    let cfg = SignalConfig::from_sources(&e, Some(toml)).expect("valid config");
    assert_eq!(cfg.own_device_id, Some(3));
}

#[test]
fn whitespace_wrapped_own_device_id_env_is_parsed() {
    let e = env(&[
        (ENV_ENDPOINT, "127.0.0.1:7583"),
        (ENV_ACCOUNT, "+15551230000"),
        (ENV_ALLOWLIST, "+15551230001"),
        (ENV_OWN_DEVICE_ID, "  3\n"),
    ]);
    let cfg = SignalConfig::from_sources(&e, None).expect("valid config");
    assert_eq!(cfg.own_device_id, Some(3));
}

#[test]
fn signed_own_device_id_env_is_invalid() {
    for value in ["+3", "-3"] {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
            (ENV_OWN_DEVICE_ID, value),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidNumber { key, .. } if key == ENV_OWN_DEVICE_ID),
            "signed own_device_id {value:?} should be invalid, got {err:?}"
        );
    }
}
