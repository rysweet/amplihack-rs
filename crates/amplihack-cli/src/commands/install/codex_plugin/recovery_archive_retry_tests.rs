//! Failure injection uses the existing synchronous storage seam; no sleeps.
use super::recovery_test_support::*;
use super::*;

fn intents(f: &Fixture) -> Vec<PathBuf> {
    fs::read_dir(&f.root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            if !p.is_file()
                || p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("pending.json.stale-")
            {
                return false;
            }
            fs::read(p).ok().is_some_and(|bytes| {
                let text = String::from_utf8_lossy(&bytes);
                text.contains("pending.json.stale-")
                    && serde_json::from_slice::<Value>(&bytes).is_ok()
            })
        })
        .collect()
}

#[test]
fn intent_publication_sync_failure_stops_install_and_keeps_recovery_evidence() {
    let f = Fixture::new();
    let bytes = f.orphan();
    let ownership = fs::read(f.root.join("ownership.json")).unwrap();
    let result = storage::testing::with_sync_hook(
        |path| {
            if path == f.root && f.root.join("pending.json").exists() {
                anyhow::bail!(
                    "injected archival publication sync failure: {}: retry after repairing storage",
                    path.display()
                );
            }
            Ok(())
        },
        || f.install(),
    );
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("injected archival publication"), "{error}");
    assert!(error.contains(&f.root.display().to_string()), "{error}");
    assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
    assert_eq!(fs::read(f.root.join("ownership.json")).unwrap(), ownership);
    assert!(!f.codex.join("config.toml").exists());
    assert!(
        f.install().unwrap(),
        "a matching retained intent must be resumable"
    );
    assert_eq!(f.archives().len(), 1);
    f.assert_progress();
}

#[test]
fn post_rename_sync_failure_leaves_intent_and_retry_finishes_same_archive() {
    let f = Fixture::new();
    let bytes = f.orphan();
    let result = with_archive_stamp(|| {
        storage::testing::with_sync_hook(
            |path| {
                if path == f.root
                    && !f.root.join("pending.json").exists()
                    && !f.archives().is_empty()
                {
                    anyhow::bail!("injected post-rename directory synchronization failure");
                }
                Ok(())
            },
            || f.install(),
        )
    });
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("injected post-rename"), "{error}");
    assert!(
        !intents(&f).is_empty(),
        "absence of pending must not disable recovery protection"
    );
    let archives = f.archives();
    assert_eq!(archives.len(), 1);
    assert_eq!(fs::read(&archives[0]).unwrap(), bytes);
    assert!(!f.codex.join("config.toml").exists());
    assert!(f.install().unwrap());
    assert_eq!(
        f.archives(),
        archives,
        "retry must finish the original archival intent"
    );
    assert!(intents(&f).is_empty());
    f.assert_progress();
}

#[test]
fn intent_retirement_sync_failure_restores_guard_before_returning_error() {
    let f = Fixture::new();
    let bytes = f.orphan();
    let result = storage::testing::with_sync_hook(
        |path| {
            if path == f.root
                && !f.root.join("pending.json").exists()
                && !f.archives().is_empty()
                && intents(&f).is_empty()
            {
                anyhow::bail!("injected intent retirement synchronization failure");
            }
            Ok(())
        },
        || f.install(),
    );
    assert!(format!("{:#}", result.unwrap_err()).contains("injected intent retirement"));
    assert_eq!(intents(&f).len(), 1);
    let archives = f.archives();
    assert_eq!(archives.len(), 1);
    assert_eq!(fs::read(&archives[0]).unwrap(), bytes);
    assert!(!f.codex.join("config.toml").exists());
    assert!(f.install().unwrap());
    assert_eq!(f.archives(), archives);
    assert!(intents(&f).is_empty());
    f.assert_progress();
}

