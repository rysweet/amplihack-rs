//! Every locked mutation validates the surviving intent or completion receipt.
use super::recovery_test_support::*;
use super::*;

fn assert_mutations_blocked(f: &Fixture) {
    let ledger = fs::read(f.root.join("ownership.json")).unwrap();
    let package = digest(&f.root.join("market/plugin")).unwrap();
    let marker = fs::read(f.root.join(recovery_archive_intent::NAME)).unwrap();
    let evidence = f
        .archives()
        .iter()
        .map(|p| (p.clone(), fs::read(p).ok()))
        .collect::<Vec<_>>();
    let pending = snapshot(&f.root.join("pending.json")).unwrap();
    let config = snapshot(&f.codex.join("config.toml")).unwrap();
    let hooks = snapshot(&f.codex.join("hooks.json")).unwrap();
    assert!(f.install().is_err());
    assert!(with_binary(&f.binary, super::uninstall).is_err());
    path_scope::with_scope(&f.root, &f.codex, || {
        let lock = open_package_lock(&f.root)?;
        lock.lock_exclusive()?;
        assert!(recover_install(&f.root, &f.binary, &f.codex).is_err());
        Ok(())
    })
    .unwrap();
    assert_eq!(fs::read(f.root.join("ownership.json")).unwrap(), ledger);
    assert_eq!(digest(&f.root.join("market/plugin")).unwrap(), package);
    assert_eq!(
        fs::read(f.root.join(recovery_archive_intent::NAME)).unwrap(),
        marker
    );
    assert_eq!(snapshot(&f.root.join("pending.json")).unwrap(), pending);
    assert_eq!(snapshot(&f.codex.join("config.toml")).unwrap(), config);
    assert_eq!(snapshot(&f.codex.join("hooks.json")).unwrap(), hooks);
    for (path, bytes) in evidence {
        assert_eq!(fs::read(path).ok(), bytes);
    }
}

#[test]
fn post_rename_failure_blocks_all_mutations_when_evidence_conflicts() {
    for conflict in ["changed", "missing", "corrupt-marker"] {
        let f = Fixture::new();
        f.orphan();
        let failed = storage::testing::with_sync_hook(
            |path| {
                if path == f.root
                    && !f.root.join("pending.json").exists()
                    && !f.archives().is_empty()
                {
                    bail!("injected post-rename sync failure");
                }
                Ok(())
            },
            || f.install(),
        );
        assert!(format!("{:#}", failed.unwrap_err()).contains("injected post-rename"));
        let archive = f.archives().pop().unwrap();
        match conflict {
            "changed" => fs::write(archive, b"changed evidence").unwrap(),
            "missing" => fs::remove_file(archive).unwrap(),
            _ => fs::write(
                f.root.join(recovery_archive_intent::NAME),
                b"corrupt marker",
            )
            .unwrap(),
        }
        assert_mutations_blocked(&f);
    }
}

#[test]
fn final_sync_and_unavailable_restoration_write_leave_discoverable_receipt() {
    let f = Fixture::new();
    let bytes = f.orphan();
    let mut injected = false;
    let failed = storage::testing::with_write_failure(|blocked| {
        storage::testing::with_sync_hook(
            |path| {
                if path == f.root
                    && regular_json(&f.root.join(recovery_archive_intent::NAME))?
                        .is_some_and(|v| v["complete"] == true)
                {
                    injected = true;
                    blocked.set(true);
                    // Fault the exact publication primitive formerly used for
                    // restoration, independent of filesystem permissions or UID.
                    assert!(
                        recovery_archive_storage::publish(&f.root.join("probe"), b"guard").is_err()
                    );
                    assert!(!f.root.join("probe").exists());
                    bail!("injected final completion synchronization failure");
                }
                Ok(())
            },
            || f.install(),
        )
    });
    assert!(injected);
    let error = format!("{:#}", failed.unwrap_err());
    assert!(error.contains("intent or receipt retained"), "{error}");
    assert!(error.contains("injected final completion"), "{error}");
    let archive = f.archives().pop().unwrap();
    assert_eq!(fs::read(&archive).unwrap(), bytes);
    assert_eq!(
        regular_json(&f.root.join(recovery_archive_intent::NAME))
            .unwrap()
            .unwrap()["complete"],
        true
    );
    fs::write(&archive, b"conflicting evidence").unwrap();
    assert_mutations_blocked(&f);
    fs::write(&archive, bytes).unwrap();
    assert!(f.install().unwrap());
    f.assert_progress();
    assert!(f.install().unwrap());
    assert_eq!(f.archives(), [archive]);
}

#[test]
fn completed_receipt_allows_new_transactions_and_subsequent_archival() {
    let f = Fixture::new();
    f.orphan();
    with_archive_stamp(|| assert!(f.install().unwrap()));
    let original = f.archives().pop().unwrap();
    let original_bytes = fs::read(&original).unwrap();
    f.upgrade(b"second abandonment");
    let pending = f.plan();
    fs::remove_file(f.codex.join("config.toml")).unwrap();
    fs::remove_file(f.codex.join("hooks.json")).unwrap();
    with_archive_stamp(|| assert!(f.install().unwrap()));
    assert_eq!(f.archives().len(), 2);
    assert_eq!(fs::read(original).unwrap(), original_bytes);
    assert_eq!(fs::read(&f.archives()[1]).unwrap(), pending);
    with_binary(&f.binary, super::uninstall).unwrap();
    assert!(!f.root.join("ownership.json").exists());
    assert!(!f.root.join("market/plugin").exists());
}

#[test]
fn unsuccessful_completion_publication_keeps_original_intent_for_retry() {
    let f = Fixture::new();
    let bytes = f.orphan();
    let mut injected = false;
    let failed = storage::testing::with_write_failure(|blocked| {
        storage::testing::with_sync_hook(
            |path| {
                if path == f.root
                    && !f.root.join("pending.json").exists()
                    && !f.archives().is_empty()
                {
                    injected = true;
                    blocked.set(true);
                }
                Ok(())
            },
            || f.install(),
        )
    });
    assert!(injected && failed.is_err());
    let marker = regular_json(&f.root.join(recovery_archive_intent::NAME))
        .unwrap()
        .unwrap();
    assert_ne!(marker["complete"], true);
    assert_eq!(fs::read(f.archives().pop().unwrap()).unwrap(), bytes);
    assert!(f.install().unwrap());
    f.assert_progress();
}

#[test]
fn evidence_is_revalidated_after_final_completion_barrier() {
    let f = Fixture::new();
    f.orphan();
    let ledger = fs::read(f.root.join("ownership.json")).unwrap();
    let package = digest(&f.root.join("market/plugin")).unwrap();
    let mut changed = false;
    let failed = storage::testing::with_sync_hook(
        |path| {
            if path == f.root
                && regular_json(&f.root.join(recovery_archive_intent::NAME))?
                    .is_some_and(|v| v["complete"] == true)
            {
                fs::write(f.archives().pop().unwrap(), b"changed during final sync")?;
                changed = true;
            }
            Ok(())
        },
        || f.install(),
    );
    assert!(changed && failed.is_err());
    assert_eq!(fs::read(f.root.join("ownership.json")).unwrap(), ledger);
    assert_eq!(digest(&f.root.join("market/plugin")).unwrap(), package);
    assert_mutations_blocked(&f);
}
