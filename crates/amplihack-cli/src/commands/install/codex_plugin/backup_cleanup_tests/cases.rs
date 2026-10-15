//! Shared original scenario bodies for independent exact selections.
use super::*;

pub(super) fn partial_backup_case(partial: bool) {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    if partial {
        fs::remove_file(root.join("previous-package/resource")).unwrap();
    }
    let result = recover_install(&root, Path::new("must-not-spawn"), &home);
    if partial {
        assert!(
            result.is_err(),
            "historical partial backups require reconciliation"
        );
        assert!(root.join("pending.json").exists());
        assert_eq!(
            fs::read(root.join("previous-package/nested/second")).unwrap(),
            b"second"
        );
    } else {
        result.unwrap();
        assert!(!root.join("pending.json").exists());
    }
}

#[cfg(unix)]
pub(super) fn backup_link_case(substitute: bool) {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = committed_fixture(dir.path());
    let outside = dir.path().join("outside");
    fs::write(&outside, b"outside untouched").unwrap();
    let link = root.join("previous-package/link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let journal = root.join("pending.json");
    let mut pending = regular_json(&journal).unwrap().unwrap();
    pending["ledger"]["package_digest"] = json!(digest(&root.join("previous-package")).unwrap());
    fs::write(&journal, json_bytes(&pending).unwrap()).unwrap();
    interrupt_authorized_unlink(&root, &home);
    if substitute {
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("changed-target", &link).unwrap();
    }
    let result = recover_install(&root, Path::new("must-not-spawn"), &home);
    assert_eq!(result.is_err(), substitute);
    assert_eq!(fs::read(&outside).unwrap(), b"outside untouched");
    if substitute {
        assert!(journal.exists());
    }
}