#[test]
fn conflicting_or_missing_archive_after_failed_sync_keeps_next_startup_blocked() {
    for conflict in ["changed", "missing", "pending-reappeared"] {
        let f = Fixture::new();
        let bytes = f.orphan();
        let result = storage::testing::with_sync_hook(
            |path| {
                if path == f.root
                    && !f.root.join("pending.json").exists()
                    && !f.archives().is_empty()
                {
                    anyhow::bail!("injected archival sync failure");
                }
                Ok(())
            },
            || f.install(),
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("injected archival"));
        let archive = f
            .archives()
            .pop()
            .expect("journal was renamed before failed sync");
        if conflict == "pending-reappeared" {
            fs::write(f.root.join("pending.json"), &bytes).unwrap();
        } else if conflict == "changed" {
            fs::write(&archive, b"conflicting evidence").unwrap();
        } else {
            fs::remove_file(&archive).unwrap();
        }
        assert!(
            f.install().is_err(),
            "{conflict}: missing pending cannot bypass unfinished intent"
        );
        assert!(!intents(&f).is_empty());
        assert!(!f.codex.join("config.toml").exists());
    }
}

#[test]
fn source_identity_and_raw_content_are_rechecked_before_rename() {
    for change in ["bytes", "inode"] {
        let f = Fixture::new();
        let bytes = f.orphan();
        let mut changed = false;
        let result = storage::testing::with_sync_hook(
            |path| {
                if path == f.root
                    && !changed
                    && !intents(&f).is_empty()
                    && f.root.join("pending.json").exists()
                {
                    let pending = f.root.join("pending.json");
                    if change == "bytes" {
                        let mut edited = bytes.clone();
                        edited.push(b' '); // Same JSON value, different evidence bytes.
                        fs::write(&pending, edited).unwrap();
                    } else {
                        fs::rename(&pending, f.root.join("captured-journal")).unwrap();
                        fs::write(&pending, &bytes).unwrap();
                    }
                    changed = true;
                }
                Ok(())
            },
            || f.install(),
        );
        assert!(
            changed,
            "test must reach durable archival intent publication"
        );
        assert!(
            result.is_err(),
            "{change}: changed source must not be archived"
        );
        assert!(f.root.join("pending.json").is_file());
        assert!(f.archives().is_empty());
        assert!(!f.codex.join("config.toml").exists());
    }
}

#[test]
fn destination_created_after_intent_is_never_overwritten() {
    let f = Fixture::new();
    let bytes = f.orphan();
    let destination = f.root.join(format!("pending.json.stale-{STAMP}"));
    let mut occupied = false;
    let result = with_archive_stamp(|| {
        storage::testing::with_sync_hook(
            |path| {
                if path == f.root && !occupied && !intents(&f).is_empty() {
                    fs::write(&destination, b"concurrent archive").unwrap();
                    occupied = true;
                }
                Ok(())
            },
            || f.install(),
        )
    });
    assert!(
        occupied,
        "test must intercept the published intent before movement"
    );
    assert!(
        result.is_err(),
        "no-replace publication must retain conflicting evidence"
    );
    assert_eq!(fs::read(destination).unwrap(), b"concurrent archive");
    assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
    assert!(!f.codex.join("config.toml").exists());
}

#[test]
fn retry_rejects_escaping_or_unsupported_archival_intent() {
    fn escape(value: &mut Value) -> bool {
        match value {
            Value::String(s) if s.starts_with("pending.json.stale-") => {
                *s = "../escaped-journal".into();
                true
            }
            Value::Object(map) => map.values_mut().any(escape),
            Value::Array(values) => values.iter_mut().any(escape),
            _ => false,
        }
    }
    for invalid in ["destination", "version"] {
        let f = Fixture::new();
        let bytes = f.orphan();
        let result = storage::testing::with_sync_hook(
            |path| {
                if path == f.root && !intents(&f).is_empty() {
                    anyhow::bail!("injected stop after intent publication");
                }
                Ok(())
            },
            || f.install(),
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("injected stop"));
        let intent = intents(&f).pop().unwrap();
        let mut value: Value = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
        if invalid == "destination" {
            assert!(
                escape(&mut value),
                "intent must record its archive basename"
            );
        } else {
            value["schema_version"] = json!(999);
        }
        fs::write(&intent, json_bytes(&value).unwrap()).unwrap();
        assert!(f.install().is_err(), "{invalid}");
        assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
        assert!(!f.root.parent().unwrap().join("escaped-journal").exists());
        assert!(!f.codex.join("config.toml").exists());
    }
}
