//! Issue #2589 (TDD): the Codex execpolicy rules file amplihack owns.
//!
//! Expected API, in `codex_plugin/deny_rules.rs`:
//!
//! ```ignore
//! pub(super) const RULES_DIR: &str = "rules";
//! pub(super) const RULES_FILE: &str = "amplihack-dangerous-commands.rules";
//! pub(super) enum DenyRulesWrite { Written, AlreadyCurrent, Absent, Refused(String) }
//! pub(super) enum DenyRulesRemoval { Removed, Absent, Kept(String) }
//! pub(super) fn write_deny_rules(codex_home: &Path, instance: &[String]) -> DenyRulesWrite;
//! pub(super) fn remove_deny_rules(codex_home: &Path) -> DenyRulesRemoval;
//! ```
//!
//! SEC-C1: the writer goes through `secure_files::write_private_atomic`, not
//! through a `<name>.tmp` written with `fs::write` under the ambient umask.
//! A predictable temp path with no `O_EXCL` lets a pre-created symlink
//! redirect the write on a shared host, and
//! `a_symlink_at_the_predictable_temp_path_redirects_no_write` is the
//! regression that fails against that shape.

use super::deny_rules::{
    DenyRulesRemoval, DenyRulesWrite, RULES_DIR, RULES_FILE, remove_deny_rules, write_deny_rules,
};
use crate::policy;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

/// A Codex home under a temp directory. Never the developer's `~/.codex`.
struct Home {
    _tmp: tempfile::TempDir,
    path: PathBuf,
}

impl Home {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("codex");
        fs::create_dir_all(&path).unwrap();
        Self { _tmp: tmp, path }
    }

    fn rules_dir(&self) -> PathBuf {
        self.path.join(RULES_DIR)
    }

    fn rules_file(&self) -> PathBuf {
        self.rules_dir().join(RULES_FILE)
    }
}

/// Bytes, mtime and inode: a rewrite, even to the same bytes, changes one.
fn fingerprint(path: &Path) -> (Vec<u8>, std::time::SystemTime, u64) {
    let meta = fs::symlink_metadata(path).unwrap();
    (
        fs::read(path).unwrap(),
        meta.modified().unwrap(),
        meta.ino(),
    )
}

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
#[test]
fn an_oversized_existing_file_is_replaced_rather_than_read_whole() {
    let home = Home::new();
    fs::create_dir_all(home.rules_dir()).unwrap();
    fs::write(home.rules_file(), "x".repeat(128 * 1024)).unwrap();
    fs::set_permissions(home.rules_file(), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(write_deny_rules(&home.path, &[]), DenyRulesWrite::Written);
    assert_eq!(
        fs::read_to_string(home.rules_file()).unwrap(),
        policy::codex::rules_file(&[])
    );
}

// --- removal (uninstall) ----------------------------------------------------

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
    assert!(
        matches!(outcome, DenyRulesRemoval::Kept(_)),
        "an unmarked file is not amplihack's: {outcome:?}"
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

/// Small helper so the happy-path tests read as one line.
trait ExpectWritten {
    fn expect_written(self);
}

impl ExpectWritten for DenyRulesWrite {
    fn expect_written(self) {
        assert_eq!(self, DenyRulesWrite::Written);
    }
}

// --- the rules directory itself (SEC-D2, the gap the design closes) ---------
//
// `write_deny_rules` checks the Codex home with `symlink_metadata` before it
// touches anything, but the directory below it, `<CODEX_HOME>/rules`, gets no
// such check: `ensure_private_directory` is `create_dir_all` followed by
// `set_permissions`, and both follow a link. A link planted at that path by
// another user on a shared machine therefore has its target chmod-ed to 0700
// and the rules file written into it. These tests fail against that shape and
// pass once the check the Codex home already has is applied one level down.

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
