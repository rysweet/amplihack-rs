use super::*;

#[test]
fn unchanged_owned_install_skips_transaction_and_package_replacement() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("home/.amplihack/codex");
    let home = dir.path().join("home/.codex");
    let package = root.join("market/plugin");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let hooks_binary = dir.path().join("amplihack-hooks");
    let binary = dir.path().join("native");
    fs::write(&hooks_binary, b"hooks").unwrap();
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\nprintf '{{\"installed\":[{{\"pluginId\":\"{ID}\",\"installed\":true,\"source\":{{\"path\":\"{}\"}}}}]}}'\n",
            package.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir_all(&package).unwrap();
    fs::create_dir_all(&home).unwrap();
    build(&source, &hooks_binary, &package).unwrap();

    let hooks = declarations(&package).unwrap();
    fs::write(
        home.join("hooks.json"),
        serde_json::to_vec(&json!({"hooks":hooks})).unwrap(),
    )
    .unwrap();
    let market = root.join("market");
    fs::create_dir_all(market.join(".agents/plugins")).unwrap();
    fs::write(
        market.join(".agents/plugins/marketplace.json"),
        serde_json::to_vec(&json!({
            "name":"amplihack-local",
            "plugins":[{"name":"amplihack", "source":{"source":"local","path":"./plugin"},
                "policy":{"installation":"AVAILABLE","authentication":"ON_INSTALL"}, "category":"Productivity"}]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("ownership.json"),
        json_bytes(&json!({
            "schema_version":1,
            "transaction":"installed-once",
            "codex_home":home,
            "package_digest":digest(&package).unwrap(),
            "hooks":hooks
        }))
        .unwrap(),
    )
    .unwrap();

    let prepared = path_scope::with_scope(&root, &home, || {
        prepare(&root, &home, &binary, &source, &hooks_binary)
    })
    .unwrap();

    assert!(prepared.already_current);
    assert!(!root.join("pending.json").exists());
    assert!(!root.join("previous-package").exists());
}
