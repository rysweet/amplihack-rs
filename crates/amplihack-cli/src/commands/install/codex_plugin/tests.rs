use super::*;

#[cfg(unix)]
#[test]
fn native_failure_diagnostics_do_not_expose_child_output() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("native");
    fs::write(&binary, "#!/bin/sh\nprintf '\\033[31mAuthorization: Bearer secret-canary' >&2\nprintf 'prompt-canary'\nexit 7\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let error = native(&binary, &["plugin", "list", "--json"], dir.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("native plugin command failed"));
    assert!(!error.contains("canary"));
    assert!(!error.contains('\u{1b}'));
}

#[test]
fn derived_production_instructions_are_portable() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = tempfile::tempdir().unwrap();
    instruction_skills(
        &source.join("amplifier-bundle/agents"),
        output.path(),
        "persona",
    )
    .unwrap();
    instruction_skills(
        &source.join("docs/claude/commands/amplihack"),
        output.path(),
        "command",
    )
    .unwrap();
    let mut count = 0;
    for entry in fs::read_dir(output.path()).unwrap() {
        let content = fs::read_to_string(entry.unwrap().path().join("SKILL.md")).unwrap();
        let body = content.split_once("\n---\n").unwrap().1;
        assert!(
            !body.trim_start().starts_with("---"),
            "embedded runtime frontmatter"
        );
        assert!(!body.contains("Use GPT-4 to analyze code + comments"));
        count += 1;
    }
    assert_eq!(count, 66);
}

#[test]
fn codex_hooks_reconcile_preserves_foreign_definitions_across_update_and_removal() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    let foreign =
        json!({"matcher":"foreign","hooks":[{"type":"command","command":"echo user-owned"}]});
    let initial = json!({"user_metadata":{"retain":true},"hooks":{"SessionStart":[foreign]}});
    fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
    let owned =
        json!({"SessionStart":[{"hooks":[{"type":"command","command":"amplihack-managed-hook"}]}]});
    reconcile_hooks(dir.path(), &json!({}), &owned).unwrap();
    reconcile_hooks(dir.path(), &owned, &owned).unwrap();
    assert_eq!(
        regular_json(&path).unwrap().unwrap()["hooks"]["SessionStart"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    reconcile_hooks(dir.path(), &owned, &json!({})).unwrap();
    assert_eq!(regular_json(&path).unwrap().unwrap(), initial);
}

#[test]
fn codex_hooks_malformed_json_and_unowned_collisions_are_preserved() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    fs::write(&path, "{broken").unwrap();
    assert!(reconcile_hooks(dir.path(), &json!({}), &json!({})).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
    let owned = json!({"SessionStart":[{"hooks":[{"type":"command","command":"identical"}]}]});
    let original = json!({"hooks":owned});
    fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    assert!(reconcile_hooks(dir.path(), &json!({}), &owned).is_err());
    assert_eq!(regular_json(&path).unwrap().unwrap(), original);
}

#[cfg(unix)]
#[test]
fn codex_resource_and_config_symlink_escape_are_refused() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let staged = dir.path().join("staged");
    fs::create_dir(&source).unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, "foreign").unwrap();
    std::os::unix::fs::symlink("../outside", source.join("resource")).unwrap();
    assert!(copy_tree(&source, &staged, &source, 0).is_err());
    let home = dir.path().join("home");
    fs::create_dir(&home).unwrap();
    std::os::unix::fs::symlink(&outside, home.join("hooks.json")).unwrap();
    assert!(reconcile_hooks(&home, &json!({}), &json!({})).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "foreign");
}

#[test]
fn malformed_recovery_snapshots_are_rejected_before_mutation() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let home = dir.path().join("home");
    fs::create_dir_all(root.join("market/plugin")).unwrap();
    fs::create_dir(&home).unwrap();
    let resource = root.join("market/plugin/resource");
    fs::write(&resource, b"retain exact bytes").unwrap();
    for snapshots in [
        json!({}),
        json!({"hooks":[256],"ledger":null,"marketplace":null}),
    ] {
        let pending = json!({"schema_version":1,"codex_home":home,"transaction":"test",
            "installed":false,"had_package":true,"config":null,"ledger":null,"snapshots":snapshots});
        fs::write(
            root.join("pending.json"),
            serde_json::to_vec(&pending).unwrap(),
        )
        .unwrap();
        assert!(recover_install(&root, &dir.path().join("must-not-spawn"), &home).is_err());
        assert_eq!(fs::read(&resource).unwrap(), b"retain exact bytes");
        assert!(root.join("pending.json").is_file());
    }
}

#[test]
fn resolved_native_binary_survives_nested_selection_and_failure() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let outer = Path::new("/already/resolved/codex");
    let inner = Path::new("/nested/codex");
    with_binary(outer, || {
        assert_eq!(selected_binary().unwrap().as_deref(), Some(outer));
        let result = std::panic::catch_unwind(|| {
            with_binary(inner, || {
                assert_eq!(selected_binary().unwrap().as_deref(), Some(inner));
                panic!("controlled failed preparation");
            })
        });
        assert!(result.is_err());
        assert_eq!(selected_binary().unwrap().as_deref(), Some(outer));
    });
}
