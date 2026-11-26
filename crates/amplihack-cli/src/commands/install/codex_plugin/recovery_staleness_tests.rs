//! Startup classification, exercised through the real install boundary.
use super::recovery_test_support::*;
use super::*;

#[path = "recovery_staleness_tests/diagnostics.rs"]
mod diagnostics;

#[test]
fn orphaned_targets_archive_journal_and_finish_reconciliation() {
    for whole_directory in [false, true] {
        let f = Fixture::new();
        let bytes = f.orphan();
        if whole_directory {
            fs::remove_dir_all(&f.codex).unwrap();
        }
        assert!(
            f.install()
                .expect("missing targets must no longer block startup")
        );
        let archives = f.archives();
        assert_eq!(archives.len(), 1);
        assert_eq!(fs::read(&archives[0]).unwrap(), bytes);
        f.assert_progress();
    }
}

#[test]
fn corroborated_later_completion_archives_abandoned_transaction() {
    let f = Fixture::new();
    let bytes = f.superseded();
    let current = f.ledger();
    assert_ne!(current["transaction"], f.journal()["transaction"]);
    assert!(
        f.install()
            .expect("verified replacement must permit startup")
    );
    assert_eq!(f.archives().len(), 1);
    assert_eq!(fs::read(&f.archives()[0]).unwrap(), bytes);
    f.assert_progress();
}

#[test]
fn permitted_absence_does_not_archive_an_interrupted_first_install() {
    let f = Fixture::new();
    let _bytes = f.plan();
    let pending = f.journal();
    assert!(pending["config"].is_null());
    assert!(pending["snapshots"]["hooks"].is_null());
    assert!(
        f.install().unwrap(),
        "strict rollback of legitimate absence should finish"
    );
    assert!(
        f.archives().is_empty(),
        "permitted absence is not orphan evidence"
    );
    f.assert_progress();
}

#[test]
fn absence_permitted_in_expected_or_intermediate_state_does_not_prove_orphaning() {
    for permitted in ["original", "expected", "intermediate"] {
        let f = Fixture::new();
        f.orphan();
        let mut pending = f.journal();
        // Make hooks absence legitimate, so config alone determines classification.
        pending["snapshots"]["hooks"] = Value::Null;
        match permitted {
            "original" => pending["config"] = Value::Null,
            "expected" => pending["expected"]["config"] = Value::Null,
            _ => pending["config_states"]
                .as_array_mut()
                .unwrap()
                .push(Value::Null),
        }
        // An unrelated mismatch forces strict recovery to refuse instead of replaying.
        fs::write(
            f.root.join("market/.agents/plugins/marketplace.json"),
            b"{\"foreign\":true}",
        )
        .unwrap();
        let bytes = f.write_journal(&pending);
        f.assert_retained(&bytes);
    }
}

#[test]
fn partial_disappearance_with_surviving_transaction_state_stays_protected() {
    for missing in ["config.toml", "hooks.json"] {
        let f = Fixture::new();
        assert!(f.install().unwrap());
        f.upgrade(b"abandoned");
        let bytes = f.plan();
        fs::remove_file(f.codex.join(missing)).unwrap();
        f.assert_retained(&bytes);
    }
}

#[test]
fn newer_timestamps_without_completed_replacement_are_insufficient() {
    let f = Fixture::new();
    assert!(f.install().unwrap());
    f.upgrade(b"abandoned");
    let bytes = f.plan();
    fs::write(f.codex.join("config.toml"), b"# later foreign edit\n").unwrap();
    fs::write(
        f.codex.join("hooks.json"),
        b"{\"hooks\":{},\"foreign\":true}\n",
    )
    .unwrap();
    modified(&f.root.join("pending.json"), 1_700_000_000);
    for name in ["config.toml", "hooks.json"] {
        modified(&f.codex.join(name), 1_800_000_000);
    }
    f.assert_retained(&bytes);
}

#[test]
fn mixed_recorded_and_unfamiliar_targets_stay_protected() {
    for foreign in ["config.toml", "hooks.json"] {
        let f = Fixture::new();
        assert!(f.install().unwrap());
        f.upgrade(b"abandoned");
        let bytes = f.plan();
        let content: &[u8] = if foreign == "config.toml" {
            b"# unfamiliar\n"
        } else {
            b"{\"hooks\":{},\"unfamiliar\":true}"
        };
        fs::write(f.codex.join(foreign), content).unwrap();
        f.assert_retained(&bytes);
    }
}

