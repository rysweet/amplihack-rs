use super::recovery_tests::interrupted;
use super::*;

#[test]
fn uninstall_inventory_resumes_partial_cleanup_and_preserves_foreign_survivors() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = home.join(".amplihack/codex");
    let codex_home = home.join(".codex");
    let package = root.join("uninstall-package");
    fs::create_dir_all(package.join("nested")).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(package.join("nested/owned.txt"), b"owned").unwrap();
    fs::write(package.join("keep.txt"), b"also owned").unwrap();
    let _env = crate::test_support::EnvGuard::set([("HOME", home.to_str().unwrap())]);

    path_scope::with_scope(&root, &codex_home, || {
        let inventory = package_cleanup::capture(&package)?;
        fs::remove_file(package.join("nested/owned.txt"))?;
        package_cleanup::validate_survivors(&package, &inventory)?;
        fs::write(package.join("foreign.txt"), b"preserve me")?;
        assert!(package_cleanup::remove_survivors(&package, &inventory).is_err());
        assert_eq!(fs::read(package.join("foreign.txt"))?, b"preserve me");

        fs::remove_file(package.join("foreign.txt"))?;
        package_cleanup::remove_survivors(&package, &inventory)?;
        assert!(!package.exists());
        Ok(())
    })
    .unwrap();
}

pub(super) fn committed_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let (root, home, mut pending) = interrupted(dir, true);
    fs::create_dir(root.join("previous-package/nested")).unwrap();
    fs::write(root.join("previous-package/nested/second"), b"second").unwrap();
    pending["ledger"]["package_digest"] = json!(digest(&root.join("previous-package")).unwrap());
    let ledger = json!({"schema_version":1,"codex_home":home,"transaction":"test",
        "package_digest":pending["target_digest"],"hooks":{}});
    pending["expected"]["ledger"] = json!(json_bytes(&ledger).unwrap());
    fs::write(root.join("ownership.json"), json_bytes(&ledger).unwrap()).unwrap();
    fs::write(root.join("pending.json"), json_bytes(&pending).unwrap()).unwrap();
    (root, home)
}

fn interrupt_authorized_unlink(root: &Path, home: &Path) {
    let mut stopped = false;
    let result = storage::testing::with_sync_hook(
        |path| {
            let backup = root.join("previous-package");
            if path.starts_with(&backup)
                && (!backup.join("resource").exists() || !backup.join("nested/second").exists())
            {
                stopped = true;
                anyhow::bail!("interrupted after actual authorized unlink");
            }
            Ok(())
        },
        || recover_install(root, Path::new("must-not-spawn"), home),
    );
    assert!(result.is_err(), "cleanup must expose injected interruption");
    assert!(
        stopped,
        "production cleanup must sync after an actual unlink"
    );
    let pending = regular_json(&root.join("pending.json")).unwrap().unwrap();
    assert!(
        pending["backup_cleanup"].is_object(),
        "authorization must precede unlink"
    );
    assert!(root.join("previous-package").is_dir());
}

#[test]
fn committed_cleanup_resumes_after_actual_authorized_unlink() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    interrupt_authorized_unlink(&root, &home);
    recover_install(&root, Path::new("must-not-spawn"), &home).unwrap();
    recover_install(&root, Path::new("must-not-spawn"), &home).unwrap();
    assert!(!root.join("previous-package").exists());
    assert!(!root.join("pending.json").exists());
    assert_eq!(
        fs::read(root.join("market/plugin/resource")).unwrap(),
        b"target"
    );
}

#[test]
fn full_backup_control_and_proof_free_partial_backup() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for partial in [false, true] {
        cases::partial_backup_case(partial);
    }
}

#[test]
fn cleanup_proof_directory_barrier_failure_prevents_every_unlink() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    let before = digest(&root.join("previous-package")).unwrap();
    let result = storage::testing::with_sync_hook(
        |path| {
            if path == root
                && regular_json(&root.join("pending.json"))?.unwrap()["backup_cleanup"].is_object()
            {
                anyhow::bail!("injected proof publication barrier failure");
            }
            Ok(())
        },
        || recover_install(&root, Path::new("must-not-spawn"), &home),
    );
    assert!(format!("{:#}", result.unwrap_err()).contains("proof publication barrier"));
    assert_eq!(digest(&root.join("previous-package")).unwrap(), before);
    assert!(root.join("pending.json").exists());
    // Visible proof from a failed barrier must be synchronized again before retry deletes.
    recover_install(&root, Path::new("must-not-spawn"), &home).unwrap();
}

#[test]
fn cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for change in [
        "file",
        "injected",
        "type",
        "path",
        "duplicate",
        "transaction",
        "home",
        "digest",
        "shape",
        "schema",
        "null",
        "absolute",
        "hash",
        "parent",
        "ledger",
        "live",
        "hooks",
    ] {
        cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case(change);
    }
}

fn cleanup_rejects_changed_survivors_and_invalid_inventory_with_evidence_intact_case(change: &str) {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    interrupt_authorized_unlink(&root, &home);
    // The shallow resource survives the first unlink of nested/second.
    let resource = root.join("previous-package/resource");
    assert!(resource.is_file());
    let journal = root.join("pending.json");
    let mut pending = regular_json(&journal).unwrap().unwrap();
    match change {
        "file" => fs::write(&resource, b"foreign").unwrap(),
        "injected" => fs::write(root.join("previous-package/new"), b"foreign").unwrap(),
        "type" => {
            fs::remove_file(&resource).unwrap();
            fs::create_dir(&resource).unwrap();
        }
        "path" => pending["backup_cleanup"]["entries"][0]["path"] = json!("../outside"),
        "duplicate" => {
            let entry = pending["backup_cleanup"]["entries"][0].clone();
            pending["backup_cleanup"]["entries"]
                .as_array_mut()
                .unwrap()
                .push(entry);
        }
        "transaction" => pending["backup_cleanup"]["transaction"] = json!("another"),
        "home" => pending["backup_cleanup"]["codex_home"] = json!(dir.path()),
        "digest" => pending["backup_cleanup"]["original_digest"] = json!("0".repeat(64)),
        "shape" => pending["backup_cleanup"]["extra"] = json!(true),
        "schema" => pending["backup_cleanup"]["schema_version"] = json!(2),
        "null" => pending["backup_cleanup"] = Value::Null,
        "absolute" => pending["backup_cleanup"]["entries"][0]["path"] = json!("/outside"),
        "hash" => {
            let entries = pending["backup_cleanup"]["entries"].as_array_mut().unwrap();
            let file = entries
                .iter_mut()
                .find(|e| e["kind"]["type"] == "File")
                .unwrap();
            file["kind"]["sha256"] = json!("g".repeat(64));
        }
        "parent" => pending["backup_cleanup"]["entries"]
            .as_array_mut()
            .unwrap()
            .retain(|e| e["path"] != "nested"),
        "ledger" => fs::write(root.join("ownership.json"), b"{}").unwrap(),
        "live" => fs::write(root.join("market/plugin/resource"), b"foreign").unwrap(),
        "hooks" => fs::write(home.join("hooks.json"), b"foreign").unwrap(),
        _ => unreachable!(),
    }
    fs::write(&journal, json_bytes(&pending).unwrap()).unwrap();
    let before = fs::read(&journal).unwrap();
    assert!(
        recover_install(&root, Path::new("must-not-spawn"), &home).is_err(),
        "{change}"
    );
    assert_eq!(fs::read(&journal).unwrap(), before, "{change}");
    assert!(fs::symlink_metadata(&resource).is_ok(), "{change}");
}

mod durability;
mod independent;

mod cases;
