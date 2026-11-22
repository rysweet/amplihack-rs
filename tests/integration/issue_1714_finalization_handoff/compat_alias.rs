//! #2966 and the live-install rule (#3049): `finalization_fail_or_handoff`.
//!
//! The validator's old drop-in for fail_result handed off on the run's own PR
//! number. #2966 removed its only caller in the bundle, and the first review of
//! the fix asked for the name to keep working for one release, for a script
//! written against the earlier release. The second review found that the name
//! had been kept with different behaviour: it delegated to
//! `finalization_fail_or_handoff_open_pr`, which hands off only a pull request
//! the assessment named. It now keeps the body it had before #2966, behind a
//! deprecation warning, and nothing in the bundle calls it.

use super::fixture::*;
use super::static_guards::{definers, recipe_files, rel, tool_files};

// #2966: the validator no longer answers BLOCKED_CI from the probe's record, so
// the drop-in that handed off on the run's own PR number has no caller in the
// bundle. A hand-off from the bundle goes only through the *_open_pr
// functions, which need the pull request number the assessment returned. The
// old name stays defined for one release under the live-install rule (#3049),
// in the hand-off brick only, with its old body (brick.rs runs it); nothing
// calls it.
#[test]
fn the_prior_state_handoff_helper_is_only_a_deprecated_definition_nothing_calls() {
    let holders = definers("finalization_fail_or_handoff()");
    assert_eq!(
        holders,
        [format!("amplifier-bundle/tools/{BRICK}")],
        "finalization_fail_or_handoff() is defined only in the hand-off brick"
    );
    let brick_text = read(&tool_path(BRICK));
    let body = brick_text
        .split_once("finalization_fail_or_handoff() {")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("the deprecated helper's body");
    let calls: Vec<&str> = code_lines(body)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        calls.len(),
        3,
        "a warning and the two lines of the body before #2966: {calls:?}"
    );
    assert!(
        calls[0].starts_with("echo \"WARNING: #2966: finalization_fail_or_handoff is deprecated"),
        "it warns first: {calls:?}"
    );
    assert_eq!(
        &calls[1..],
        [
            "_finalization_handoff_emit \"$@\" || :",
            "fail_result \"$@\""
        ],
        "the body is the one it had before #2966: {calls:?}"
    );
    for path in tool_files().into_iter().chain(recipe_files()) {
        let text = read(&path);
        let callers: Vec<&str> = code_lines(&text)
            .filter(|l| {
                let l = l.trim();
                l.starts_with("finalization_fail_or_handoff ")
                    || ["; ", "&& ", "|| ", "$(", "then ", "do "]
                        .iter()
                        .any(|p| l.contains(&format!("{p}finalization_fail_or_handoff ")))
            })
            .collect();
        assert!(
            callers.is_empty(),
            "{}: nothing in the bundle may call the deprecated helper: {callers:?}",
            rel(&path)
        );
    }
}

// Crusty round 2 (#2966, concern 3): a script written against the earlier
// release gets the behaviour it was written for. With the hand-off flag set and
// a state auto-drive is handed, the helper hands off on the validator's current
// pr_number, the run's own pull request, whatever the assessment named. Without
// the flag, or for a state that is never handed off, or with no valid number,
// it fails closed through fail_result. It warns on stderr every time.
#[test]
fn the_deprecated_helper_behaves_as_it_did_before_2966() {
    let script = r#"fail_result() { echo "fail_result $1 | $2"; exit 1; }
emit_result() { echo '{"terminal_state":"'"$terminal_state"'","pr_number":"'"$pr_number"'"}'; }
_finalization_pr_number_valid() { [[ "${1:-}" =~ ^[1-9][0-9]*$ ]]; }
RECIPE_VAR_finalization_handoff="$FLAG"
open_pr_confirmed_number=7
pr_number="$OWN_PR"
finalization_fail_or_handoff "$STATE" "deploy: no secret" "admin: add it" "evidence""#;
    for (flag, state, own_pr, success, stdout) in [
        // The old behaviour: the run's own number, not the assessment's.
        (
            "true",
            "BLOCKED_CI",
            "42",
            true,
            r#""handoff":"merge_ready_loop""#,
        ),
        // Standalone: no hand-off, fail_result as before.
        (
            "false",
            "BLOCKED_CI",
            "42",
            false,
            "fail_result BLOCKED_CI | deploy: no secret",
        ),
        // A state that is never handed off fails closed.
        (
            "true",
            "FAILED_DIRTY_WORKTREE",
            "42",
            false,
            "fail_result FAILED_DIRTY_WORKTREE | deploy: no secret",
        ),
        // No valid number of its own: nothing to hand off, as before.
        (
            "true",
            "BLOCKED_CI",
            "",
            false,
            "fail_result BLOCKED_CI | deploy: no secret",
        ),
    ] {
        let case = format!("flag={flag} state={state} own_pr={own_pr:?}");
        let outcome = brick(
            script,
            &[("FLAG", flag), ("STATE", state), ("OWN_PR", own_pr)],
        );
        assert_eq!(outcome.success(), success, "{case}: {}", outcome.describe());
        assert!(
            outcome.stdout.contains(stdout),
            "{case}: want `{stdout}`\n{}",
            outcome.describe()
        );
        if success {
            assert!(
                outcome.stdout.contains(r#""pr_number":"42""#),
                "{case}: the hand-off names the run's own pull request\n{}",
                outcome.describe()
            );
        }
        assert!(
            !outcome.stdout.contains(r#""pr_number":"7""#),
            "{case}: the deprecated helper never adopts the assessment's number\n{}",
            outcome.describe()
        );
        assert!(
            outcome
                .stderr
                .contains("WARNING: #2966: finalization_fail_or_handoff is deprecated"),
            "{case}: the helper says it is deprecated\n{}",
            outcome.describe()
        );
    }
}