#[test]
fn supersession_requires_supported_complete_consistent_evidence() {
    for missing in [
        "legacy-pending",
        "legacy-ledger",
        "unsupported-proof",
        "inconsistent-base",
        "unsupported-base",
        "generation-only",
        "same-transaction",
        "base-transaction",
        "recorded-config",
        "recorded-hooks",
        "config",
        "hooks",
        "package",
        "marketplace",
        "registrations",
        "backup",
        "uninstall",
        "cleanup",
        "retired-package",
    ] {
        let f = Fixture::new();
        f.superseded();
        let mut pending = f.journal();
        match missing {
            "legacy-pending" => {
                pending.as_object_mut().unwrap().remove("recovery_evidence");
            }
            "cleanup" => pending["backup_cleanup"] = json!({}),
            "inconsistent-base" => pending["ledger"]["transaction"] = json!("unrelated-base"),
            "unsupported-base" => pending["recovery_evidence"] = json!({"schema_version":999}),
            "recorded-config" | "recorded-hooks" => {
                let (value, name) = if missing == "recorded-config" {
                    (&pending["config"], "config.toml")
                } else {
                    (&pending["snapshots"]["hooks"], "hooks.json")
                };
                let bytes: Vec<u8> = serde_json::from_value(value.clone()).unwrap();
                fs::write(f.codex.join(name), bytes).unwrap();
            }
            "config" => {
                fs::write(f.codex.join("config.toml"), b"# foreign after completion\n").unwrap()
            }
            "hooks" => fs::write(
                f.codex.join("hooks.json"),
                b"{\"hooks\":{},\"foreign\":true}",
            )
            .unwrap(),
            "package" => fs::write(f.root.join("market/plugin/skills/marker"), b"foreign").unwrap(),
            "marketplace" => fs::write(
                f.root.join("market/.agents/plugins/marketplace.json"),
                b"{}",
            )
            .unwrap(),
            "registrations" => fs::remove_file(f.home.join("registered")).unwrap(),
            "backup" => {
                fs::create_dir(f.root.join("previous-package")).unwrap();
            }
            "retired-package" => {
                fs::create_dir(f.root.join("uninstall-package")).unwrap();
            }
            _ => {
                let mut ledger = f.ledger();
                match missing {
                    "legacy-ledger" => {
                        ledger.as_object_mut().unwrap().remove("completion");
                    }
                    "unsupported-proof" => ledger["completion"] = json!({"schema_version":999}),
                    "generation-only" => {
                        ledger["completion"] = json!({"schema_version":1,"generation":999})
                    }
                    "same-transaction" => ledger["transaction"] = pending["transaction"].clone(),
                    "base-transaction" => {
                        ledger["transaction"] = pending["ledger"]["transaction"].clone()
                    }
                    _ => ledger["uninstall_pending"] = json!(true),
                }
                fs::write(f.root.join("ownership.json"), json_bytes(&ledger).unwrap()).unwrap();
            }
        }
        let bytes = f.write_journal(&pending);
        f.assert_retained(&bytes);
    }
}

#[test]
fn malformed_out_of_scope_and_unsupported_journals_never_auto_archive() {
    for invalid in ["malformed", "scope", "version", "snapshots"] {
        let f = Fixture::new();
        f.orphan();
        let mut pending = f.journal();
        match invalid {
            "scope" => pending["codex_home"] = json!(f.home.join("other-codex")),
            "version" => pending["schema_version"] = json!(999),
            "snapshots" => pending["config"] = json!([999]),
            _ => {}
        }
        let bytes = if invalid == "malformed" {
            let bytes = b"\xffnot json".to_vec();
            fs::write(f.root.join("pending.json"), &bytes).unwrap();
            bytes
        } else {
            f.write_journal(&pending)
        };
        assert!(f.install().is_err(), "{invalid}");
        assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
        assert!(f.archives().is_empty());
    }
}

#[test]
fn nonregular_targets_are_errors_not_absence() {
    for kind in ["symlink", "directory"] {
        let f = Fixture::new();
        let bytes = f.orphan();
        let config = f.codex.join("config.toml");
        if kind == "symlink" {
            std::os::unix::fs::symlink(f.home.join("missing"), &config).unwrap();
        } else {
            fs::create_dir(&config).unwrap();
        }
        assert!(f.install().is_err(), "{kind}");
        assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
        assert!(f.archives().is_empty());
    }
}

#[test]
fn current_attempt_foreign_edit_still_refuses_without_archival() {
    let f = Fixture::new();
    assert!(f.install().unwrap());
    let before = fs::read(f.root.join("ownership.json")).unwrap();
    f.upgrade(b"current-attempt");
    fs::write(f.home.join("edit-during-add"), b"yes").unwrap();
    let error = format!("{:#}", f.install().unwrap_err());
    assert!(error.contains("foreign Codex config changed"), "{error}");
    assert!(f.root.join("pending.json").is_file());
    assert!(f.archives().is_empty());
    assert!(
        fs::read(f.root.join("ownership.json")).unwrap() == before,
        "foreign bytes must be rejected before completed ownership publication"
    );
}
