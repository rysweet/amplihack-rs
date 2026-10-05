use super::*;

fn interrupted(dir: &Path, had_package: bool) -> (PathBuf, PathBuf, Value) {
    let root = dir.join("root");
    let home = dir.join("home");
    fs::create_dir_all(root.join("market/plugin")).unwrap();
    fs::create_dir_all(root.join("market/.agents/plugins")).unwrap();
    fs::create_dir(&home).unwrap();
    fs::write(root.join("market/plugin/resource"), b"target").unwrap();
    let old_digest = if had_package {
        fs::create_dir(root.join("previous-package")).unwrap();
        fs::write(root.join("previous-package/resource"), b"original").unwrap();
        Some(digest(&root.join("previous-package")).unwrap())
    } else {
        None
    };
    let ledger = old_digest.map(|package_digest| {
        json!({"schema_version":1,"codex_home":home,
        "transaction":"previous","package_digest":package_digest,"hooks":{}})
    });
    let original_ledger = ledger.as_ref().map(|v| json_bytes(v).unwrap());
    if let Some(bytes) = &original_ledger {
        fs::write(root.join("ownership.json"), bytes).unwrap();
    }
    let pending = json!({"schema_version":2,"codex_home":home,"transaction":"test",
        "installed":false,"had_package":had_package,"config":null,"ledger":ledger,
        "target_digest":digest(&root.join("market/plugin")).unwrap(),
        "snapshots":{"hooks":null,"marketplace":null,"ledger":original_ledger},
        "expected":{"config":null,"hooks":json_bytes(&json!({"hooks":{}})).unwrap(),
        "marketplace":json_bytes(&json!({"plugins":[]})).unwrap(),"ledger":original_ledger}});
    fs::write(
        home.join("hooks.json"),
        json_bytes(&json!({"hooks":{}})).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("market/.agents/plugins/marketplace.json"),
        json_bytes(&json!({"plugins":[]})).unwrap(),
    )
    .unwrap();
    atomic_json(&root.join("pending.json"), &pending, None).unwrap();
    (root, home, pending)
}

#[test]
fn interrupted_recovery_preserves_foreign_resources_before_native_or_package_changes() {
    for resource in [
        "config",
        "hooks",
        "package",
        "backup",
        "marketplace",
        "ledger",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (root, home, _) = interrupted(dir.path(), true);
        let path = match resource {
            "config" => home.join("config.toml"),
            "hooks" => home.join("hooks.json"),
            "package" => root.join("market/plugin/resource"),
            "backup" => root.join("previous-package/resource"),
            "marketplace" => root.join("market/.agents/plugins/marketplace.json"),
            _ => root.join("ownership.json"),
        };
        fs::write(&path, b"foreign exact bytes").unwrap();
        let error = recover_install(&root, &dir.path().join("must-not-spawn"), &home).unwrap_err();
        assert!(format!("{error:#}").contains("retained"), "{error:#}");
        assert_eq!(fs::read(path).unwrap(), b"foreign exact bytes");
        assert!(root.join("pending.json").is_file());
        assert!(root.join("previous-package").is_dir());
        assert!(root.join("market/plugin").is_dir());
    }
}

#[cfg(unix)]
#[test]
fn interrupted_recovery_restores_absence_and_bytes_and_is_repeatable() {
    use std::os::unix::fs::PermissionsExt;
    for had_package in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (root, home, _) = interrupted(dir.path(), had_package);
        let binary = dir.path().join("native");
        fs::write(&binary, "#!/bin/sh\nprintf '{\"installed\":[]}'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        recover_install(&root, &binary, &home).unwrap();
        recover_install(&root, &binary, &home).unwrap();
        assert!(!home.join("hooks.json").exists());
        assert!(!home.join("config.toml").exists());
        assert!(!root.join("pending.json").exists());
        if had_package {
            assert_eq!(
                fs::read(root.join("market/plugin/resource")).unwrap(),
                b"original"
            );
        } else {
            assert!(!root.join("market/plugin").exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn interrupted_recovery_preserves_config_symlink_and_legacy_journal() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home, mut pending) = interrupted(dir.path(), false);
    let outside = dir.path().join("outside");
    fs::write(&outside, b"foreign").unwrap();
    std::os::unix::fs::symlink(&outside, home.join("config.toml")).unwrap();
    assert!(recover_install(&root, Path::new("must-not-spawn"), &home).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"foreign");
    fs::remove_file(home.join("config.toml")).unwrap();
    pending["schema_version"] = json!(1);
    fs::write(
        root.join("pending.json"),
        serde_json::to_vec(&pending).unwrap(),
    )
    .unwrap();
    assert!(recover_install(&root, Path::new("must-not-spawn"), &home).is_err());
    assert!(root.join("market/plugin").exists());
    assert!(root.join("pending.json").exists());
}

#[test]
fn committed_cleanup_preserves_modified_backup_and_pending_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home, mut pending) = interrupted(dir.path(), true);
    let ledger = json!({"schema_version":1,"codex_home":home,"transaction":"test",
        "package_digest":pending["target_digest"],"hooks":{}});
    let bytes = json_bytes(&ledger).unwrap();
    pending["expected"]["ledger"] = json!(bytes);
    fs::write(root.join("ownership.json"), bytes).unwrap();
    fs::write(
        root.join("pending.json"),
        serde_json::to_vec(&pending).unwrap(),
    )
    .unwrap();
    fs::write(root.join("previous-package/resource"), b"foreign backup").unwrap();
    assert!(recover_install(&root, Path::new("must-not-spawn"), &home).is_err());
    assert_eq!(
        fs::read(root.join("previous-package/resource")).unwrap(),
        b"foreign backup"
    );
    assert!(root.join("pending.json").exists());
}

