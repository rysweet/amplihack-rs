//! #3129 R1: a turn run that fails does not end the run.
//!
//! Each test runs the real binary, the pinned runner and the real recipes
//! against the stand-in gh and the stub agent of `support`, and reads only
//! what the run produced. Each fails on ccb445a9, where any failed turn run
//! ended the run as `failed` or `usage_limit`, except the one usage-limit
//! ending the operator's own setting asks for.
//!
//! Nothing sleeps, polls or times out, and none of these runs waits for
//! guidance. The one failure that does, a launch bundle that lost a file a
//! turn needs, is tested with the stand-in goal in `contract`.

#![cfg(target_os = "linux")]

use super::support::{Harness, MERGE_WHEN_GREEN, PR_BRANCH, describe, git};
use serde_json::json;

/// Turn 1: `$TURN_ONE` (bash), once. Every later turn: merge when green.
fn first_turn_then_merge(turn_one: &str) -> String {
    format!(
        r#"
if [ ! -e "$STUB_LOG/t1" ]; then
  : > "$STUB_LOG/t1"
{turn_one}
fi
{MERGE_WHEN_GREEN}
"#
    )
}

fn ends_merged_on_turn_two(h: &Harness, out: &std::process::Output) {
    assert_eq!(
        h.loop_end()["end"],
        json!("merged"),
        "the run carried on and merged: {}\n{}",
        h.loop_end(),
        describe(out)
    );
    assert_eq!(h.turns(), vec!["001", "002"], "{}", describe(out));
    assert_eq!(h.result("001")["outcome"], json!("turn-failed"));
    let previous = &h.reading("002")["previous"];
    assert_eq!(
        previous["outcome"],
        json!("turn-failed"),
        "the next turn's agent is given the failure: {previous}"
    );
    assert!(
        previous["failure"]["log"]
            .as_str()
            .is_some_and(|l| l.ends_with("/turns/001/turn.log")),
        "the failure names the turn log: {previous}"
    );
    assert_eq!(h.merge_calls().len(), 1, "{:?}", h.calls());
}

/// The agent finishes deciding, and the step after it (saving the decision)
/// fails: here the agent's own work left its turn folder at mode 0755, which
/// the save step refuses. The work the agent pushed is kept, the decision is
/// not carried out, and the next turn starts at once.
#[test]
fn a_step_that_fails_after_the_agent_pushed_work_keeps_the_work_and_the_next_turn_runs() {
    let h = Harness::new();
    h.policy(&first_turn_then_merge(&format!(
        r#"
  printf 'line four, the agent'"'"'s fix\n' >> widget.txt
  git commit -qam "the agent's fix" && git push -q origin HEAD:{PR_BRANCH}
  chmod 0755 "$TURN"
  echo '{{"action":"continue","args":{{}},"why":"I pushed a fix."}}'
  exit 0
"#
    )));
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    ends_merged_on_turn_two(&h, &out);
    let first = h.result("001");
    assert_eq!(first["recovery"], json!("next-turn"), "{first}");
    assert_eq!(first["failure"]["agent_finished"], json!(true), "{first}");
    assert_eq!(
        git(
            &h.origin,
            &[
                "log",
                "-1",
                "--format=%s",
                &format!("refs/heads/{PR_BRANCH}")
            ]
        ),
        "the agent's fix",
        "the pushed work is kept"
    );
}

/// The agent step fails with a usage limit before it decides, with the
/// operator's limit wait on (the default). The run does not end, and nothing
/// waits for the owner: the next turn starts at once with the evidence. The
/// message carries the classifier's marker but not the harness's own limit
/// shape, so no real limit wait starts in the test.
#[test]
fn a_usage_limit_in_a_turn_never_ends_the_run() {
    let h = Harness::new();
    h.policy(&first_turn_then_merge(
        r#"
  echo "error: this account reached its usage limit for now" >&2
  exit 1
"#,
    ));
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    ends_merged_on_turn_two(&h, &out);
    let first = h.result("001");
    assert_eq!(first["failure"]["usage_limit"], json!(true), "{first}");
    assert_eq!(first["failure"]["class"], json!("usage_limit"), "{first}");
    assert_eq!(
        first["failure"]["usage_limit_wait"],
        json!(true),
        "the operator's setting was read, not the log: {first}"
    );
    assert_eq!(first["recovery"], json!("next-turn"), "{first}");
    assert_ne!(h.loop_end()["end"], json!("usage_limit"));
}

/// R1's one usage-limit ending, kept on purpose: the operator's own
/// `recipe.usage_limit_wait = false` asks for a limit to be passed up at once.
/// The driver reads that setting itself (`amplihack config get`), so the
/// ending follows the setting and not a log line.
#[test]
fn a_usage_limit_with_the_operators_limit_wait_off_ends_the_run_as_usage_limit() {
    let h = Harness::new();
    h.policy(&first_turn_then_merge(
        r#"
  echo "error: this account reached its usage limit for now" >&2
  exit 1
"#,
    ));
    let mut c = h.drive_command("auto-drive-to-merge", &["no_merge=false"]);
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT", "0");
    let out = c.output().expect("amplihack recipe run");
    assert_eq!(
        h.loop_end()["end"],
        json!("usage_limit"),
        "{}\n{}",
        h.loop_end(),
        describe(&out)
    );
    assert_eq!(h.turns(), vec!["001"], "{}", describe(&out));
    let first = h.result("001");
    assert_eq!(first["recovery"], json!("ended"), "{first}");
    assert_eq!(
        first["failure"]["usage_limit_wait"],
        json!(false),
        "{first}"
    );
    assert!(h.merge_calls().is_empty(), "{:?}", h.calls());
}

/// The agent step fails before it decides (R1's pre-decision case). Nothing
/// waits for the owner: the next turn starts at once, and its agent is given
/// the failure's evidence. On 47b21de0 this run blocks in the owner wait (or
/// ends in a hand-off where no wake can be armed) and never reaches turn 2.
#[test]
fn a_turn_that_fails_before_its_agent_decides_starts_the_next_turn_with_the_evidence() {
    let h = Harness::new();
    h.policy(&first_turn_then_merge(
        r#"
  echo "the agent process stopped unexpectedly" >&2
  exit 1
"#,
    ));
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    ends_merged_on_turn_two(&h, &out);
    let first = h.result("001");
    assert_eq!(first["recovery"], json!("next-turn"), "{first}");
    assert_eq!(first["failure"]["agent_finished"], json!(false), "{first}");
    assert!(
        !h.state.join("turns/001/owner-wait.json").exists(),
        "no owner wait ran"
    );
    let failure = &h.reading("002")["previous"]["failure"];
    assert!(failure["class"].is_string(), "{failure}");
    assert!(failure["completed_steps"].is_array(), "{failure}");
    assert!(failure["failed_steps"].is_array(), "{failure}");
}
