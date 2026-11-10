//! Completion evidence must originate in a validated, durable transaction.
use super::recovery_test_support::*;
use super::*;

#[test]
fn completed_reconcile_records_versioned_hashes_and_increasing_generation() {
    let f = Fixture::new();
    assert!(f.install().unwrap());
    let first = f.ledger();
    let proof = &first["completion"];
    assert_eq!(
        proof["schema_version"], 1,
        "new completed ledgers need supported proof"
    );
    let generation = proof["generation"]
        .as_u64()
        .expect("checked completion generation");
    let encoded = serde_json::to_string(proof).unwrap();
    for path in [
        f.codex.join("config.toml"),
        f.codex.join("hooks.json"),
        f.root.join("market/.agents/plugins/marketplace.json"),
    ] {
        let hash = format!("{:x}", Sha256::digest(fs::read(path).unwrap()));
        assert!(
            encoded.contains(&hash),
            "completion must bind each exact committed snapshot"
        );
    }
    f.upgrade(b"next-generation");
    assert!(f.install().unwrap());
    let second = f.ledger();
    assert_eq!(
        second["completion"]["generation"].as_u64(),
        generation.checked_add(1)
    );
    assert_ne!(first["transaction"], second["transaction"]);
    let exact = fs::read(f.root.join("ownership.json")).unwrap();
    assert!(f.install().unwrap());
    assert_eq!(
        fs::read(f.root.join("ownership.json")).unwrap(),
        exact,
        "unchanged-install fast path must not invent another completion"
    );
}

#[test]
fn pending_expected_ledger_is_durable_before_completed_ownership_publication() {
    let f = Fixture::new();
    assert!(f.install().unwrap());
    let previous = fs::read(f.root.join("ownership.json")).unwrap();
    f.upgrade(b"upgrade-with-proof");
    let mut observed = false;
    let mut expected_synced = false;
    storage::testing::with_sync_hook(
        |path| {
            let current = fs::read(f.root.join("ownership.json")).unwrap();
            if path == f.root && current == previous && f.root.join("pending.json").exists() {
                let pending = f.journal();
                let expected: Vec<u8> =
                    serde_json::from_value(pending["expected"]["ledger"].clone()).unwrap();
                let value: Value = serde_json::from_slice(&expected).unwrap();
                expected_synced |= value["completion"].is_object();
            }
            if path == f.root && current != previous && !observed {
                let pending = f.journal();
                assert!(
                    expected_synced,
                    "completed expected ledger must reach a durability barrier first"
                );
                assert_eq!(pending["expected"]["ledger"], json!(current));
                assert!(f.ledger()["completion"].is_object());
                observed = true;
            }
            Ok(())
        },
        || f.install(),
    )
    .unwrap();
    assert!(observed, "must observe the real ownership publication");
}

#[test]
fn pending_upgrade_carries_base_evidence_and_preserves_legacy_schema_versions() {
    let f = Fixture::new();
    assert!(f.install().unwrap());
    f.upgrade(b"planned");
    f.plan();
    let pending = f.journal();
    assert_eq!(pending["schema_version"], 2);
    assert_eq!(f.ledger()["schema_version"], 1);
    assert!(
        pending["recovery_evidence"].is_object(),
        "new journals need base completion evidence"
    );
    assert_eq!(pending["recovery_evidence"]["schema_version"], 1);
    let evidence = serde_json::to_string(&pending["recovery_evidence"]).unwrap();
    let hash = format!(
        "{:x}",
        Sha256::digest(fs::read(f.root.join("ownership.json")).unwrap())
    );
    assert!(
        evidence.contains(&hash),
        "base evidence must bind exact previous ownership bytes"
    );
    assert!(
        evidence.contains(f.ledger()["transaction"].as_str().unwrap()),
        "base transaction must be bound"
    );
}

#[test]
fn generation_overflow_refuses_before_creating_a_new_transaction() {
    let f = Fixture::new();
    assert!(f.install().unwrap());
    let mut ledger = f.ledger();
    // A corrupt/unsupported proof can also refuse; wrapping can never publish.
    if !ledger["completion"].is_object() {
        ledger["completion"] = json!({"schema_version":1});
    }
    ledger["completion"]["generation"] = json!(u64::MAX);
    let before = json_bytes(&ledger).unwrap();
    fs::write(f.root.join("ownership.json"), &before).unwrap();
    let package = digest(&f.root.join("market/plugin")).unwrap();
    f.upgrade(b"would-overflow");
    assert!(
        f.install().is_err(),
        "generation arithmetic must be checked"
    );
    assert_eq!(fs::read(f.root.join("ownership.json")).unwrap(), before);
    assert_eq!(digest(&f.root.join("market/plugin")).unwrap(), package);
}
