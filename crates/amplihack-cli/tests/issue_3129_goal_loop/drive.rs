//! #3129: the nine behaviour tests of the goal loop (D19).
//!
//! Each one runs the real `amplihack` binary and the pinned runner against a
//! byte-for-byte copy of the bundle, the stand-in `gh`, a bare origin that
//! refuses non-fast-forward pushes and deletes, and a stub agent that picks
//! its action by a fixed rule (see `support`). No test reads recipe or source
//! text; they read what the run produced.
//!
//! Nothing sleeps, polls or times out. Where a test needs the run held at a
//! point (a check wait, an agent turn), the stub blocks on a FIFO and the test
//! releases it with `fifo_release`; if the run exits early the harness's
//! watcher unblocks the test, which then fails with the run's output.

#![cfg(target_os = "linux")]

use super::support::{Harness, MERGE_WHEN_GREEN, PR_BRANCH, describe};
use crate::support::fifo_release;
use std::fs;

fn ok(out: &std::process::Output) {
    assert!(out.status.success(), "{}", describe(out));
}

fn no_draft_conversion(h: &Harness) {
    for c in h.calls() {
        assert!(
            !c.contains("--undo"),
            "a call converted the PR to draft: {c}"
        );
        assert!(!c.starts_with("pr ready"), "unexpected pr ready call: {c}");
    }
}

fn every_merge_call_is_safe(h: &Harness) {
    for c in h.merge_calls() {
        assert!(c.contains("--match-head-commit"), "{c}");
        assert!(
            c.contains("--merge") || c.contains("--squash"),
            "an allowed method: {c}"
        );
        assert!(!c.contains("--admin") && !c.contains("--rebase"), "{c}");
    }
}

// 1. A branch that is only behind, with no strict checks, gets no base merge.
#[test]
fn only_behind_with_no_strict_checks_gets_no_base_merge_and_the_pr_merges() {
    let h = Harness::new();
    h.advance_base(false);
    h.policy(MERGE_WHEN_GREEN);
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    ok(&out);
    assert_eq!(
        h.origin_head(PR_BRANCH),
        h.initial_head,
        "the remote's PR branch never moved"
    );
    assert!(
        h.merge_commits_on(PR_BRANCH).is_empty(),
        "no merge commit from the base"
    );
    assert_eq!(h.merge_calls().len(), 1, "{:?}", h.calls());
    assert!(h.merge_calls()[0].contains(&format!("--match-head-commit {}", h.initial_head)));
    every_merge_call_is_safe(&h);
    no_draft_conversion(&h);
    assert_eq!(h.loop_end()["end"], "merged");
}

// 2. A required check still running makes the run wait; it merges when the
//    check completes.
#[test]
fn a_running_required_check_makes_the_run_wait_and_it_merges_when_the_check_completes() {
    let h = Harness::new();
    h.set_checks("scripts/check fast and full\tpending\t0\thttps://example.invalid\n");
    h.policy(MERGE_WHEN_GREEN);
    let mut live = fifo_release::start(
        h.drive_command("auto-drive-to-merge", &["no_merge=false"]),
        &h.gh.join("release.fifo"),
        "the drive",
    );
    // The stand-in `gh pr checks --watch` is blocked: the run is waiting.
    assert!(
        h.calls()
            .iter()
            .any(|c| c.starts_with("pr checks") && c.contains("--watch")),
        "{:?}",
        h.calls()
    );
    assert!(
        h.merge_calls().is_empty(),
        "no merge while a required check runs"
    );
    h.set_checks("scripts/check fast and full\tpass\t30m\thttps://example.invalid\n");
    let out = live.finish();
    ok(&out);
    let calls = h.calls();
    let watch = calls
        .iter()
        .position(|c| c.starts_with("pr checks") && c.contains("--watch"))
        .unwrap();
    let merge = calls
        .iter()
        .position(|c| c.starts_with("pr merge"))
        .expect("a merge call");
    assert!(watch < merge, "the merge came after the wait: {calls:?}");
    every_merge_call_is_safe(&h);
    no_draft_conversion(&h);
    assert_eq!(h.loop_end()["end"], "merged");
}

