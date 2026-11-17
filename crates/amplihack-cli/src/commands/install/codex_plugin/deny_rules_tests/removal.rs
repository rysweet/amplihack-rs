//! Issue #2589: what `amplihack uninstall` takes away, and what it keeps.
//!
//! Only amplihack's own marked file goes. A file someone else wrote at
//! that path, and a symlink at it, are left exactly as found.

use super::*;

#[test]
fn uninstall_removes_amplihacks_own_marked_file() {
    let home = Home::new();
    write_deny_rules(&home.path, &[]).expect_written();
    assert_eq!(remove_deny_rules(&home.path), DenyRulesRemoval::Removed);
    assert!(!home.rules_file().exists());
    assert!(
        home.rules_dir().exists(),
        "the directory may hold the user's own rules and is not removed"
    );
}

#[test]
fn uninstall_with_no_file_present_is_quiet() {
    let home = Home::new();
    assert_eq!(remove_deny_rules(&home.path), DenyRulesRemoval::Absent);
}

#[test]
fn uninstall_keeps_a_file_at_the_same_path_that_amplihack_did_not_write() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    fs::write(home.rules_file(), "# someone else's rules\n").unwrap();
    let outcome = remove_deny_rules(&home.path);
    assert_eq!(
        outcome,
        DenyRulesRemoval::Kept(UNMARKED_REASON.to_string()),
        "an unmarked file is not amplihack's, and the reason is the one \
         constant install uses too (R7)"
    );
    assert!(home.rules_file().exists());
}

#[test]
fn uninstall_keeps_a_symlink_at_the_path_and_removes_nothing_through_it() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    let outside = home._tmp.path().join("outside.rules");
    fs::write(&outside, policy::codex::rules_file(&[])).unwrap();
    symlink(&outside, home.rules_file()).unwrap();
    let outcome = remove_deny_rules(&home.path);
    assert!(
        matches!(outcome, DenyRulesRemoval::Kept(_)),
        "got {outcome:?}"
    );
    assert!(outside.exists(), "nothing is removed through a symlink");
    assert!(fs::symlink_metadata(home.rules_file()).is_ok());
}
