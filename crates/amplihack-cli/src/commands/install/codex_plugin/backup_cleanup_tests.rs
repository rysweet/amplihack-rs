use super::recovery_tests::interrupted;
use super::*;

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
    for partial in [false, true] {
        cases::partial_backup_case(partial);
    }
}

#[test]
fn cleanup_proof_directory_barrier_failure_prevents_every_unlink() {
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

#[cfg(unix)]
#[test]
fn cleanup_checks_symlink_targets_and_never_follows_them() {
    for substitute in [false, true] {
        cases::backup_link_case(substitute);
    }
}

#[test]
fn backup_removal_sync_failure_keeps_journal_and_can_retry() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    let result = storage::testing::with_sync_hook(
        |path| {
            if path == root && !root.join("previous-package").exists() {
                assert!(root.join("pending.json").is_file());
                anyhow::bail!("interrupted after backup removal before journal removal");
            }
            Ok(())
        },
        || recover_install(&root, Path::new("must-not-spawn"), &home),
    );
    assert!(result.is_err());
    assert!(!root.join("previous-package").exists());
    assert!(root.join("pending.json").exists());
    recover_install(&root, Path::new("must-not-spawn"), &home).unwrap();
    recover_install(&root, Path::new("must-not-spawn"), &home).unwrap();
    assert!(!root.join("pending.json").exists());
}

#[test]
fn proof_publication_rejects_concurrent_journal_edit_before_unlink() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    let before = digest(&root.join("previous-package")).unwrap();
    let journal = root.join("pending.json");
    let mut pending = regular_json(&journal).unwrap().unwrap();
    let mut changed = pending.clone();
    changed["foreign"] = json!("concurrent edit");
    fs::write(&journal, json_bytes(&changed).unwrap()).unwrap();
    let result = backup_cleanup::finish(&mut pending, &root, &home);
    assert!(format!("{:#}", result.unwrap_err()).contains("changed concurrently"));
    assert_eq!(digest(&root.join("previous-package")).unwrap(), before);
    assert_eq!(regular_json(&journal).unwrap().unwrap(), changed);
}

#[cfg(unix)]
#[test]
fn authorized_cleanup_rejects_directory_symlink_substitution() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    interrupt_authorized_unlink(&root, &home);
    let nested = root.join("previous-package/nested");
    fs::remove_dir(&nested).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("second"), b"foreign").unwrap();
    std::os::unix::fs::symlink(&outside, &nested).unwrap();
    assert!(recover_install(&root, Path::new("must-not-spawn"), &home).is_err());
    assert_eq!(fs::read(outside.join("second")).unwrap(), b"foreign");
    assert!(root.join("pending.json").exists());
    assert!(
        fs::symlink_metadata(nested)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn unsupported_backup_fifo_is_rejected_without_opening_it() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    let path = root.join("previous-package/fifo");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let error = recover_install(&root, Path::new("must-not-spawn"), &home).unwrap_err();
    assert!(format!("{error:#}").contains("unsupported backup entry"));
    assert!(root.join("pending.json").exists());
    assert!(path.exists());
}

mod independent;

mod cases;
