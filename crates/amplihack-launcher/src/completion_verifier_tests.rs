use super::*;

#[allow(clippy::too_many_arguments)]
fn make_signals(
    all_steps: bool,
    pr_created: bool,
    ci: bool,
    mergeable: bool,
    commits: bool,
    clean: bool,
    score: f64,
    pr_num: Option<u32>,
) -> CompletionSignals {
    CompletionSignals {
        all_steps_complete: all_steps,
        pr_created,
        ci_passing: ci,
        pr_mergeable: mergeable,
        has_commits: commits,
        no_uncommitted_changes: clean,
        completion_score: score,
        pr_number: pr_num,
    }
}

#[test]
fn parse_claim_explicit_complete() {
    assert!(CompletionVerifier::parse_completion_claim(
        "Evaluation: complete"
    ));
}

#[test]
fn parse_claim_explicit_incomplete() {
    assert!(!CompletionVerifier::parse_completion_claim(
        "Evaluation: incomplete"
    ));
}

#[test]
fn parse_claim_implicit_done() {
    assert!(CompletionVerifier::parse_completion_claim(
        "All tasks are done."
    ));
}

#[test]
fn parse_claim_implicit_pending() {
    assert!(!CompletionVerifier::parse_completion_claim(
        "There are still tasks pending."
    ));
}

#[test]
fn parse_claim_empty() {
    assert!(!CompletionVerifier::parse_completion_claim(""));
}

#[test]
fn parse_claim_ambiguous_defaults_false() {
    assert!(!CompletionVerifier::parse_completion_claim(
        "Some random text about nothing"
    ));
}

#[test]
fn verified_complete() {
    let verifier = CompletionVerifier::default();
    let signals = make_signals(true, true, true, true, true, true, 1.0, Some(42));
    let result = verifier.verify("Evaluation: complete. All tasks completed.", &signals);
    assert_eq!(result.status, VerificationStatus::Verified);
    assert!(result.verified);
}

#[test]
fn disputed_false_claim() {
    let verifier = CompletionVerifier::default();
    let signals = make_signals(false, false, false, false, false, true, 0.05, None);
    let result = verifier.verify("Everything is done!", &signals);
    assert_eq!(result.status, VerificationStatus::Disputed);
    assert!(!result.verified);
}

#[test]
fn verified_correctly_incomplete() {
    let verifier = CompletionVerifier::default();
    let signals = make_signals(false, false, false, false, false, true, 0.05, None);
    // Note: "still working" doesn't contain "pr" substring, avoiding false PR detection
    let result = verifier.verify("Still working on the tasks.", &signals);
    assert_eq!(result.status, VerificationStatus::Verified);
    assert!(result.verified);
}

#[test]
fn disputed_overly_conservative() {
    let verifier = CompletionVerifier::default();
    let signals = make_signals(true, true, true, true, true, true, 1.0, Some(1));
    let result = verifier.verify("Still working on it, need to do more", &signals);
    assert_eq!(result.status, VerificationStatus::Disputed);
}

#[test]
fn ci_pending_incomplete_status() {
    let verifier = CompletionVerifier::default();
    // All steps done, PR created, but CI not passing — score just under threshold
    let signals = make_signals(true, true, false, false, true, true, 0.75, Some(5));
    let result =
        verifier.verify("All tasks completed. Evaluation: complete. CI still pending.", &signals);
    // Score 0.75 >= 0.7, CI pending acknowledged, pr_created, all_steps_complete
    assert_eq!(result.status, VerificationStatus::Incomplete);
}

#[test]
fn format_report_verified() {
    let result = VerificationResult {
        status: VerificationStatus::Verified,
        verified: true,
        explanation: "Work is complete".into(),
        discrepancies: vec![],
    };
    let report = CompletionVerifier::format_report(&result);
    assert!(report.contains("VERIFIED"));
    assert!(report.contains("✓"));
}

#[test]
fn format_report_disputed_with_discrepancies() {
    let result = VerificationResult {
        status: VerificationStatus::Disputed,
        verified: false,
        explanation: "Score too low".into(),
        discrepancies: vec!["No PR created".into(), "Tasks pending".into()],
    };
    let report = CompletionVerifier::format_report(&result);
    assert!(report.contains("DISPUTED"));
    assert!(report.contains("✗"));
    assert!(report.contains("Discrepancies found:"));
    assert!(report.contains("No PR created"));
}

#[test]
fn discrepancy_pr_mentioned_but_absent() {
    let verifier = CompletionVerifier::default();
    let signals = make_signals(false, false, false, false, false, true, 0.05, None);
    let result = verifier.verify("The PR is ready for review", &signals);
    assert!(
        result
            .discrepancies
            .iter()
            .any(|d| d.contains("mentions PR but no PR exists"))
    );
}

#[test]
fn discrepancy_ci_claim_mismatch() {
    let verifier = CompletionVerifier::default();
    let signals = make_signals(true, true, false, true, true, true, 0.8, Some(1));
    let result = verifier.verify("CI checks are passing", &signals);
    assert!(
        result
            .discrepancies
            .iter()
            .any(|d| d.contains("CI passing but CI status is not SUCCESS"))
    );
}
