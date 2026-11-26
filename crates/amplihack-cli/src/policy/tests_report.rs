//! Issue #2589 (TDD): the install's policy section reports what this run
//! actually wrote, not a fixed claim.
//!
//! The defect these tests exist for: `install_report_lines()` took no
//! arguments and always printed "deny rules written ... Claude Code reads
//! them from permissions.deny", whether or not a single file had taken an
//! entry. A settings file holding a `permissions` value amplihack cannot
//! merge gets no deny entries at all, and on the old form the run that met
//! it still said the rules were written. A security control believed to be
//! present and absent is worse than one known to be absent, so a positive
//! claim must not outlive the write behind it.
//!
//! Expected API, declared in `policy/mod.rs`:
//!
//! ```ignore
//! /// What one harness's deny-rule write came to on this run.
//! #[derive(Debug, Clone, Copy, PartialEq, Eq)]
//! pub(crate) enum HarnessWrite {
//!     /// Every file this harness has holds the entries, whether this run
//!     /// added them or found them already there.
//!     Written,
//!     /// There was no home on this machine to write into, and none was
//!     /// created.
//!     NothingToWriteInto,
//!     /// Some files took the entries and some were refused.
//!     Partial,
//!     /// Every file this harness has refused the entries.
//!     AllRefused,
//! }
//!
//! /// One line of the install's policy section: the marker it is printed
//! /// with, and the sentence itself.
//! #[derive(Debug, Clone, PartialEq, Eq)]
//! pub(crate) struct ReportLine { pub marker: &'static str, pub text: String }
//!
//! pub(crate) const WRITTEN_MARKER: &str = "ℹ️";
//! pub(crate) const SKIPPED_MARKER: &str = "↩️";
//!
//! pub(crate) fn install_report_lines(
//!     claude: HarnessWrite,
//!     codex: HarnessWrite,
//! ) -> Vec<ReportLine>;
//! ```
//!
//! The wording is fixed by `docs/reference/dangerous-command-policy.md`,
//! under "Install and update output"; these tests and that table must agree.

use super::*;

/// Every line of a report, as the install prints it.
fn rendered(claude: HarnessWrite, codex: HarnessWrite) -> Vec<String> {
    install_report_lines(claude, codex)
        .into_iter()
        .map(|line| format!("{}  {}", line.marker, line.text))
        .collect()
}

