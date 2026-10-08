use super::fixtures::Rollback;
use super::*;

#[test]
fn rollback_rejects_foreign_directory_links_before_native_or_restore() {
    for location in ["home", "market", "agents", "ancestor"] {
        rollback_rejects_foreign_directory_links_before_native_or_restore_case(location);
    }
}

fn rollback_rejects_foreign_directory_links_before_native_or_restore_case(location: &str) {
    let fixture = Rollback::new(true, true);
    let path = match location {
        "home" => fixture.home.clone(),
        "market" => fixture.root.join("market"),
        "agents" => fixture.root.join("market/.agents"),
        _ => fixture.dir.path().join("published"),
    };
    let journal = fixture.root.join("pending.json");
    let before = fs::read(&journal).unwrap();
    let displaced = fixture.dir.path().join("foreign");
    fs::rename(&path, &displaced).unwrap();
    std::os::unix::fs::symlink(&displaced, &path).unwrap();
    let error =
        recover_install(&fixture.root, Path::new("must-not-spawn"), &fixture.home).unwrap_err();
    assert!(
        format!("{error:#}").contains("directory component"),
        "{error:#}"
    );
    assert_eq!(fs::read(&journal).unwrap(), before);
    assert_eq!(fs::read_link(&path).unwrap(), displaced);
    fs::remove_file(&path).unwrap();
    fs::rename(displaced, path).unwrap();
    fixture.recover().unwrap();
    fixture.assert_original();
    assert!(!journal.exists());
}

#[test]
fn rollback_preserves_concurrent_byte_only_journal_change() {
    let fixture = Rollback::new(true, true);
    let journal = fixture.root.join("pending.json");
    let mut foreign = json_bytes(&fixture.pending).unwrap();
    foreign.push(b'\n');
    let mut changed = false;
    let result = storage::testing::with_sync_hook(
        |path| {
            if path == fixture.root && !changed {
                fixture.assert_original();
                changed = true;
                fs::write(&journal, &foreign)?;
            }
            Ok(())
        },
        || fixture.recover(),
    );
    assert!(changed);
    assert!(format!("{:#}", result.unwrap_err()).contains("journal changed concurrently"));
    assert_eq!(fs::read(journal).unwrap(), foreign);
}

mod independent;