#[cfg(unix)]
#[test]
fn partially_restored_recovery_preserves_exact_original_file_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (root, home, mut pending) = interrupted(dir.path(), true);
    let config = b"# original preferences\napproval_policy = 'on-request'\n";
    let hooks = b"\n{\"hooks\":{}}\n\n";
    pending["config"] = json!(config.to_vec());
    pending["expected"]["config"] = pending["config"].clone();
    pending["snapshots"]["hooks"] = json!(hooks.to_vec());
    // Package and hooks were already restored before a second interruption.
    fs::remove_dir_all(root.join("market/plugin")).unwrap();
    fs::rename(root.join("previous-package"), root.join("market/plugin")).unwrap();
    fs::write(home.join("config.toml"), config).unwrap();
    fs::write(home.join("hooks.json"), hooks).unwrap();
    fs::write(
        root.join("pending.json"),
        serde_json::to_vec(&pending).unwrap(),
    )
    .unwrap();
    let binary = dir.path().join("native");
    fs::write(&binary, "#!/bin/sh\nprintf '{\"installed\":[]}'\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    recover_install(&root, &binary, &home).unwrap();
    assert_eq!(fs::read(home.join("config.toml")).unwrap(), config);
    assert_eq!(fs::read(home.join("hooks.json")).unwrap(), hooks);
    assert!(!root.join("pending.json").exists());
}

#[cfg(unix)]
#[test]
fn native_config_transition_rolls_back_exact_bytes_and_rejects_foreign_edits() {
    use std::os::unix::fs::PermissionsExt;
    for foreign in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (root, home, mut pending) = interrupted(dir.path(), false);
        let original =
            b"# preserve comment\napproval_policy = 'on-request'\nsandbox_mode = 'read-only'\n";
        let market = root.join("market");
        let registered = format!(
            "{}\n[marketplaces.amplihack-local]\nsource_type = \"local\"\nsource = \"{}\"\n\n[plugins.\"amplihack@amplihack-local\"]\nenabled = true\n",
            std::str::from_utf8(original).unwrap(),
            market.display()
        );
        pending["config"] = json!(original.to_vec());
        pending["expected"]["config"] = pending["config"].clone();
        pending["config_states"] =
            json!(config::config_states(Some(original), &market, false).unwrap());
        assert!(config::config_matches(
            &pending,
            &json!(registered.as_bytes())
        ));
        let current = if foreign {
            registered.replace("read-only", "danger-full-access")
        } else {
            registered
        };
        fs::write(home.join("config.toml"), &current).unwrap();
        atomic_json(
            &root.join("pending.json"),
            &pending,
            regular_json(&root.join("pending.json")).unwrap(),
        )
        .unwrap();
        let binary = dir.path().join("native");
        fs::write(&binary, "#!/bin/sh\nprintf '{\"installed\":[]}'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let result = recover_install(&root, &binary, &home);
        if foreign {
            assert!(result.is_err());
            assert_eq!(
                fs::read_to_string(home.join("config.toml")).unwrap(),
                current
            );
            assert!(root.join("pending.json").exists());
            assert!(root.join("market/plugin").exists());
        } else {
            result.unwrap();
            assert_eq!(fs::read(home.join("config.toml")).unwrap(), original);
            assert!(!root.join("pending.json").exists());
            assert!(!root.join("market/plugin").exists());
        }
    }
}
