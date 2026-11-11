//! Crusty round 5 on #1831: the capacity governor and the usage-limit
//! supervisor (#1618, #1801) around one exhausted limit. Inside a running
//! recipe only the supervisor waits; the governor admits the step, so its
//! own shorter wait never ends the run with exit 80. Runs the real
//! `amplihack` binary with the governor on, a recipe nested under a running
//! recipe's slot, and a stub `claude` that prints the provider's limit
//! message. The supervisor's wait is virtual (the debug-only clock seams
//! the #1618 tests use), so no test sleeps. See "Exhausted limits inside a
//! recipe" in docs/reference/capacity-governor.md.

#![cfg(target_os = "linux")]

use crate::issue_1811_fixture::{Fixture, describe, text};
use crate::support::exec_stub::write_executable;
use std::fs;
use std::process::Output;

const DENIED: i32 = 80;
/// 2026-10-18 08:00:00 -07:00, as in the #1618 fixture.
const TEST_NOW: i64 = 1_792_335_600;
/// The provider's session-limit line, with a reset 30 minutes after
/// `TEST_NOW`.
const SESSION_TEXT: &str = "You've hit your session limit · resets 8:30am (America/Los_Angeles)";
/// The provider's weekly-limit line, with a reset months away.
const WEEKLY_TEXT: &str = "You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)";

/// Counts its calls in `$RECORD_DIR/count`; for the first `$LIMIT_CALLS`
/// prints `$LIMIT_MESSAGE` on stderr and exits 1, otherwise exits 0.
const LIMITED_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
n=$(( $(cat "$RECORD_DIR/count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$RECORD_DIR/count"
echo "attempt $n output"
if [ "$n" -le "${LIMIT_CALLS:-0}" ]; then
  printf '%s\n' "$LIMIT_MESSAGE" >&2
  exit 1
fi
exit 0
"#;

/// Runs the fixture's two-step recipe nested under a running recipe's slot,
/// with the usage-limit wait on and the stub `claude` limited once with
/// `message`.
fn nested_recipe(f: &Fixture, message: &str) -> (Output, String, u32) {
    write_executable(&f.tools.join("claude"), LIMITED_CLAUDE, 0o755);
    f.parent_slot("t-recipe", "recipe");
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .env("AMPLIHACK_GOVERNOR_PARENT", "t-recipe")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT", "1")
        .env("TZ", "America/Los_Angeles")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW", TEST_NOW.to_string())
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "1000000")
        .env("LIMIT_CALLS", "1")
        .env("LIMIT_MESSAGE", message)
        .output()
        .unwrap();
    let steps = fs::read_to_string(f.records.join("runner.out")).unwrap_or_default();
    let calls = fs::read_to_string(f.records.join("count"))
        .map(|s| s.trim().parse().unwrap())
        .unwrap_or(0);
    (out, steps, calls)
}

fn full(out: &Output, steps: &str) -> String {
    format!("{}\nsteps: {steps}", describe(out))
}

/// Every login session-exhausted: the governor admits the step, the
/// provider's limit message reaches the supervisor, which waits for the
/// stated reset and runs the step again, and the recipe finishes. Before
/// this round the governor held the step at step 3 and it exited 80 when
/// the governor's wait ran out.
#[test]
fn session_exhausted_step_waits_for_the_reset_and_the_recipe_finishes() {
    let f = Fixture::new();
    f.readings(&[("alpha", 100.0, 50.0), ("beta", 100.0, 60.0)]);
    let (out, steps, calls) = nested_recipe(&f, SESSION_TEXT);
    assert!(out.status.success(), "{}", full(&out, &steps));
    assert_eq!(calls, 3, "step 1 twice (limited, then rerun), step 2 once");
    assert!(
        !steps.contains("governor: not started") && !text(&out.stderr).contains("not started"),
        "the governor never held the step: {}",
        full(&out, &steps)
    );
    let waits: Vec<&str> = steps
        .lines()
        .filter_map(|l| l.strip_prefix("amplihack.recipe.usage_limit_wait "))
        .filter(|l| !l.starts_with("progress "))
        .collect();
    assert_eq!(
        waits
            .first()
            .map(|l| l.starts_with("start reason=usage_limit ")),
        Some(true),
        "the supervisor waited for the provider's reset: {}",
        full(&out, &steps)
    );
    assert!(
        waits.contains(&"end reason=reset reached") && waits.contains(&"resume attempt=2"),
        "{}",
        full(&out, &steps)
    );
}

/// Every login weekly-exhausted, the reset months away: the governor admits
/// the step and the supervisor alone decides. It does not wait (the reset is
/// too far and no login can take over), so the step fails once with the
/// provider's exit code, never the governor's 80. This pins that one
/// component decides, rather than two that agree only while their
/// eligibility rules match.
#[test]
fn weekly_exhausted_step_is_decided_by_the_supervisor_not_the_governor() {
    let f = Fixture::new();
    f.readings(&[("alpha", 0.0, 100.0), ("beta", 0.0, 100.0)]);
    let (out, steps, calls) = nested_recipe(&f, WEEKLY_TEXT);
    assert!(!out.status.success(), "{}", full(&out, &steps));
    assert_ne!(out.status.code(), Some(DENIED), "{}", full(&out, &steps));
    assert!(
        !steps.contains("governor: not started") && !text(&out.stderr).contains("not started"),
        "{}",
        full(&out, &steps)
    );
    assert_eq!(calls, 1, "the step ran once and was not rerun");
    assert!(
        steps.contains("amplihack.recipe.usage_limit_wait skipped reason=reset_too_far"),
        "the supervisor declined the wait: {}",
        full(&out, &steps)
    );
}
