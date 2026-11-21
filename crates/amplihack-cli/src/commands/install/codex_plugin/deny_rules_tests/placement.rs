//! Issue #2589: the two directories on the way to the rules file, and what
//! amplihack refuses to write through (SEC-D2).
//!
//! Each is examined with `symlink_metadata` before anything is created. The
//! check is needed at `<CODEX_HOME>/rules` as well as at the home itself:
//! `ensure_private_directory` is `create_dir_all` followed by
//! `set_permissions`, and both of those follow a link, so without it a link
//! planted at that path by another user on a shared machine would have its
//! target chmod-ed 0700 and the rules file written inside it.

use super::*;

/// The same guard the Codex home has, one level down: a link at `rules` is
/// refused, its target keeps the mode it had, and nothing is written into it.
#[test]
fn a_symlinked_rules_directory_is_refused() {
    let home = Home::new();
    let outside = home._tmp.path().join("outside-rules");
    fs::create_dir_all(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&outside, home.rules_dir()).unwrap();

    let outcome = write_deny_rules(&home.path, &[]);
    assert!(
        matches!(outcome, DenyRulesWrite::Refused(_)),
        "a link at the rules directory is refused, not followed: got {outcome:?}"
    );
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o755,
        "the link target must not be chmod-ed 0700 through the link"
    );
    assert!(
        !outside.join(RULES_FILE).exists(),
        "and must not receive the rules file"
    );
}

/// The refusal names the directory, and quotes nothing that came off disk
/// (SEC-D3).
#[test]
fn the_rules_directory_refusal_reason_is_a_fixed_phrase() {
    let home = Home::new();
    let outside = home._tmp.path().join("outside-rules");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.rules"), "TOP-SECRET-CONTENT\n").unwrap();
    symlink(&outside, home.rules_dir()).unwrap();

    let DenyRulesWrite::Refused(reason) = write_deny_rules(&home.path, &[]) else {
        panic!("expected a refusal");
    };
    assert!(
        reason.contains("symlink"),
        "the reason says what is wrong: {reason}"
    );
    assert!(!reason.contains("TOP-SECRET-CONTENT"), "{reason}");
    assert!(!reason.contains('\n'), "{reason}");
}

/// A regular file where the rules directory belongs is a different fault
/// from a link, and is refused too rather than removed.
#[test]
fn a_rules_path_that_is_a_regular_file_is_refused_and_left_as_found() {
    let home = Home::new();
    fs::write(home.rules_dir(), "not a directory\n").unwrap();

    let outcome = write_deny_rules(&home.path, &[]);
    assert!(
        matches!(outcome, DenyRulesWrite::Refused(_)),
        "got {outcome:?}"
    );
    assert_eq!(
        fs::read_to_string(home.rules_dir()).unwrap(),
        "not a directory\n",
        "left exactly as found"
    );
}
