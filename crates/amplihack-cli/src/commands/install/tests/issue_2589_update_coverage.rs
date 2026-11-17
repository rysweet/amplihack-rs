//! Issue #2589 (TDD): `amplihack update` reaches the policy writers, and a
//! failed write is loud rather than silent (F12, R16, SEC-C3).
//!
//! Expected seam, in `install/mod.rs`:
//!
//! ```ignore
//! /// Applies the dangerous-command policy to every harness amplihack
//! /// manages. Never fatal: a harness that could not be written returns a
//! /// warning line and the install carries on.
//! pub(super) fn apply_dangerous_command_policy() -> PolicyReport;
//!
//! pub(super) struct PolicyReport {
//!     pub lines: Vec<crate::policy::ReportLine>,
//!     pub warnings: Vec<String>,
//! }
//! ```
//!
//! The lines report what this run wrote: a harness whose files were refused
//! gets no line saying they were written, and a harness there was nothing to
//! write for is named rather than passed over.
//!
//! Visibility this file needs: `update::post_install` becomes `pub(crate)`
//! so the post-update path can be driven from here, and `policy` items are
//! `pub(crate)`.
//!
//! Two things are checked separately, because they fail for different
//! reasons: that the function does the work (behaviour, against temp homes),
//! and that both install entry points call it (structure, against the source
//! of `install/mod.rs` — driving the whole installer in-process needs a
//! framework source this test binary does not have).

use super::super::apply_dangerous_command_policy;
use super::super::codex_plugin::{RULES_DIR, RULES_FILE};
use crate::policy;
use crate::test_support::{env_lock, env_paths, tempdir_outside_framework_roots};
use crate::update::post_install::run_post_update_install;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;

/// Empties the Claude policy record left by any earlier call on this thread,
/// so a report read here describes this test's writes alone.
fn clear_policy_record() {
    let _ = crate::policy::claude::take_record();
}

fn temp_homes() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempdir_outside_framework_roots();
    let home = tmp.path().join("home");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    (tmp, home, codex)
}

#[test]
fn applying_the_policy_writes_the_codex_rules_file() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (_tmp, home, codex) = temp_homes();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    let report = apply_dangerous_command_policy();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(codex.join(RULES_DIR).join(RULES_FILE).exists());
}

#[test]
fn applying_the_policy_twice_changes_nothing() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (_tmp, home, codex) = temp_homes();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    apply_dangerous_command_policy();
    let path = codex.join(RULES_DIR).join(RULES_FILE);
    let before = fs::read(&path).unwrap();
    apply_dangerous_command_policy();
    assert_eq!(fs::read(&path).unwrap(), before);
}

/// The Codex home here exists and takes the file, so the report says Codex
/// was written. The Claude writers did not run in this test, so Claude is
/// named as having nothing to write into rather than passed over.
#[test]
fn the_report_describes_what_this_run_wrote() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (_tmp, home, codex) = temp_homes();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    let report = apply_dangerous_command_policy();
    let expected = policy::install_report_lines(
        policy::HarnessWrite::NothingToWriteInto,
        policy::HarnessWrite::Written,
    );
    assert_eq!(
        report.lines, expected,
        "the section reports this run's writes"
    );
    let codex_line = report
        .lines
        .iter()
        .find(|line| line.text.starts_with("Codex:"))
        .expect("Codex was written and is named");
    assert!(
        codex_line.text.contains("deny rules written to"),
        "{:?}",
        codex_line.text
    );
}

