//! Allowing trusted aliases must preserve owned-resource and foreign-state guards.
use super::*;

#[test]
fn alias_recovery_preserves_foreign_snapshots_packages_and_raw_journal() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for committed in [false, true] {
        for resource in ["config", "package", "backup"] {
            alias_recovery_preserves_foreign_snapshots_packages_and_raw_journal_case(
                committed, resource,
            );
        }
    }
}

fn alias_recovery_preserves_foreign_snapshots_packages_and_raw_journal_case(
    committed: bool,
    resource: &str,
) {
    let paths = AliasPaths::new(true);
    let _env = EnvGuard::set([
        ("HOME", paths.home.to_str().unwrap()),
        ("CODEX_HOME", paths.selected_home()),
    ]);
    pending_fixture(&paths, committed);
    let path = match resource {
        "config" => paths.codex_home.join("config.toml"),
        "package" => paths.root.join("market/plugin/resource"),
        _ => paths.root.join("previous-package/resource"),
    };
    fs::write(&path, b"foreign exact bytes").unwrap();
    let journal = paths.root.join("pending.json");
    let before = fs::read(&journal).unwrap();
    assert!(recover_install(&paths.root, Path::new("must-not-spawn"), &paths.codex_home).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"foreign exact bytes");
    assert_eq!(fs::read(journal).unwrap(), before);
    assert!(paths.root.join("previous-package").is_dir());
    paths.assert_alias_unchanged();
}

#[test]
fn alias_recovery_rejects_owned_directory_and_snapshot_links_without_target_changes() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for resource in ["config", "market"] {
        alias_recovery_rejects_owned_directory_and_snapshot_links_without_target_changes_case(
            resource,
        );
    }
}

fn alias_recovery_rejects_owned_directory_and_snapshot_links_without_target_changes_case(
    resource: &str,
) {
    let paths = AliasPaths::new(false);
    let _env = EnvGuard::set([
        ("HOME", paths.home.to_str().unwrap()),
        ("CODEX_HOME", paths.selected_home()),
    ]);
    pending_fixture(&paths, false);
    let path = if resource == "config" {
        paths.codex_home.join("config.toml")
    } else {
        paths.root.join("market")
    };
    let displaced = paths.dir.path().join("foreign-target");
    fs::rename(&path, &displaced).unwrap();
    std::os::unix::fs::symlink(&displaced, &path).unwrap();
    let target = if resource == "config" {
        displaced.clone()
    } else {
        displaced.join("plugin/resource")
    };
    let original = fs::read(&target).unwrap();
    let journal = paths.root.join("pending.json");
    let before = fs::read(&journal).unwrap();
    assert!(recover_install(&paths.root, Path::new("must-not-spawn"), &paths.codex_home).is_err());
    assert_eq!(fs::read(target).unwrap(), original);
    assert_eq!(fs::read_link(path).unwrap(), displaced);
    assert_eq!(fs::read(journal).unwrap(), before);
}

fn copy_regular_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        let target = destination.join(path.file_name().unwrap());
        let meta = fs::symlink_metadata(&path).unwrap();
        assert!(
            !meta.file_type().is_symlink(),
            "fixture copy never follows resource links"
        );
        if meta.is_dir() {
            copy_regular_tree(&path, &target);
        } else {
            assert!(meta.is_file());
            fs::copy(path, target).unwrap();
        }
    }
}

#[test]
fn alias_retarget_or_anchor_replacement_during_barrier_retains_both_journals() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for action in ["alias", "anchor"] {
        alias_retarget_or_anchor_replacement_during_barrier_retains_both_journals_case(action);
    }
}

fn alias_retarget_or_anchor_replacement_during_barrier_retains_both_journals_case(action: &str) {
    let paths = AliasPaths::new(false);
    let _env = EnvGuard::set([
        ("HOME", paths.home.to_str().unwrap()),
        ("CODEX_HOME", paths.selected_home()),
    ]);
    pending_fixture(&paths, false);
    let binary = native_fixture(&paths);
    let home = fs::canonicalize(&paths.home).unwrap();
    let config = fs::canonicalize(paths.codex_home.join("config.toml")).unwrap();
    let replacement = paths.dir.path().join("replacement/user");
    let displaced = paths.dir.path().join("held-original-user");
    let original_root = if action == "alias" {
        home.join(".amplihack/codex")
    } else {
        displaced.join(".amplihack/codex")
    };
    let replacement_root = if action == "alias" {
        replacement.join(".amplihack/codex")
    } else {
        home.join(".amplihack/codex")
    };
    let before = fs::read(paths.root.join("pending.json")).unwrap();
    let mut changed = false;
    let result = storage::testing::with_sync_hook(
        |path| {
            if !changed && fs::canonicalize(path)? == config {
                // Clone the EXACT restored state, including journal bytes. Content
                // equality alone must not authorize retirement through a new anchor.
                assert_eq!(
                    fs::read(paths.root.join("market/plugin/resource"))?,
                    b"original"
                );
                copy_regular_tree(&home, &replacement);
                fs::write(replacement.join("foreign-sentinel"), b"foreign preserved")?;
                if action == "alias" {
                    fs::remove_file(&paths.alias)?;
                    std::os::unix::fs::symlink(replacement.parent().unwrap(), &paths.alias)?;
                } else {
                    fs::rename(&home, &displaced)?;
                    fs::rename(&replacement, &home)?;
                }
                changed = true;
            }
            Ok(())
        },
        || recover_install(&paths.root, &binary, &paths.codex_home),
    );
    assert!(
        changed,
        "actual restored-state barrier replacement must execute"
    );
    assert!(
        result.is_err(),
        "changed captured mapping/identity must stop retirement"
    );
    assert_eq!(
        fs::read(original_root.join("pending.json")).unwrap(),
        before
    );
    assert_eq!(
        fs::read(replacement_root.join("pending.json")).unwrap(),
        before
    );
    assert_eq!(
        fs::read(replacement_root.join("market/plugin/resource")).unwrap(),
        b"original"
    );
    assert_eq!(
        fs::read(
            replacement_root
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("foreign-sentinel")
        )
        .unwrap(),
        b"foreign preserved"
    );
}

mod independent;
