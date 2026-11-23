//! #3129: the auto-drive goal's trust boundaries, through a real drive.
//!
//! These keep the strength of the boundaries the old finalize and merge
//! tests held (D20): merge authority from the captured invocation only, a
//! cross-repository PR refused, `--match-head-commit` with the head the turn
//! read, a base name that cannot reach git, the pinned bundle, exit 79 passed
//! up, and no secret in published text. Each runs the real binary and runner
//! against the stand-in gh and the stub agent of `support`, and reads only
//! what the run produced.

#![cfg(target_os = "linux")]

use super::support::{Harness, describe};
use std::fs;

fn merge_policy(head_expr: &str) -> String {
    format!(
        r#"
if [ ! -e "$STUB_LOG/tried" ]; then
  : > "$STUB_LOG/tried"
  head={head_expr}
  printf '{{"action":"merge","args":{{"head":"%s"}},"why":"Try the merge."}}\n' "$head"
else
  echo '{{"action":"handoff","args":{{"owner":"operator","decision":"what to do next","text":"The merge did not go through. The operator decides what happens next."}},"why":"Merge refused."}}'
fi
"#
    )
}

#[test]
fn a_cross_repository_pr_is_refused_before_any_turn() {
    let h = Harness::new();
    h.cross_repository();
    h.policy(&merge_policy(
        r#"$(jq -r '.goal.pr.head' "$TURN/reading.json")"#,
    ));
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert!(!out.status.success(), "{}", describe(&out));
    assert_eq!(h.loop_end()["end"], "refused");
    assert!(h.turns().is_empty(), "{:?}", h.turns());
    assert!(h.merge_calls().is_empty());
}

#[test]
fn merging_off_means_the_merge_action_is_not_authorised_even_when_the_agent_asks() {
    let h = Harness::new();
    h.policy(&merge_policy(
        r#"$(jq -r '.goal.pr.head' "$TURN/reading.json")"#,
    ));
    let out = h.drive("auto-drive-to-merge", &["no_merge=true"]);
    assert!(
        h.merge_calls().is_empty(),
        "{:?}\n{}",
        h.calls(),
        describe(&out)
    );
    assert_eq!(h.result("001")["outcome"], "not-authorised");
}

#[test]
fn a_dry_run_is_not_authorised_to_merge() {
    let h = Harness::new();
    h.policy(&merge_policy(
        r#"$(jq -r '.goal.pr.head' "$TURN/reading.json")"#,
    ));
    let _ = h.drive(
        "auto-drive-to-merge",
        &["no_merge=false", "autodrive_dry_run=true"],
    );
    assert!(h.merge_calls().is_empty(), "{:?}", h.calls());
}

#[test]
fn a_head_that_differs_from_this_turns_reading_is_refused_and_nothing_is_merged() {
    let h = Harness::new();
    h.policy(&merge_policy("0000000000000000000000000000000000000000"));
    let _ = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert!(h.merge_calls().is_empty(), "{:?}", h.calls());
    assert_ne!(h.result("001")["outcome"], "merged");
}

#[test]
fn a_head_that_is_not_hex_is_refused() {
    let h = Harness::new();
    h.policy(&merge_policy("'main; touch pwned'"));
    let _ = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert!(h.merge_calls().is_empty());
    assert!(!h.repo.join("pwned").exists());
}

#[test]
fn a_base_name_that_looks_like_an_option_or_climbs_never_reaches_git() {
    for bad in ["-evil", "../escape", "a..b"] {
        let h = Harness::new();
        h.base_name(bad);
        h.policy(&merge_policy(
            r#"$(jq -r '.goal.pr.head // ""' "$TURN/reading.json")"#,
        ));
        let _ = h.drive("auto-drive-to-merge", &["no_merge=false"]);
        let refs = super::support::git(&h.repo, &["for-each-ref", "--format=%(refname)"]);
        assert!(
            !refs.contains("evil") && !refs.contains("escape"),
            "{bad}: {refs}"
        );
        let base = &h.reading("001")["goal"]["base"];
        assert!(
            base["error"].as_str().is_some_and(|e| !e.is_empty()),
            "{bad}: the reading says why: {base}"
        );
    }
}

#[test]
fn exit_79_from_gh_ends_the_run_and_nothing_is_merged() {
    let h = Harness::new();
    h.set_checks("build\tpending\t0\thttps://example.invalid\n");
    h.checks_exit_79();
    h.policy(r#"echo '{"action":"wait","args":{"required":true},"why":"Waiting."}'"#);
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert!(!out.status.success(), "{}", describe(&out));
    assert!(h.merge_calls().is_empty());
    assert_eq!(
        h.turns().len(),
        1,
        "the run ended on 79, not on a later turn: {:?}",
        h.turns()
    );
}

#[test]
fn a_token_shaped_string_in_hand_off_text_is_refused_and_not_published_and_the_run_carries_on() {
    let h = Harness::new();
    let token = format!("ghp_{}", "A1b2C3d4E5".repeat(4));
    h.policy(&format!(
        r#"
if [ ! -e "$STUB_LOG/tried" ]; then
  : > "$STUB_LOG/tried"
  echo '{{"action":"handoff","args":{{"owner":"operator","decision":"x","text":"Use this token {token} to log in."}},"why":"x"}}'
else
  echo '{{"action":"handoff","args":{{"owner":"operator","decision":"what next","text":"The operator decides what happens next."}},"why":"x"}}'
fi
"#
    ));
    let _ = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert_eq!(h.result("001")["outcome"], "refused-secret");
    for c in h.comments() {
        assert!(!c.contains(&token), "the token was published: {c}");
    }
    assert_eq!(
        h.comments().len(),
        1,
        "only the clean second hand-off was posted"
    );
    assert_eq!(h.loop_end()["end"], "handoff");
}

#[test]
fn a_goal_tool_in_the_worktree_is_never_used_the_pinned_bundle_is() {
    let h = Harness::new();
    let planted = h.repo.join("amplifier-bundle/tools");
    fs::create_dir_all(&planted).unwrap();
    let marker = h.repo.join("pwned");
    crate::support::exec_stub::write_executable(
        &planted.join("autodrive_pr_goal.sh"),
        format!("#!/bin/sh\ntouch {}\nexit 0\n", marker.display()),
        0o755,
    );
    h.policy(super::support::MERGE_WHEN_GREEN);
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert!(
        !marker.exists(),
        "the worktree's tool ran:\n{}",
        describe(&out)
    );
    assert_eq!(h.loop_end()["end"], "merged");
}

#[test]
fn a_pending_required_check_is_never_merged_even_when_the_agent_asks() {
    let h = Harness::new();
    h.set_checks("build\tpending\t0\thttps://example.invalid\n");
    h.policy(&merge_policy(
        r#"$(jq -r '.goal.pr.head' "$TURN/reading.json")"#,
    ));
    let _ = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert!(h.merge_calls().is_empty(), "{:?}", h.calls());
    assert_eq!(h.result("001")["outcome"], "checks-not-green");
}
