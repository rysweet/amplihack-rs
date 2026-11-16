//! Issue #2528: the reuse gate on a bundle snapshot's `<key>/` folder.
//!
//! Declared from `store.rs` as
//! `#[cfg(all(test, unix))] #[path = "tests_store.rs"] mod tests_store;`
//! because [`holds_only_bundle`] is private to that module.
//!
//! `holds_only_bundle` is the control that refuses to *reuse* a snapshot
//! carrying anything but the bundle and its pin record. It matters because
//! every step reads files directly under `AMPLIHACK_HOME`: were a planted
//! `config` or `.claude/` to survive the gate, the digest check beside it
//! would still pass, since that digest covers only `amplifier-bundle/`.

use super::holds_only_bundle;
use crate::test_support::tempdir_outside_framework_roots;
use std::fs;
use std::path::PathBuf;

/// A `<key>/` folder in the shape `place` leaves behind: a real
/// `amplifier-bundle/` directory and a regular `pin.json`.
fn placed_home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempdir_outside_framework_roots();
    let home = tmp.path().join("0123456789abcdef01234567");
    fs::create_dir_all(home.join(super::super::BUNDLE_DIR)).unwrap();
    fs::write(home.join(super::super::PIN_RECORD), "{}").unwrap();
    (tmp, home)
}

#[test]
fn the_placed_shape_is_accepted() {
    let (_tmp, home) = placed_home();
    assert!(
        holds_only_bundle(&home),
        "a bundle folder and a pin record is exactly what `place` writes"
    );
}

/// The reason the gate exists: steps read `AMPLIHACK_HOME/config` and
/// `AMPLIHACK_HOME/.claude/` directly, and neither is under the digest.
#[test]
fn an_extra_entry_a_step_would_read_is_refused() {
    // A planted file, and a planted directory: the gate counts entries, so
    // neither kind may slip past.
    for name in ["config", "settings.json", ".installed-by"] {
        let (_tmp, home) = placed_home();
        fs::write(home.join(name), "planted").unwrap();
        assert!(
            !holds_only_bundle(&home),
            "{name} is not what was placed, so the snapshot may not be reused"
        );
    }
    for name in [".claude", "tools"] {
        let (_tmp, home) = placed_home();
        fs::create_dir(home.join(name)).unwrap();
        assert!(
            !holds_only_bundle(&home),
            "{name}/ is not what was placed, so the snapshot may not be reused"
        );
    }
}

#[test]
fn a_missing_entry_is_refused() {
    let (_tmp, home) = placed_home();
    fs::remove_file(home.join(super::super::PIN_RECORD)).unwrap();
    assert!(!holds_only_bundle(&home), "the pin record is gone");

    let (_tmp, home) = placed_home();
    fs::remove_dir(home.join(super::super::BUNDLE_DIR)).unwrap();
    assert!(!holds_only_bundle(&home), "the bundle folder is gone");
}

/// Both names must be the right *kind*, checked without following links --
/// a symlink named `amplifier-bundle` would otherwise let the digest read a
/// tree outside the snapshot.
#[test]
fn a_symlink_standing_in_for_either_name_is_refused() {
    let (tmp, home) = placed_home();
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(elsewhere.join("recipes")).unwrap();

    fs::remove_dir(home.join(super::super::BUNDLE_DIR)).unwrap();
    std::os::unix::fs::symlink(&elsewhere, home.join(super::super::BUNDLE_DIR)).unwrap();
    assert!(
        !holds_only_bundle(&home),
        "a symlinked bundle folder is not the folder that was placed"
    );

    let (tmp, home) = placed_home();
    let target = tmp.path().join("target.json");
    fs::write(&target, "{}").unwrap();
    fs::remove_file(home.join(super::super::PIN_RECORD)).unwrap();
    std::os::unix::fs::symlink(&target, home.join(super::super::PIN_RECORD)).unwrap();
    assert!(
        !holds_only_bundle(&home),
        "a symlinked pin record is not the record that was placed"
    );
}

/// The two names must not be swapped: a *file* called `amplifier-bundle` or
/// a *directory* called `pin.json` is not the placed shape.
#[test]
fn the_two_names_may_not_swap_kinds() {
    let (_tmp, home) = placed_home();
    fs::remove_dir(home.join(super::super::BUNDLE_DIR)).unwrap();
    fs::write(home.join(super::super::BUNDLE_DIR), "not a folder").unwrap();
    assert!(!holds_only_bundle(&home));

    let (_tmp, home) = placed_home();
    fs::remove_file(home.join(super::super::PIN_RECORD)).unwrap();
    fs::create_dir(home.join(super::super::PIN_RECORD)).unwrap();
    assert!(!holds_only_bundle(&home));
}

/// A folder that cannot be read gives false (fail closed), rather than
/// reporting an empty folder as a match.
#[test]
fn an_unreadable_folder_is_refused() {
    let (_tmp, home) = placed_home();
    assert!(
        !holds_only_bundle(&home.join("no-such-key")),
        "a folder that does not exist cannot be read"
    );
}