// 3. A base that conflicts is merged, the conflict resolved, the PR merged.
#[test]
fn a_conflicting_base_is_merged_by_merge_commit_resolved_and_the_pr_merges() {
    let h = Harness::new();
    h.advance_base(true);
    h.policy(&format!(
        r#"
n=$(jq -r '.goal.base.conflict_files | length' "$TURN/reading.json")
if [ "$n" != "0" ]; then
  git fetch -q origin
  git merge origin/main >/dev/null 2>&1 || true
  printf 'line one\nline two merged\nline three\n' > widget.txt
  git add widget.txt
  git commit -qm "Merge main into the PR branch"
  git push -q origin HEAD:{PR_BRANCH}
  echo '{{"action":"continue","args":{{}},"why":"The base conflicts with the PR, so I merged it and resolved widget.txt."}}'
else
{MERGE_WHEN_GREEN}
fi
"#
    ));
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    ok(&out);
    let head = h.origin_head(PR_BRANCH);
    assert_ne!(head, h.initial_head);
    let merges = h.merge_commits_on(PR_BRANCH);
    assert_eq!(merges.len(), 1, "one merge commit: {merges:?}");
    let parents = super::support::git(&h.origin, &["rev-list", "--parents", "-n", "1", &head]);
    assert_eq!(
        parents.split_whitespace().count(),
        3,
        "two parents: {parents}"
    );
    assert!(
        h.is_ancestor(&h.initial_head, &head),
        "the PR's original commits stay in its history"
    );
    assert_eq!(h.merge_calls().len(), 1);
    assert!(h.merge_calls()[0].contains(&format!("--match-head-commit {head}")));
    every_merge_call_is_safe(&h);
    no_draft_conversion(&h);
    assert_eq!(h.loop_end()["end"], "merged");
}

// 4. A missing .venv is installed by the repository's instructions, and the
//    run carries on.
#[test]
fn a_missing_venv_is_installed_by_the_repository_instructions_and_the_run_carries_on() {
    let h = Harness::new();
    h.policy(&format!(
        r#"
if [ ! -d .venv ]; then
  ./scripts/setup
  echo '{{"action":"continue","args":{{}},"why":"The README says scripts/setup creates .venv; it was missing, so I ran it."}}'
else
{MERGE_WHEN_GREEN}
fi
"#
    ));
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    ok(&out);
    assert!(h.repo.join(".venv").is_dir(), "the setup script ran");
    assert_eq!(
        h.loop_end()["end"],
        "merged",
        "the run carried on to the merge"
    );
    assert!(h.comments().is_empty(), "no hand-off: {:?}", h.comments());
}

// 5. A merging-off run (through auto-drive-light) goes on to the task's later
//    steps, and guidance asking for a merge produces no merge call.
#[test]
fn a_merging_off_run_does_the_later_task_step_and_ignores_guidance_asking_for_a_merge() {
    let h = Harness::new();
    h.guide("Please merge this pull request now.");
    h.policy(
        r#"
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
prev=$(jq -r '.previous.outcome // ""' "$TURN/reading.json")
guided=$(jq -r '[.guidance[]?] | length' "$TURN/reading.json")
if [ ! -f notes.txt ]; then
  echo "written by the task's later step" > notes.txt
  echo '{"action":"continue","args":{},"why":"The PR is ready; the task has one more step, so I did it."}'
elif [ "$guided" != "0" ] && [ "$prev" != "not-authorised" ]; then
  printf '{"action":"merge","args":{"head":"%s"},"why":"The guidance asks for a merge."}\n' "$head"
else
  echo '{"action":"ready","args":{"text":"The pull request is ready. Merging is off for this run, so the operator decides the merge."},"why":"Merging is off."}'
fi
"#,
    );
    let out = h.drive(
        "auto-drive-light",
        &["task_description=Get this pull request ready, then write notes.txt."],
    );
    ok(&out);
    assert!(
        h.repo.join("notes.txt").is_file(),
        "the task's later step ran"
    );
    assert!(
        h.merge_calls().is_empty(),
        "guidance cannot widen merge authority: {:?}",
        h.calls()
    );
    let end = h.loop_end();
    assert_eq!(end["end"], "ready", "{end}");
    assert!(
        h.comments().iter().any(|c| c.contains("Merging is off")),
        "{:?}",
        h.comments()
    );
    no_draft_conversion(&h);
}

// 6. A decision only the owner can make ends in a hand-off naming the owner
//    and the decision.
#[test]
fn a_decision_only_the_owner_can_make_ends_in_a_handoff_naming_owner_and_decision() {
    let h = Harness::new();
    h.policy(
        r#"
echo '{"action":"handoff","args":{"owner":"Data-Director","decision":"whether the schema change may ship before the migration","text":"The Data-Director decides whether the schema change may ship before the migration. The PR changes a column type. CI passed at this head."},"why":"Only the Data-Director can accept this."}'
"#,
    );
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    let end = h.loop_end();
    assert_eq!(end["end"], "handoff", "{end}\n{}", describe(&out));
    let comments = h.comments();
    assert_eq!(comments.len(), 1, "{comments:?}");
    assert!(comments[0].contains("Data-Director"), "{}", comments[0]);
    assert!(
        comments[0].contains("schema change may ship before the migration"),
        "{}",
        comments[0]
    );
    assert!(h.merge_calls().is_empty());
    no_draft_conversion(&h);
}

