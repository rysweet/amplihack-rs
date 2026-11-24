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
