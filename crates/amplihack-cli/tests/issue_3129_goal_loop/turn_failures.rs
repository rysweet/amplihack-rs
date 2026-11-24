//! #3129 R1: a turn run that fails does not end the run.
//!
//! Each test runs the real binary, the pinned runner and the real recipes
//! against the stand-in gh and the stub agent of `support`, and reads only
//! what the run produced. Each fails on ccb445a9, where any failed turn run
//! ended the run as `failed` or `usage_limit`.
//!
//! Nothing sleeps, polls or times out. Where the run must be held while the
//! test sends guidance, the stub agent blocks on the harness FIFO and the test
//! releases it (`fifo_release`).

#![cfg(target_os = "linux")]

use super::support::{Harness, MERGE_WHEN_GREEN, PR_BRANCH, describe, git};
use crate::support::fifo_release;
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

/// The agent step fails with a usage limit before it decides. The run does
/// not end: the next turn starts (its supervised agent step moves login or
/// waits for the reset), or, where the runner could not supervise the step,
/// the run waits for the owner's word, which the test sends during the turn.
/// The message carries the classifier's marker but not the harness's own
/// limit shape, so no real limit wait starts in the test.
#[test]
fn a_usage_limit_in_a_turn_never_ends_the_run() {
    let h = Harness::new();
    h.policy(&first_turn_then_merge(
        r#"
  read -r _ < "$STUB_FIFO"
  echo "error: this account reached its usage limit for now" >&2
  exit 1
"#,
    ));
    let mut live = fifo_release::start(
        h.drive_command("auto-drive-to-merge", &["no_merge=false"]),
        &h.fifo,
        "the drive",
    );
    h.guide("The login has room again; carry on.");
    let out = live.finish();
    ends_merged_on_turn_two(&h, &out);
    let first = h.result("001");
    assert_eq!(first["failure"]["usage_limit"], json!(true), "{first}");
    assert!(
        first["recovery"] == json!("next-turn") || first["recovery"] == json!("owner-wait"),
        "{first}"
    );
    assert_ne!(h.loop_end()["end"], json!("usage_limit"));
}

/// The agent step fails before it decides, for a reason a fresh turn would
/// likely meet again at once. The run waits for the owner's word (an event,
/// no timer); guidance sent during the turn wakes it, and the next turn runs.
#[test]
fn a_turn_that_fails_before_its_agent_decides_waits_for_the_owner_and_then_carries_on() {
    let h = Harness::new();
    h.policy(&first_turn_then_merge(
        r#"
  read -r _ < "$STUB_FIFO"
  echo "the agent process stopped unexpectedly" >&2
  exit 1
"#,
    ));
    let mut live = fifo_release::start(
        h.drive_command("auto-drive-to-merge", &["no_merge=false"]),
        &h.fifo,
        "the drive",
    );
    h.guide("I fixed the host. Carry on.");
    let out = live.finish();
    ends_merged_on_turn_two(&h, &out);
    let first = h.result("001");
    assert_eq!(first["recovery"], json!("owner-wait"), "{first}");
    assert_eq!(first["failure"]["agent_finished"], json!(false), "{first}");
    let second = h.reading("002");
    assert!(
        second["guidance"]
            .as_array()
            .is_some_and(|g| g.iter().any(|e| e["new"] == json!(true))),
        "the owner's word is new to the next turn: {second}"
    );
}