// 7. A retry against its own earlier PR, with a legacy per-PR state folder and
//    an earlier run's records, carries on to completed.
#[test]
fn a_retry_with_a_legacy_state_folder_and_earlier_records_carries_on_to_completed() {
    let h = Harness::new();
    let legacy = h.work.join("autodrive").join("github.com__o__r__pr-7");
    fs::create_dir_all(&legacy).unwrap();
    fs::write(
        legacy.join("finalize-merge.json"),
        r#"{"result":"refused","pr":7,"run_id":"earlier-run"}"#,
    )
    .unwrap();
    let earlier = h.state.join("turns").join("001");
    fs::create_dir_all(&earlier).unwrap();
    fs::write(
        earlier.join("record.md"),
        "Turn 001. An earlier run. It handed off.\n",
    )
    .unwrap();
    fs::write(
        h.state.join("loop-end.json"),
        r#"{"end":"handoff","run_id":"earlier-run","subject":"PR 7","detail":"old"}"#,
    )
    .unwrap();
    h.policy(MERGE_WHEN_GREEN);
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    ok(&out);
    let end = h.loop_end();
    assert_eq!(end["end"], "merged", "{end}");
    assert_ne!(end["run_id"], "earlier-run", "{end}");
    assert!(
        h.turns().contains(&"002".to_string()),
        "numbering continues: {:?}",
        h.turns()
    );
    assert_eq!(
        fs::read_to_string(earlier.join("record.md")).unwrap(),
        "Turn 001. An earlier run. It handed off.\n",
        "an earlier record is never rewritten"
    );
}

// 8. The old budget keys have no effect, through a recipe alias.
#[test]
fn the_retired_budget_keys_have_no_effect_through_the_merge_loop_alias() {
    let h = Harness::new();
    h.policy(&format!(
        r#"
n=$(( $(cat "$STUB_LOG/n" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$STUB_LOG/n"
if [ "$n" -lt 4 ]; then
  echo '{{"action":"continue","args":{{}},"why":"Not ready yet; looking again."}}'
else
{MERGE_WHEN_GREEN}
fi
"#
    ));
    let out = h.drive(
        "autodrive-merge-loop",
        &[
            "no_merge=false",
            "autodrive_budget_loop_rounds=1",
            "autodrive_budget_total_rounds=1",
            "autodrive_budget_wall_seconds=1",
            "autodrive_budget_made_up_key=1",
        ],
    );
    ok(&out);
    assert!(
        h.turns().len() >= 4,
        "the run went past the old limits: {:?}",
        h.turns()
    );
    assert_eq!(h.loop_end()["end"], "merged");
    let text = describe(&out).to_lowercase();
    assert!(
        text.contains("retired"),
        "one plain note that the keys are retired:\n{text}"
    );
}

// 9. Guidance sent mid-run, with the real `amplihack recipe guide`, governs
//    the next turn.
#[test]
fn guidance_sent_mid_run_skips_the_pending_action_and_governs_the_next_turn() {
    let h = Harness::new();
    h.policy(
        r#"
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
new=$(jq -r '[.guidance[]? | select(.new)] | length' "$TURN/reading.json")
if [ ! -e "$STUB_LOG/t1" ]; then
  : > "$STUB_LOG/t1"
  read -r _ < "$STUB_FIFO"
  printf '{"action":"merge","args":{"head":"%s"},"why":"All checks passed at this head."}\n' "$head"
elif [ "$new" != "0" ]; then
  echo '{"action":"handoff","args":{"owner":"operator","decision":"whether to merge after the hold","text":"The operator asked the run to hold. The run stopped as asked. Nothing was merged."},"why":"New guidance asks me to hold."}'
else
  echo '{"action":"continue","args":{},"why":"Nothing new."}'
fi
"#,
    );
    let mut live = fifo_release::start(
        h.drive_command("auto-drive-to-merge", &["no_merge=false"]),
        &h.fifo,
        "the drive",
    );
    h.guide("Hold. Do not merge until I say so.");
    let out = live.finish();
    assert!(
        h.merge_calls().is_empty(),
        "guidance arrived before the action: {:?}\n{}",
        h.calls(),
        describe(&out)
    );
    assert!(
        h.record("001").contains("skipped-guidance"),
        "{}",
        h.record("001")
    );
    let second = h.reading("002");
    let entries = second["guidance"].as_array().expect("guidance entries");
    assert!(entries.iter().any(|e| e["new"] == true), "{second}");
    assert!(
        !second.to_string().contains("Hold. Do not merge"),
        "guidance text is never in reading.json"
    );
    assert_eq!(h.loop_end()["end"], "handoff");
}
