//! #2966 and the live-install rule (#3049): `finalization_fail_or_handoff`.
//!
//! The validator's old drop-in for fail_result handed off on the run's own PR
//! number. #2966 removed its only caller in the bundle, and the first review of
//! the fix asked for the name to keep working for one release, for a script
//! written against the earlier release. It is a deprecated alias of
//! `finalization_fail_or_handoff_open_pr`: it warns, then hands off only a pull
//! request the assessment named, and nothing in the bundle calls it.

use super::fixture::*;
use super::static_guards::{definers, recipe_files, rel, tool_files};

// #2966: the validator no longer answers BLOCKED_CI from the probe's record, so
// the drop-in that handed off on the run's own PR number has no caller in the
// bundle. A hand-off goes only through the *_open_pr functions, which need the
// pull request number the assessment returned. The old name stays defined for
// one release under the live-install rule (#3049), in the hand-off brick only,
// as an alias of the *_open_pr drop-in (brick.rs runs it); nothing calls it.
#[test]
fn the_prior_state_handoff_helper_is_only_an_alias_nothing_calls() {
    let holders = definers("finalization_fail_or_handoff()");
    assert_eq!(
        holders,
        [format!("amplifier-bundle/tools/{BRICK}")],
        "finalization_fail_or_handoff() is defined only as the hand-off brick's alias"
    );
    let brick_text = read(&tool_path(BRICK));
    let body = brick_text
        .split_once("finalization_fail_or_handoff() {")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("the alias body");
    let calls: Vec<&str> = code_lines(body)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        calls.last().copied(),
        Some("finalization_fail_or_handoff_open_pr \"$@\""),
        "the alias delegates to the open-PR drop-in and does nothing else: {calls:?}"
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
            "{}: nothing in the bundle may call the deprecated alias: {callers:?}",
            rel(&path)
        );
    }
}

// #2966 and the live-install rule (#3049): `finalization_fail_or_handoff` was
// the validator's drop-in for fail_result that handed off on the run's own PR
// number. #2966 removed its only caller in the bundle. A script written against
// the earlier release may still call it, so for one release it remains, as a
// deprecated alias that warns and does exactly what
// `finalization_fail_or_handoff_open_pr` does: it hands off only a pull request
// the assessment named, and otherwise fails closed through fail_result.
#[test]
fn the_old_helper_name_is_a_warning_alias_for_the_open_pr_helper() {
    let script = r#"fail_result() { echo "fail_result $1 | $2"; exit 1; }
emit_result() { echo '{"terminal_state":"'"$terminal_state"'","pr_number":"'"$pr_number"'"}'; }
_finalization_pr_number_valid() { [[ "${1:-}" =~ ^[1-9][0-9]*$ ]]; }
RECIPE_VAR_finalization_handoff=true
open_pr_confirmed_number=""
[ "$CASE" != named ] || open_pr_confirmed_number=7
pr_number=42
finalization_fail_or_handoff BLOCKED_CI "deploy: no secret" "admin: add it" "evidence""#;
    for (case, success, stdout) in [
        ("named", true, r#""pr_number":"7""#),
        (
            "unnamed",
            false,
            "fail_result BLOCKED_CI | deploy: no secret",
        ),
    ] {
        let outcome = brick(script, &[("CASE", case)]);
        assert_eq!(outcome.success(), success, "{case}: {}", outcome.describe());
        assert!(
            outcome.stdout.contains(stdout),
            "{case}: want `{stdout}`\n{}",
            outcome.describe()
        );
        assert!(
            !outcome.stdout.contains(r#""pr_number":"42""#),
            "{case}: the run's own PR number is never the one handed off\n{}",
            outcome.describe()
        );
        assert!(
            outcome
                .stderr
                .contains("WARNING: #2966: finalization_fail_or_handoff is deprecated"),
            "{case}: the alias says it is deprecated\n{}",
            outcome.describe()
        );
    }
}
