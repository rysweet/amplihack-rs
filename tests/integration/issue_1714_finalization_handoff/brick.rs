//! Unit tests for `workflow_finalization_handoff.sh`: the exact-match
//! allow-list, the exact-match flag, idempotent sourcing and the absence of
//! top-level side effects.

use super::fixture::*;

fn allowed(state: &str) -> bool {
    let outcome = brick(r#"finalization_handoff_allowed "$ST""#, &[("ST", state)]);
    match outcome.output.status.code() {
        Some(0) => true,
        Some(1) => false,
        other => panic!(
            "{state:?}: unexpected exit {other:?}\n{}",
            outcome.describe()
        ),
    }
}

fn flag(value: Option<&str>, bare: Option<&str>) -> String {
    let mut envs = Vec::new();
    if let Some(value) = value {
        envs.push(("RECIPE_VAR_finalization_handoff", value));
    }
    if let Some(bare) = bare {
        envs.push(("FINALIZATION_HANDOFF", bare));
    }
    let outcome = brick("finalization_handoff_flag", &envs);
    assert!(outcome.success(), "{}", outcome.describe());
    assert!(outcome.stderr.is_empty(), "{}", outcome.describe());
    outcome.stdout.trim_end_matches('\n').to_owned()
}

// Exactly BLOCKED_CI and FAILED_IMPLEMENTATION may be handed off (#1714
// case 2); a checkout that is not the PR's (dirty, wrong branch) never is.
#[test]
fn only_blocked_ci_and_failed_implementation_are_allowed() {
    assert!(allowed("BLOCKED_CI"));
    assert!(allowed("FAILED_IMPLEMENTATION"));
    for state in [
        "FAILED_CLOSED_UNMERGED",
        "FAILED_PR_METADATA_UNAVAILABLE",
        "FAILED_MEANINGFUL_DIFF",
        "FAILED_DIRTY_WORKTREE",
        "FAILED_WRONG_BRANCH",
        "FAILED_INVALID_INPUT",
        "FAILED_INVALID_EVIDENCE",
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "FAILED_REPORTING",
        "FAILED_MISSING_TOOLING",
        "HOLLOW_SUCCESS",
        "INCOMPLETE",
        "EXISTING_OPEN_PR",
        "IMPLEMENTED_VERIFIED",
        "MERGED",
    ] {
        assert!(!allowed(state), "{state} must not be handed off");
    }
}

// T8: no case folding, no trimming, no globbing.
#[test]
fn near_misses_are_rejected() {
    for state in [
        "",
        "blocked_ci",
        "Blocked_CI",
        " BLOCKED_CI",
        "BLOCKED_CI ",
        "BLOCKED_CI\n",
        "BLOCKED_*",
        "BLOCKED_CI|FAILED_IMPLEMENTATION",
        "failed_implementation",
        "FAILED_IMPLEMENTATION ",
        "FAILED_*",
        "*",
    ] {
        assert!(!allowed(state), "{state:?} must be rejected");
    }
}

// T2: only exactly `true` in RECIPE_VAR_finalization_handoff switches it on.
#[test]
fn the_flag_is_on_only_for_exactly_true() {
    assert_eq!(flag(Some("true"), None), "true");
    for value in [
        None,
        Some(""),
        Some("1"),
        Some("yes"),
        Some("on"),
        Some("TRUE"),
        Some("True"),
        Some(" true"),
        Some("true "),
        Some("true\n"),
    ] {
        assert_eq!(flag(value, None), "false", "RECIPE_VAR={value:?}");
    }
    assert_eq!(
        flag(None, Some("true")),
        "false",
        "a bare FINALIZATION_HANDOFF is never read"
    );
    assert_eq!(flag(Some(""), Some("true")), "false");
}

// T10: sourcing twice under `set -e` works, and the constants are not readonly.
#[test]
fn the_brick_can_be_sourced_twice() {
    let outcome = brick(
        r#"set -euo pipefail
. "$BRICK_PATH"
printf '%s %s\n' "$FINALIZATION_HANDOFF_FIELD" "$FINALIZATION_HANDOFF_VALUE"
FINALIZATION_HANDOFF_VALUE=other
FINALIZATION_HANDOFF_VALUE=merge_ready_loop
echo sourced-twice"#,
        &[("BRICK_PATH", tool_path(BRICK).to_str().unwrap())],
    );
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(
        outcome.stdout,
        "handoff merge_ready_loop\nsourced-twice\n",
        "{}",
        outcome.describe()
    );
}

// The brick defines functions and constants only: sourcing it changes no
// directory, umask or shell option and writes nothing.
#[test]
fn sourcing_has_no_top_level_side_effects() {
    let script = r#"cd /; umask 0027; set +e +u
before="$(pwd) $(umask) $- $(set -o | tr -s ' \t' ' ' | tr '\n' ,)"
. "$BRICK_PATH" > "$TMPDIR/brick.out" 2>&1
after="$(pwd) $(umask) $- $(set -o | tr -s ' \t' ' ' | tr '\n' ,)"
[ "$before" = "$after" ] || { echo "changed: $before -> $after"; exit 3; }
[ ! -s "$TMPDIR/brick.out" ] || { echo "output:"; cat "$TMPDIR/brick.out"; exit 4; }
for f in finalization_handoff_allowed finalization_handoff_flag finalization_fail_or_handoff autodrive_build_handoff; do
  [ "$(type -t "$f")" = function ] || { echo "missing function $f"; exit 5; }
done
echo clean"#;
    let outcome = brick(
        r#"bash -c "$SCRIPT""#,
        &[
            ("BRICK_PATH", tool_path(BRICK).to_str().unwrap()),
            ("SCRIPT", script),
        ],
    );
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.stdout.trim(), "clean", "{}", outcome.describe());
}

fn quoted_states(list: &str) -> std::collections::BTreeSet<String> {
    list.split(',')
        .map(|s| s.trim().trim_matches('"').to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

// `_handoff_known_state` copies the finaliser's `terminal_vocabulary`; a
// state added to one list must be added to the other.
#[test]
fn known_states_match_the_terminal_vocabulary() {
    let complete = read(&tool_path("workflow_finalization_complete.sh"));
    let vocab = complete
        .split_once("terminal_vocabulary:[")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| quoted_states(list))
        .expect("terminal_vocabulary:[...] in workflow_finalization_complete.sh");
    let brick_text = read(&tool_path(BRICK));
    let (_, body) = brick_text
        .split_once("_handoff_known_state() {")
        .expect("_handoff_known_state in the brick");
    let known: std::collections::BTreeSet<String> = body
        .lines()
        .find_map(|l| l.trim().strip_suffix(") return 0 ;;"))
        .expect("the case line of _handoff_known_state")
        .split('|')
        .map(str::to_owned)
        .collect();
    for (name, set) in [
        ("terminal_vocabulary", &vocab),
        ("_handoff_known_state", &known),
    ] {
        assert!(
            set.contains("BLOCKED_CI"),
            "{name} parsed without BLOCKED_CI: {set:?}"
        );
    }
    assert!(
        vocab == known,
        "lists differ\nonly in terminal_vocabulary: {:?}\nonly in _handoff_known_state: {:?}",
        vocab.difference(&known).collect::<Vec<_>>(),
        known.difference(&vocab).collect::<Vec<_>>()
    );
}