/// SEC-A3: an absent Codex home is named as a skipped step, not reported as
/// a write that happened.
#[test]
fn an_absent_codex_home_is_named_rather_than_claimed_as_written() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let tmp = tempdir_outside_framework_roots();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

    let report = apply_dangerous_command_policy();
    let codex_line = report
        .lines
        .iter()
        .find(|line| line.text.starts_with("Codex:"))
        .expect("a harness with no home is still named");
    assert_eq!(codex_line.marker, policy::SKIPPED_MARKER, "{codex_line:?}");
    assert!(
        !codex_line.text.contains("deny rules written to"),
        "{:?}",
        codex_line.text
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

/// A Codex home whose rules file cannot be replaced gets no line saying the
/// rules were written: only the warning.
#[test]
fn a_refused_codex_write_is_not_reported_as_a_written_one() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (tmp, home, codex) = temp_homes();
    let rules = codex.join(RULES_DIR);
    fs::create_dir_all(&rules).unwrap();
    let outside = tmp.path().join("outside.rules");
    fs::write(&outside, "untouched\n").unwrap();
    symlink(&outside, rules.join(RULES_FILE)).unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    let report = apply_dangerous_command_policy();
    assert!(
        !report
            .lines
            .iter()
            .any(|line| line.text.starts_with("Codex:")),
        "an all-refused harness gets no line; the warning carries it: {:?}",
        report.lines
    );
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
}

/// SEC-C3: a security control that is not installed must not be silent, and
/// must not fail the install either.
#[test]
fn a_refused_codex_write_warns_and_does_not_fail_the_install() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (tmp, home, codex) = temp_homes();
    let rules = codex.join(RULES_DIR);
    fs::create_dir_all(&rules).unwrap();
    let outside = tmp.path().join("outside.rules");
    fs::write(&outside, "untouched\n").unwrap();
    symlink(&outside, rules.join(RULES_FILE)).unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    let report = apply_dangerous_command_policy();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    let warning = &report.warnings[0];
    assert!(warning.contains("Codex"), "{warning}");
    assert!(
        warning.contains("dangerous-command"),
        "the warning must say which control is missing: {warning}"
    );
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "untouched\n",
        "and nothing is written through the link"
    );
}

/// The post-update path runs the same installer closure `amplihack update`
/// passes, so the policy is applied on update as well as on install.
#[test]
fn the_post_update_install_path_applies_the_policy() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (_tmp, home, codex) = temp_homes();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    run_post_update_install(false, || {
        apply_dangerous_command_policy();
        Ok(())
    })
    .expect("the post-update install runs its installer");
    assert!(codex.join(RULES_DIR).join(RULES_FILE).exists());
}

#[test]
fn skip_install_applies_nothing() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (_tmp, home, codex) = temp_homes();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    run_post_update_install(true, || {
        apply_dangerous_command_policy();
        Ok(())
    })
    .expect("--skip-install is not an error");
    assert!(!codex.join(RULES_DIR).join(RULES_FILE).exists());
}

/// Structural: both install entry points call the writer. Without this, the
/// behaviour tests above pass while nothing ever calls the function.
#[test]
fn both_install_entry_points_call_the_policy_writer() {
    const SOURCE: &str = include_str!("../mod.rs");
    let calls = SOURCE.matches("apply_dangerous_command_policy()").count();
    assert!(
        calls >= 3,
        "expected the definition and a call from each of `install` and the \
         post-update install path; found {calls} occurrence(s)"
    );
}

/// SEC-A3: a Codex home amplihack does not manage is never created by the
/// policy writer, and its absence is not a warning.
#[test]
fn an_absent_codex_home_is_neither_created_nor_warned_about() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempdir_outside_framework_roots();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

    let report = apply_dangerous_command_policy();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(!home.join(".codex").exists());
}

/// A link planted where the rules directory belongs stops that write and
/// nothing else: the install carries on, the warning names the harness, and
/// the link target is neither chmod-ed nor written into.
#[test]
fn a_symlinked_rules_directory_warns_without_failing_the_install() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (tmp, home, codex) = temp_homes();
    let outside = tmp.path().join("outside-rules");
    fs::create_dir_all(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&outside, codex.join(RULES_DIR)).unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    let report = apply_dangerous_command_policy();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("Codex"),
        "{:?}",
        report.warnings
    );
    assert!(
        !report
            .lines
            .iter()
            .any(|line| line.text.starts_with("Codex:")),
        "nothing was written, so nothing is claimed: {:?}",
        report.lines
    );
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o755,
        "the link target keeps the mode it had"
    );
    assert!(!outside.join(RULES_FILE).exists());
}

