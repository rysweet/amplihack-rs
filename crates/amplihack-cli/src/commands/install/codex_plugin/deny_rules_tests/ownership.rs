//! Issue #2589 (R7): a file at amplihack's rules path that amplihack did not
//! write.
//!
//! `remove_deny_rules` already keeps such a file, and the module comment in
//! `codex_plugin/mod.rs` says hand-written files are left alone. The writer
//! did not: it replaced whatever stood there. These tests pin install and
//! uninstall to the same answer, through the one phrase that says why.

use super::*;

// Expected API, in `codex_plugin/deny_rules.rs`:
//
// ```ignore
// /// Why a file at the rules path is neither replaced nor removed. One
// /// constant, so install and uninstall cannot drift apart.
// pub(in crate::commands::install) const UNMARKED_REASON: &str =
//     "the file there does not carry amplihack's marker line";
// ```
//
// The trade-off this accepts is recorded in the pull request: an agent that
// can write the rules directory can disable the Codex half of the policy by
// planting an unmarked file. It cannot do so quietly -- the write is refused
// loudly, nothing claims the rules were written, and the warning names the
// file -- and the alternative, replacing whatever stands there, would
// destroy a user's own rules without asking.

/// Install leaves an unmarked file exactly as it found it, claims no write,
/// and gives the shared reason.
#[test]
fn install_leaves_a_rules_file_without_the_marker_untouched() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    let theirs = "# someone else's rules\nprefix_rule(program=\"nope\")\n";
    fs::write(home.rules_file(), theirs).unwrap();
    fs::set_permissions(home.rules_file(), fs::Permissions::from_mode(0o600)).unwrap();
    let before = fingerprint(&home.rules_file());

    let DenyRulesWrite::Refused(reason) = write_deny_rules(&home.path, &[]) else {
        panic!("a file amplihack did not write is not replaced");
    };
    assert_eq!(
        reason, UNMARKED_REASON,
        "install and uninstall say the same thing about the same file"
    );
    assert_eq!(
        fingerprint(&home.rules_file()),
        before,
        "the file is byte-identical, and was not even rewritten to the same bytes"
    );
    assert_eq!(fs::read_to_string(home.rules_file()).unwrap(), theirs);
}

/// Uninstall keeps the same file, with the same reason. The two halves of
/// the matrix are asserted together so they cannot drift.
#[test]
fn install_and_uninstall_agree_about_a_rules_file_without_the_marker() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    let theirs = "# someone else's rules\n";
    fs::write(home.rules_file(), theirs).unwrap();
    fs::set_permissions(home.rules_file(), fs::Permissions::from_mode(0o600)).unwrap();

    let write = write_deny_rules(&home.path, &[]);
    let removal = remove_deny_rules(&home.path);
    let (DenyRulesWrite::Refused(write_reason), DenyRulesRemoval::Kept(kept_reason)) =
        (write, removal)
    else {
        panic!("both halves leave a file amplihack did not write alone");
    };
    assert_eq!(
        write_reason, kept_reason,
        "one constant, so the two cannot drift apart"
    );
    assert_eq!(
        fs::read_to_string(home.rules_file()).unwrap(),
        theirs,
        "the file survives both"
    );
}

/// The other three rows of the matrix, so the refusal above did not simply
/// stop the writer working: absent is written, amplihack's own stale file is
/// replaced, and an identical one is left alone.
#[test]
fn a_marked_file_is_still_written_updated_and_left_current() {
    let home = Home::new();
    assert_eq!(
        write_deny_rules(&home.path, &[]),
        DenyRulesWrite::Written,
        "absent: written"
    );

    let stale = format!(
        "{}\n# an older rendering\n",
        policy::codex::OWNERSHIP_MARKER
    );
    fs::write(home.rules_file(), &stale).unwrap();
    assert_eq!(
        write_deny_rules(&home.path, &[]),
        DenyRulesWrite::Written,
        "marked and stale: replaced"
    );
    assert_eq!(
        fs::read_to_string(home.rules_file()).unwrap(),
        policy::codex::rules_file(&[])
    );

    let before = fingerprint(&home.rules_file());
    assert_eq!(
        write_deny_rules(&home.path, &[]),
        DenyRulesWrite::AlreadyCurrent,
        "marked and identical: untouched"
    );
    assert_eq!(fingerprint(&home.rules_file()), before);
}

/// The marker is matched on the leading bytes, not searched for: a file that
/// mentions the marker line further down is still not amplihack's.
#[test]
fn a_file_that_merely_mentions_the_marker_is_not_amplihacks() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    let theirs = format!(
        "# my own rules\n# copied from {}\n",
        policy::codex::OWNERSHIP_MARKER
    );
    fs::write(home.rules_file(), &theirs).unwrap();
    fs::set_permissions(home.rules_file(), fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(
        write_deny_rules(&home.path, &[]),
        DenyRulesWrite::Refused(UNMARKED_REASON.to_string())
    );
    assert_eq!(fs::read_to_string(home.rules_file()).unwrap(), theirs);
}