/// The text of every line, with the marker dropped.
fn texts(claude: HarnessWrite, codex: HarnessWrite) -> Vec<String> {
    install_report_lines(claude, codex)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

/// Whether any line claims `harness` had its rules written to a file.
fn claims_written(lines: &[String], harness: &str) -> bool {
    lines
        .iter()
        .any(|line| line.starts_with(harness) && line.contains("deny rules written to"))
}

// --- the shape of a report that succeeded ----------------------------------

#[test]
fn a_run_that_wrote_everything_names_each_of_the_three_harnesses_once() {
    let lines = texts(HarnessWrite::Written, HarnessWrite::Written);
    assert_eq!(lines.len(), 3, "one line per harness: {lines:?}");
    for harness in ["Claude Code:", "Codex:", "Copilot CLI:"] {
        assert_eq!(
            lines.iter().filter(|l| l.starts_with(harness)).count(),
            1,
            "{harness} must be named exactly once in {lines:?}"
        );
    }
    assert!(claims_written(&lines, "Claude Code"), "{lines:?}");
    assert!(claims_written(&lines, "Codex"), "{lines:?}");
}

#[test]
fn the_harness_lines_are_in_the_order_the_documented_output_shows() {
    let lines = texts(HarnessWrite::Written, HarnessWrite::Written);
    let order: Vec<&str> = lines
        .iter()
        .map(|l| l.split(':').next().unwrap_or_default())
        .collect();
    assert_eq!(
        order,
        vec!["Claude Code", "Codex", "Copilot CLI"],
        "{lines:?}"
    );
}

#[test]
fn a_written_line_carries_the_written_marker() {
    for line in install_report_lines(HarnessWrite::Written, HarnessWrite::Written) {
        assert_eq!(
            line.marker, WRITTEN_MARKER,
            "a line about something that was written uses the written marker: {line:?}"
        );
    }
}

// --- the Copilot line is unconditional -------------------------------------

/// Copilot CLI's mechanism is the launch arguments: there is nothing on disk
/// to succeed or fail at, so its line does not vary with what the file
/// writers did.
#[test]
fn the_copilot_line_is_printed_whatever_the_file_writers_did() {
    for claude in [
        HarnessWrite::Written,
        HarnessWrite::NothingToWriteInto,
        HarnessWrite::Partial,
        HarnessWrite::AllRefused,
    ] {
        for codex in [
            HarnessWrite::Written,
            HarnessWrite::NothingToWriteInto,
            HarnessWrite::Partial,
            HarnessWrite::AllRefused,
        ] {
            let lines = texts(claude, codex);
            assert_eq!(
                lines
                    .iter()
                    .filter(|l| l.starts_with("Copilot CLI:"))
                    .count(),
                1,
                "the Copilot line is unconditional; missing for {claude:?}/{codex:?}: {lines:?}"
            );
        }
    }
}

#[test]
fn the_copilot_line_states_the_mechanism_and_that_it_applies_per_launch() {
    let lines = texts(HarnessWrite::Written, HarnessWrite::Written);
    let copilot = lines
        .iter()
        .find(|l| l.starts_with("Copilot CLI:"))
        .expect("the Copilot line is always present");
    assert!(
        copilot.contains("each launch") || copilot.contains("every launch"),
        "the report must say the policy is applied per launch: {copilot}"
    );
    assert!(
        copilot.contains("per project"),
        "the report must say where Copilot keeps its own approvals: {copilot}"
    );
}

// --- a harness that was refused is never claimed ---------------------------

/// The defect this file exists for. Every file refused means no deny rules
/// for that harness, and the run must not say otherwise.
#[test]
fn a_harness_whose_files_were_all_refused_gets_no_line_claiming_it_was_written() {
    let lines = texts(HarnessWrite::AllRefused, HarnessWrite::Written);
    assert!(
        !claims_written(&lines, "Claude Code"),
        "a refused Claude write must not be reported as written: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("Claude Code:")),
        "an all-refused harness gets no line at all; the warnings carry it: {lines:?}"
    );

    let lines = texts(HarnessWrite::Written, HarnessWrite::AllRefused);
    assert!(!claims_written(&lines, "Codex"), "{lines:?}");
    assert!(!lines.iter().any(|l| l.starts_with("Codex:")), "{lines:?}");
}

#[test]
fn a_partly_written_harness_says_so_and_points_at_the_warnings() {
    let lines = texts(HarnessWrite::Partial, HarnessWrite::Written);
    let claude = lines
        .iter()
        .find(|l| l.starts_with("Claude Code:"))
        .expect("a partial write is still reported");
    assert!(
        !claude.contains("deny rules written to"),
        "a partial write must not read as a complete one: {claude}"
    );
    assert!(
        claude.contains("refused"),
        "a partial write names the warnings below: {claude}"
    );
}

// --- a harness there was nothing to write for ------------------------------

/// A machine with no Codex home is not a failure and not a success; passing
/// over it silently leaves the reader to guess which. It is named, with the
/// skipped marker.
#[test]
fn a_harness_with_no_home_is_named_rather_than_passed_over() {
    let lines = install_report_lines(HarnessWrite::Written, HarnessWrite::NothingToWriteInto);
    let codex = lines
        .iter()
        .find(|l| l.text.starts_with("Codex:"))
        .expect("a harness with no home is still named");
    assert_eq!(
        codex.marker, SKIPPED_MARKER,
        "nothing to write is a skipped step, not a written one: {codex:?}"
    );
    assert!(
        !codex.text.contains("deny rules written to"),
        "{:?}",
        codex.text
    );
    assert!(
        codex.text.contains("nothing to write"),
        "the line says plainly that there was nothing to write: {:?}",
        codex.text
    );
    assert!(
        codex.text.contains("when one appears"),
        "and that the rules are written once a home exists: {:?}",
        codex.text
    );
}

#[test]
fn a_claude_install_with_no_settings_file_is_named_the_same_way() {
    let lines = install_report_lines(HarnessWrite::NothingToWriteInto, HarnessWrite::Written);
    let claude = lines
        .iter()
        .find(|l| l.text.starts_with("Claude Code:"))
        .expect("Claude is named when there was nothing to write");
    assert_eq!(claude.marker, SKIPPED_MARKER, "{claude:?}");
    assert!(!claude.text.contains("deny rules written to"), "{claude:?}");
}

// --- the output rules that apply to every line -----------------------------

#[test]
fn every_line_is_one_plain_sentence_with_no_path_and_no_count() {
    for claude in [
        HarnessWrite::Written,
        HarnessWrite::NothingToWriteInto,
        HarnessWrite::Partial,
        HarnessWrite::AllRefused,
    ] {
        for codex in [
            HarnessWrite::Written,
            HarnessWrite::NothingToWriteInto,
            HarnessWrite::Partial,
            HarnessWrite::AllRefused,
        ] {
            for line in rendered(claude, codex) {
                assert!(!line.contains('\n'), "one sentence per line: {line}");
                assert!(
                    !line.contains("/home/") && !line.contains("/Users/"),
                    "no user path in install output: {line}"
                );
                for banned in ["gate", "Gate", "refusal", "Refusal", "refuse ", "Refuse "] {
                    assert!(
                        !line.contains(banned),
                        "install output names the mechanism rather than writing {banned:?}: {line}"
                    );
                }
            }
        }
    }
}

/// F9, corrected: `amplifier`, `rusty` and `rustyclawd` read the Claude
/// settings file through a pinned `CLAUDE_CONFIG_DIR`, so none of them is a
/// harness amplihack leaves uncovered. The report must not say otherwise.
#[test]
fn the_report_never_claims_a_harness_is_left_without_a_mechanism() {
    let text = texts(HarnessWrite::Written, HarnessWrite::Written)
        .join("\n")
        .to_lowercase();
    for claim in ["no mechanism", "offers none", "has none", "none of its own"] {
        assert!(
            !text.contains(claim),
            "the Claude-family harnesses are covered by the Claude settings \
             file, so the report must not claim {claim:?}: {text}"
        );
    }
}

// --- the warning that goes with a refused write ----------------------------

#[test]
fn a_failed_policy_write_warning_names_the_harness_and_the_reason() {
    let warning = write_failure_warning("Codex", "is a symlink");
    assert!(warning.contains("Codex"), "{warning}");
    assert!(warning.contains("is a symlink"), "{warning}");
    assert!(
        warning.contains("dangerous-command"),
        "the warning must say which control is missing: {warning}"
    );
    assert!(!warning.contains('\n'), "one line: {warning}");
}

/// A warning that does not say which file cannot be acted on, so the Claude
/// writers pass a label that names the settings file they met.
#[test]
fn a_claude_warning_names_the_file_it_met() {
    let home = write_failure_warning(
        "Claude Code (home settings)",
        "permissions.deny is not a JSON array",
    );
    assert!(home.contains("Claude Code"), "{home}");
    assert!(home.contains("home settings"), "{home}");

    let profile = write_failure_warning(
        "Claude Code (profile work)",
        "permissions is not a JSON object",
    );
    assert!(profile.contains("profile work"), "{profile}");
}

// --- the record the Claude writers leave behind ----------------------------
//
// Claude's `permissions.deny` is written from three places: the install's own
// settings writer, the per-profile refresh and profile seeding. They run at
// different points of the install and none of them returns to the policy
// section, so each records what it did and the policy section reads the
// record rather than being threaded through all three.
//
// Expected API, declared in `policy/claude.rs`:
//
// ```ignore
// /// Records what one Claude settings file came to. `label` names the file
// /// ("home settings", "profile work") and never holds file content.
// pub(crate) fn record(label: &str, outcome: &DenyOutcome);
//
// /// What the record adds up to, and one warning line per refused file,
// /// clearing the record. An empty record is `NothingToWriteInto`.
// pub(crate) fn take_record() -> (super::HarnessWrite, Vec<String>);
// ```
//
// The record is thread-local: the install writes and reads it on one thread,
// and a per-thread record keeps the tests in this binary independent of each
// other without a lock.

use super::claude::{DenyOutcome, clear_record, record, take_record};

/// Each test starts from an empty record on its own thread.
fn clear() {
    let _ = take_record();
}

#[test]
fn an_empty_record_means_there_was_nothing_to_write_into() {
    clear();
    let (outcome, warnings) = take_record();
    assert_eq!(outcome, HarnessWrite::NothingToWriteInto);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_record_of_writes_alone_is_a_written_outcome() {
    clear();
    record("home settings", &DenyOutcome::Written);
    record("profile work", &DenyOutcome::AlreadyCurrent);
    let (outcome, warnings) = take_record();
    assert_eq!(
        outcome,
        HarnessWrite::Written,
        "a file that already held the entries holds them still"
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_record_holding_one_refusal_among_writes_is_partial_and_warns_once() {
    clear();
    record("home settings", &DenyOutcome::Written);
    record(
        "profile work",
        &DenyOutcome::Unparseable("permissions is not a JSON object".to_string()),
    );
    let (outcome, warnings) = take_record();
    assert_eq!(outcome, HarnessWrite::Partial);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("profile work"), "{:?}", warnings[0]);
    assert!(
        warnings[0].contains("permissions is not a JSON object"),
        "{:?}",
        warnings[0]
    );
    assert!(
        warnings[0].contains("dangerous-command"),
        "the warning says which control is missing: {:?}",
        warnings[0]
    );
}

#[test]
fn a_record_of_refusals_alone_is_an_all_refused_outcome() {
    clear();
    record(
        "home settings",
        &DenyOutcome::Unparseable("permissions.deny is not a JSON array".to_string()),
    );
    let (outcome, warnings) = take_record();
    assert_eq!(outcome, HarnessWrite::AllRefused);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn taking_the_record_clears_it_so_a_later_run_does_not_inherit_a_warning() {
    clear();
    record(
        "home settings",
        &DenyOutcome::Unparseable("permissions.deny is not a JSON array".to_string()),
    );
    let (first, warnings) = take_record();
    assert_eq!(first, HarnessWrite::AllRefused);
    assert_eq!(warnings.len(), 1);
    let (second, warnings) = take_record();
    assert_eq!(
        second,
        HarnessWrite::NothingToWriteInto,
        "the record is emptied by the read"
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// The label is written by amplihack; the reason comes from the file. Neither
/// may carry a control character into the terminal (SEC-V4).
#[test]
fn a_recorded_warning_carries_no_control_character() {
    clear();
    record(
        "home settings",
        &DenyOutcome::Unparseable("bad\u{1b}[31m value\n".to_string()),
    );
    let (_, warnings) = take_record();
    assert_eq!(warnings.len(), 1);
    assert!(
        !warnings[0].chars().any(char::is_control),
        "unescaped control character in {:?}",
        warnings[0]
    );
}

/// `take_record` empties the record on the way out, which covers a phase
/// that reaches its policy section. A phase that does not -- a launch
/// bootstrap that wrote settings without printing a policy section, a
/// profile seed, another test on this thread -- leaves its outcomes behind,
/// and the next run would read them as its own. `clear_record` is what the
/// write phase calls before the Claude writers run.
#[test]
fn clearing_the_record_discards_what_was_recorded_before_it() {
    clear();
    record("home settings", &DenyOutcome::Written);
    record(
        "profile work",
        &DenyOutcome::Unparseable("permissions.deny is not a JSON array".to_string()),
    );

    clear_record();

    let (outcome, warnings) = take_record();
    assert_eq!(
        outcome,
        HarnessWrite::NothingToWriteInto,
        "nothing recorded before the clear belongs to this run"
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// Clearing does not disable the record: what the write phase records after
/// the clear is reported as usual.
#[test]
fn what_is_recorded_after_the_clear_is_still_reported() {
    clear();
    record("home settings", &DenyOutcome::Written);
    clear_record();
    record("profile work", &DenyOutcome::Written);
    let (outcome, warnings) = take_record();
    assert_eq!(outcome, HarnessWrite::Written);
    assert!(warnings.is_empty(), "{warnings:?}");
}
