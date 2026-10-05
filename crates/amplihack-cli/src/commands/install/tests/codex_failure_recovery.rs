use super::super::codex_plugin;
use super::helpers::{create_exe_stub, create_source_repo};
use crate::test_support::{EnvGuard, home_env_lock};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn failed_upgrade_restores_package_ledger_and_hooks() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let source = home.join("source");
    create_source_repo(&source);
    let bin = home.join("bin");
    let binary = create_exe_stub(&bin, "codex");
    let hooks = create_exe_stub(&bin, "amplihack-hooks");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let _env = EnvGuard::set([
        ("HOME", home.to_str().unwrap()),
        ("CODEX_HOME", home.join(".codex").to_str().unwrap()),
        ("PATH", &path),
        ("AMPLIHACK_CODEX_BINARY_PATH", binary.to_str().unwrap()),
        ("AMPLIHACK_AGENT_BINARY", "codex"),
    ]);
    codex_plugin::install(&source, &hooks).unwrap();
    let ledger = home.join(".amplihack/codex/ownership.json");
    let before = fs::read(&ledger).unwrap();
    let before_hooks = fs::read(home.join(".codex/hooks.json")).unwrap();
    let package = home.join(".amplihack/codex/market/plugin");
    let before_package = fs::read(package.join("plugin.json")).unwrap();
    let script = fs::read_to_string(&binary).unwrap();
    fs::write(
        &binary,
        script.replace(
            "'plugin marketplace add')",
            "'plugin marketplace add') exit 17;;\n  'never')",
        ),
    )
    .unwrap();
    assert!(codex_plugin::install(&source, &hooks).is_err());
    assert_eq!(fs::read(&ledger).unwrap(), before);
    assert_eq!(
        fs::read(home.join(".codex/hooks.json")).unwrap(),
        before_hooks
    );
    assert_eq!(
        fs::read(package.join("plugin.json")).unwrap(),
        before_package
    );
    assert!(!home.join(".amplihack/codex/pending.json").exists());
    // Simulate interruption after replacing the package, before committing its ledger.
    let root = home.join(".amplihack/codex");
    fs::rename(&package, root.join("previous-package")).unwrap();
    fs::create_dir(&package).unwrap();
    fs::write(package.join("plugin.json"), "interrupted replacement").unwrap();
    let pending = json!({"schema_version":1,"codex_home":home.join(".codex"),"transaction":"interrupted", "installed":true, "had_package":true,
        "config":fs::read(home.join(".codex/config.toml")).ok(),
        "hooks":serde_json::from_slice::<serde_json::Value>(&before_hooks).unwrap(),
        "ledger":serde_json::from_slice::<serde_json::Value>(&before).unwrap(),
        "marketplace":serde_json::from_slice::<serde_json::Value>(&fs::read(root.join("market/.agents/plugins/marketplace.json")).unwrap()).unwrap()});
    fs::write(
        root.join("pending.json"),
        serde_json::to_vec(&pending).unwrap(),
    )
    .unwrap();
    fs::remove_dir_all(source.join("amplifier-bundle/skills")).unwrap();
    assert!(codex_plugin::install(&source, &hooks).is_err());
    assert_eq!(
        fs::read(package.join("plugin.json")).unwrap(),
        before_package
    );
    assert!(!root.join("pending.json").exists());
}

#[test]
fn identical_unowned_hook_failed_retry_and_uninstall_preserve_foreign_hook() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let source = home.join("source");
    create_source_repo(&source);
    let bin = home.join("bin");
    let binary = create_exe_stub(&bin, "codex");
    let hooks = create_exe_stub(&bin, "amplihack-hooks");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let codex_home = home.join(".codex");
    fs::create_dir(&codex_home).unwrap();
    let foreign = json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":format!("'{}' pre-tool-use",home.join(".amplihack/codex/market/plugin/bin/hook").display())}]}]}});
    let hooks_path = codex_home.join("hooks.json");
    fs::write(&hooks_path, serde_json::to_vec(&foreign).unwrap()).unwrap();
    let _env = EnvGuard::set([
        ("HOME", home.to_str().unwrap()),
        ("CODEX_HOME", codex_home.to_str().unwrap()),
        ("PATH", &path),
        ("AMPLIHACK_CODEX_BINARY_PATH", binary.to_str().unwrap()),
        ("AMPLIHACK_AGENT_BINARY", "codex"),
    ]);
    for _ in 0..2 {
        assert!(codex_plugin::install(&source, &hooks).is_err());
    }
    codex_plugin::uninstall().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(hooks_path).unwrap()).unwrap(),
        foreign
    );
    assert!(!home.join(".amplihack/codex/ownership.json").exists());
}

#[test]
fn unsupported_optional_codex_skips_without_persistent_writes() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("codex");
    fs::write(
        &bin,
        "#!/bin/sh\nprintf '\x1b[31mtoken-secret-canary' >&2\nexit 2\n",
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    let _env = EnvGuard::set([
        ("HOME", dir.path().to_str().unwrap()),
        ("CODEX_HOME", dir.path().join(".codex").to_str().unwrap()),
        ("AMPLIHACK_CODEX_BINARY_PATH", bin.to_str().unwrap()),
        ("PATH", dir.path().to_str().unwrap()),
        ("AMPLIHACK_AGENT_BINARY", "claude"),
    ]);
    assert!(!codex_plugin::install(dir.path(), &bin).unwrap());
    assert!(!dir.path().join(".amplihack/codex").exists());
    unsafe {
        std::env::set_var("AMPLIHACK_AGENT_BINARY", "codex");
    }
    let error = codex_plugin::install(dir.path(), &bin)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("token-secret-canary"));
    assert!(!dir.path().join(".amplihack/codex").exists());
}

#[test]
fn hook_and_ledger_write_failures_retain_recovery_until_retry() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for relative in [".codex/hooks.json", ".amplihack/codex/ownership.json"] {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let source = home.join("source");
        create_source_repo(&source);
        let bin = home.join("bin");
        let binary = create_exe_stub(&bin, "codex");
        let hooks = create_exe_stub(&bin, "amplihack-hooks");
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let _env = EnvGuard::set([
            ("HOME", home.to_str().unwrap()),
            ("CODEX_HOME", home.join(".codex").to_str().unwrap()),
            ("PATH", &path),
            ("AMPLIHACK_AGENT_BINARY", "codex"),
        ]);
        codex_plugin::install(&source, &hooks).unwrap();
        let script = fs::read_to_string(&binary).unwrap();
        // Inject a filesystem write failure from the native subprocess after preflight.
        let fault = format!(
            "'plugin marketplace add') mv \"$HOME/{relative}\" \"$HOME/{relative}.saved\"; mkdir \"$HOME/{relative}\";"
        );
        fs::write(&binary, script.replace("'plugin marketplace add')", &fault)).unwrap();
        assert!(codex_plugin::install(&source, &hooks).is_err());
        assert!(home.join(".amplihack/codex/pending.json").exists());
        assert!(
            home.join(".amplihack/codex/market/plugin/plugin.json")
                .exists()
        );
        fs::remove_dir(home.join(relative)).unwrap();
        fs::rename(home.join(format!("{relative}.saved")), home.join(relative)).unwrap();
        fs::write(&binary, script).unwrap();
        codex_plugin::install(&source, &hooks).unwrap();
        assert!(!home.join(".amplihack/codex/pending.json").exists());
    }
}
