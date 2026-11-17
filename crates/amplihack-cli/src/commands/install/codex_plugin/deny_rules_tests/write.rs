//! Issue #2589: what the Codex rules writer puts on disk.
//!
//! SEC-C1: the writer goes through `secure_files::write_private_atomic`,
//! not through a `<name>.tmp` written with `fs::write` under the ambient
//! umask. A predictable temp path with no `O_EXCL` lets a pre-created
//! symlink redirect the write on a shared host, and
//! `a_symlink_at_the_predictable_temp_path_redirects_no_write` is the
//! regression that fails against that shape.

use super::*;

#[test]
fn the_file_is_written_into_the_rules_directory_of_the_codex_home() {
    let home = Home::new();
    assert_eq!(write_deny_rules(&home.path, &[]), DenyRulesWrite::Written);
    let text = fs::read_to_string(home.rules_file()).unwrap();
    assert_eq!(
        text,
        policy::codex::rules_file(&[]),
        "the file is exactly what the one policy list renders"
    );
}

#[test]
fn the_file_is_private_and_the_rules_directory_is_too() {
    let home = Home::new();
    write_deny_rules(&home.path, &[]).expect_written();
    assert_eq!(
        fs::metadata(home.rules_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(home.rules_dir()).unwrap().permissions().mode() & 0o777,
        0o700,
        "SEC-D2: created with ensure_private_directory, not under the ambient umask"
    );
}

#[test]
fn a_second_run_reports_already_current_and_leaves_the_bytes_untouched() {
    let home = Home::new();
    write_deny_rules(&home.path, &[]).expect_written();
    let before = fingerprint(&home.rules_file());
    assert_eq!(
        write_deny_rules(&home.path, &[]),
        DenyRulesWrite::AlreadyCurrent
    );
    assert_eq!(fingerprint(&home.rules_file()), before);
}

#[test]
fn a_changed_instance_list_rewrites_the_file() {
    let home = Home::new();
    write_deny_rules(&home.path, &[]).expect_written();
    assert_eq!(
        write_deny_rules(&home.path, &["shutdown".to_string()]),
        DenyRulesWrite::Written
    );
    let text = fs::read_to_string(home.rules_file()).unwrap();
    assert!(text.contains("\"shutdown\""), "{text}");
    assert!(text.contains("\"pkill\""), "an addition removes no default");
}

/// R15/T8: amplihack replaces only its own file. A hand-written rules file in
/// the same directory is left alone.
#[test]
fn another_file_in_the_rules_directory_is_untouched() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    let other = home.rules_dir().join("no-pattern-kill.rules");
    fs::write(&other, "# hand written\n").unwrap();
    let before = fingerprint(&other);
    write_deny_rules(&home.path, &[]).expect_written();
    assert_eq!(fingerprint(&other), before);
}

#[test]
fn an_absent_codex_home_is_never_created() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("no-codex-here");
    assert_eq!(write_deny_rules(&missing, &[]), DenyRulesWrite::Absent);
    assert!(
        !missing.exists(),
        "SEC-A3: a harness home amplihack does not manage is never created"
    );
}

#[test]
fn a_symlinked_target_is_refused_and_the_link_destination_is_not_written() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    let outside = home._tmp.path().join("outside.txt");
    fs::write(&outside, "untouched\n").unwrap();
    symlink(&outside, home.rules_file()).unwrap();

    let outcome = write_deny_rules(&home.path, &[]);
    assert!(
        matches!(outcome, DenyRulesWrite::Refused(_)),
        "got {outcome:?}"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched\n");
}

#[test]
fn a_target_that_is_not_a_regular_file_is_refused() {
    let home = Home::new();
    fs::create_dir_all(home.rules_file()).unwrap();
    let outcome = write_deny_rules(&home.path, &[]);
    assert!(
        matches!(outcome, DenyRulesWrite::Refused(_)),
        "got {outcome:?}"
    );
    assert!(home.rules_file().is_dir(), "left as found");
}

/// SEC-C1, the regression. `copilot_plugin::atomic_write` writes to
/// `<name>.tmp` with `fs::write` and no `O_EXCL` or `O_NOFOLLOW`, so a
/// symlink pre-created there sends the bytes somewhere else entirely. This
/// test fails against that shape and passes with `write_private_atomic`.
#[test]
fn a_symlink_at_the_predictable_temp_path_redirects_no_write() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    let victim = home._tmp.path().join("victim.txt");
    fs::write(&victim, "untouched\n").unwrap();
    let predictable = home.rules_dir().join(format!("{RULES_FILE}.tmp"));
    symlink(&victim, &predictable).unwrap();

    let _ = write_deny_rules(&home.path, &[]);
    assert_eq!(
        fs::read_to_string(&victim).unwrap(),
        "untouched\n",
        "the writer must not follow a pre-created symlink at a predictable temp path"
    );
}

#[test]
fn a_refusal_reason_is_a_fixed_phrase_and_never_file_content() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    let outside = home._tmp.path().join("secret.txt");
    fs::write(&outside, "TOP-SECRET-CONTENT\n").unwrap();
    symlink(&outside, home.rules_file()).unwrap();
    let DenyRulesWrite::Refused(reason) = write_deny_rules(&home.path, &[]) else {
        panic!("expected a refusal");
    };
    assert!(
        !reason.contains("TOP-SECRET-CONTENT"),
        "SEC-D3: a reason is a fixed phrase, never file content: {reason}"
    );
    assert!(!reason.contains('\n'), "{reason}");
}

/// SEC-V5: the comparison read is bounded and does not follow a symlink, so
/// an agent-writable path cannot make the install read an unbounded file.
///
/// The bound is now also what decides the file's fate: a file too large to
/// read is a file whose marker cannot be seen, and an unseen marker means
/// the file stays (R7). The reason says it is too large, and not that it
/// lacks the marker, so a user can tell their own hand-written rules from a
/// file that is merely beyond the bound.
#[test]
fn an_oversized_existing_file_is_left_as_found_and_reported() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    let body = "x".repeat(128 * 1024);
    fs::write(home.rules_file(), &body).unwrap();
    fs::set_permissions(home.rules_file(), fs::Permissions::from_mode(0o600)).unwrap();
    let DenyRulesWrite::Refused(reason) = write_deny_rules(&home.path, &[]) else {
        panic!("a file whose marker cannot be read is not replaced");
    };
    assert!(
        reason.contains("too large"),
        "the reason distinguishes an oversized file from an unmarked one: {reason}"
    );
    assert_eq!(
        fs::read_to_string(home.rules_file()).unwrap(),
        body,
        "the file is byte-identical to what was found"
    );
}