// --- the record covers this run alone (SEC-C3) ------------------------------
//
// The Claude writers leave their outcomes in a thread-local record and the
// policy section reads it. `take_record` empties the record on the way out,
// which covers the ordinary sequence; it does not cover a record left behind
// by a phase that never reached its policy section -- an install whose
// Codex launch branch ran earlier in the same process, a profile seed, or a
// test on the same thread. An outcome from one of those, read by a later
// run, is a positive line about a file this run never met.
//
// `policy::claude::clear_record()` is what the write phase calls before the
// Claude writers run, and both install entry points call it.

/// An outcome recorded before this run's write phase began is not reported
/// as this run's work.
#[test]
fn an_outcome_recorded_before_the_write_phase_is_not_reported_as_this_runs_work() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (_tmp, home, codex) = temp_homes();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    // An earlier phase on this thread wrote a Claude settings file and
    // never reached a policy section to read its record.
    policy::claude::record("home settings", &policy::claude::DenyOutcome::Written);

    // This run's write phase begins here, as it does in both entry points.
    policy::claude::clear_record();
    let report = apply_dangerous_command_policy();

    let claude_line = report
        .lines
        .iter()
        .find(|line| line.text.starts_with("Claude Code:"))
        .expect("a harness with no file met is still named");
    assert_eq!(
        claude_line.marker,
        policy::SKIPPED_MARKER,
        "no Claude settings file was met on this run: {claude_line:?}"
    );
    assert!(
        !claude_line.text.contains("deny rules written"),
        "a line about a file this run never met is a false claim: {:?}",
        claude_line.text
    );
}

/// A refusal recorded by an earlier phase does not become this run's
/// warning either.
#[test]
fn a_refusal_recorded_before_the_write_phase_is_not_warned_about_again() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (_tmp, home, codex) = temp_homes();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    policy::claude::record(
        "home settings",
        &policy::claude::DenyOutcome::Unparseable("permissions.deny is not a JSON array".into()),
    );

    policy::claude::clear_record();
    let report = apply_dangerous_command_policy();
    assert!(
        report.warnings.is_empty(),
        "an earlier phase's refusal is not re-warned here: {:?}",
        report.warnings
    );
}

/// Clearing an already-empty record is not an error, so the call sits at the
/// start of the write phase unconditionally.
#[test]
fn clearing_an_empty_record_changes_nothing() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    policy::claude::clear_record();
    policy::claude::clear_record();
    let (outcome, warnings) = policy::claude::take_record();
    assert_eq!(outcome, policy::HarnessWrite::NothingToWriteInto);
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// Structural: both install entry points clear the record when their write
/// phase starts. Without this the behaviour above holds only where a test
/// calls `clear_record` itself.
#[test]
fn both_install_entry_points_clear_the_record_before_the_write_phase() {
    const SOURCE: &str = include_str!("../mod.rs");
    let clears = SOURCE.matches("clear_record()").count();
    assert!(
        clears >= 2,
        "expected a clear at the start of the write phase in each of `install` \
         and the launch bootstrap; found {clears} occurrence(s)"
    );
    for (site, _) in SOURCE.match_indices("apply_dangerous_command_policy()") {
        let before = &SOURCE[..site];
        if before.contains("fn apply_dangerous_command_policy")
            && !before.contains("clear_record()")
        {
            panic!(
                "a call to apply_dangerous_command_policy() at byte {site} has no \
                 clear_record() before it in the file: a record left by an earlier \
                 phase would be reported as this run's work"
            );
        }
    }
}
