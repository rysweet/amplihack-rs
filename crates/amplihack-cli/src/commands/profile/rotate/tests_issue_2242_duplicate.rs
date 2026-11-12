use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let p = entry.unwrap().path();
        if fs::symlink_metadata(&p).unwrap().is_dir() {
            out.extend(files(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn matching_duplicate_is_verified_parked_by_rename_receipted_and_swapped_in_one_cycle() {
    let e = fixture();
    let duplicate = fs::read(e.live()).unwrap();
    private(&e.slot("work"), std::str::from_utf8(&duplicate).unwrap());
    let old = fs::metadata(e.slot("work")).unwrap();
    let mut c = context(&e);
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    let parked: Vec<_> = files(&e.rs.dir)
        .into_iter()
        .filter(|p| fs::symlink_metadata(p).unwrap().ino() == old.ino())
        .collect();
    assert_eq!(
        parked.len(),
        1,
        "the original duplicate inode is retained once"
    );
    assert!(
        fs::read(&parked[0]).unwrap() == duplicate,
        "parked bytes must be preserved"
    );
    assert_eq!(fs::metadata(&parked[0]).unwrap().nlink(), 1);
    assert_eq!(
        fs::metadata(parked[0].parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let receipts: Vec<_> = files(&e.rs.dir)
        .into_iter()
        .filter_map(|p| {
            let v: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).ok()?;
            // Contract fields describe custody; file naming is deliberately free.
            (v.get("origin").is_some() && v.get("parked").is_some()).then_some((p, v))
        })
        .collect();
    assert_eq!(
        receipts.len(),
        1,
        "one durable receipt for the parked credential"
    );
    let (path, receipt) = &receipts[0];
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let receipt = receipt.to_string();
    assert!(
        receipt.contains("u-work") && receipt.contains("work"),
        "verified ownership and origin"
    );
    assert!(
        !receipt.contains("SENTINEL"),
        "receipts contain no credential material"
    );
    assert!(
        reads()
            .iter()
            .any(|p| p == &e.root.join("work").to_string_lossy()),
        "verify the occupied slot"
    );
    assert!(!e.rs.journal.exists());
}

#[test]
fn unverifiable_duplicate_is_preserved_without_parking_or_success_publication() {
    let e = fixture();
    private(
        &e.slot("work"),
        r#"{"claudeAiOauth":{"accessToken":"SENTINEL-UNKNOWN"}}"#,
    );
    let occupied = ino_at(&e.slot("work"));
    let mut c = context(&e);
    assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("work")), occupied);
    assert!(
        log(&e).contains("identity"),
        "explain failed verification: {}",
        log(&e)
    );
    assert!(!e.rs.dir.join("swap.json").exists());
}

#[test]
fn credential_substitution_during_duplicate_verification_cannot_authorise_parking() {
    use std::cell::Cell;
    thread_local! {static CHANGED: Cell<bool> = const {Cell::new(false)};}
    CHANGED.with(|c| c.set(false));
    let e = fixture();
    let bytes = fs::read(e.live()).unwrap();
    private(&e.slot("work"), std::str::from_utf8(&bytes).unwrap());
    let mut c = context(&e);
    c.identity = |dir| {
        let account = super::super::tests_fixture_identity::read(dir)?;
        if dir.ends_with("work") && !CHANGED.with(|c| c.replace(true)) {
            let original = dir.join("original-duplicate");
            fs::rename(dir.join(".credentials.json"), original).unwrap();
            private(
                &dir.join(".credentials.json"),
                r#"{"claudeAiOauth":{"accessToken":"SENTINEL-SUBSTITUTED"}}"#,
            );
        }
        Ok(account)
    };
    let _ = drive(&e, &mut c);
    assert!(
        CHANGED.with(Cell::get),
        "exercise the duplicate identity boundary"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert!(e.slot("work").exists());
    assert!(e.root.join("work/original-duplicate").exists());
    assert!(!e.rs.dir.join("swap.json").exists());
}

#[test]
fn parking_interruptions_recover_with_a_receipt_and_complete_in_the_next_driven_cycle() {
    for point in ["duplicate-intent", "duplicate-moved", "duplicate-receipt"] {
        let e = fixture();
        private(
            &e.slot("work"),
            std::str::from_utf8(&fs::read(e.live()).unwrap()).unwrap(),
        );
        let parked_ino = ino_at(&e.slot("work")).unwrap();
        let mut c = context(&e);
        test_exchange::interrupted(point, || {
            let _ = drive(&e, &mut c);
        });
        assert!(e.rs.journal.exists());
        assert!(
            !legacy::accepts(&fs::read(&e.rs.journal).unwrap()),
            "old readers must refuse active parking"
        );
        assert!(!e.rs.dir.join("swap.json").exists());
        assert_eq!(
            drive(&e, &mut c).unwrap(),
            CycleOutcome::Swapped,
            "{point}: {}",
            log(&e)
        );
        assert_eq!(ino_at(&e.live()), Some(e.t));
        assert_eq!(
            files(&e.rs.dir)
                .iter()
                .filter(|p| ino_at(p) == Some(parked_ino))
                .count(),
            1
        );
        assert!(!e.rs.journal.exists());
    }
}
