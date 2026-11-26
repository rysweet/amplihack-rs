//! #3129, the PR 1 case of issue comment 7114891207: when the task keeps the
//! PR title and description exactly as they are, the readiness evidence goes
//! in a PR comment, that comment counts, and the run merges.
//!
//! On 1.2.211 a run that met every criterion ended as Failed, handed off and
//! returned the PR to draft, because a parsed gate wanted the evidence below a
//! report marker in the description. The goal loop has no such gate. This
//! test pins both halves: the rendered turn prompt tells the agent where the
//! evidence goes, and a run whose PR body carries no evidence, whose task
//! keeps the description and whose agent posts the evidence in a comment
//! never edits the PR and merges on that comment.
//!
//! The real binary, runner, recipes and tools run against the stand-in gh and
//! the stub agent of `support`. The test reads only what the run produced.

#![cfg(target_os = "linux")]

use super::support::{Harness, describe};
use serde_json::json;

const TASK: &str = "task_description=Get this pull request merged. \
Keep the PR title and description exactly as they are.";
const KEPT: &str = "the title and description stay exactly as they are";
const EVIDENCE: &str = "the readiness evidence is recorded";
const MARK: &str = "Readiness evidence for this head";

#[test]
fn a_kept_description_sends_the_evidence_to_a_comment_and_the_run_merges() {
    let h = Harness::new();
    h.policy(&format!(
        r#"
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
outcomes='[{{"name":"{KEPT}","status":"met","evidence":"The run did not edit the title or the description."}},{{"name":"{EVIDENCE}","status":"met","evidence":"The evidence is in a PR comment, because the task keeps the description."}}]'
if [ ! -e "$STUB_LOG/evidence-posted" ]; then
  printf '{MARK} %s: every check passed, the merge state is CLEAN.\n' "$head" > "$STUB_LOG/evidence.md"
  gh pr comment 7 --body-file "$STUB_LOG/evidence.md" > /dev/null || exit 1
  : > "$STUB_LOG/evidence-posted"
  printf '{{"action":"continue","args":{{}},"outcomes":%s,"why":"The task keeps the description, so I posted the readiness evidence in a PR comment."}}\n' "$outcomes"
else
  printf '{{"action":"merge","args":{{"head":"%s"}},"outcomes":%s,"why":"The evidence is in a PR comment and every condition holds."}}\n' "$head" "$outcomes"
fi
"#
    ));
    let out = h.drive("auto-drive-to-merge", &["no_merge=false", TASK]);

    // The rendered prompt says where the evidence goes when the description is kept.
    let prompt = h.turn_prompt("001");
    for rule in [
        "keeps the PR title or description exactly as they are",
        "post the evidence in a PR comment",
        "never a reason to stop, hand off or return the PR to draft",
    ] {
        assert!(prompt.contains(rule), "the turn prompt lacks {rule:?}");
    }

    // The evidence went to a comment, before the merge.
    let calls = h.calls();
    let evidence = h.comments().iter().position(|c| c.starts_with(MARK));
    assert!(
        evidence.is_some(),
        "comments: {:?}\n{}",
        h.comments(),
        describe(&out)
    );
    let comment = calls
        .iter()
        .position(|c| c.starts_with("pr comment"))
        .expect("a comment");
    let merge = calls
        .iter()
        .position(|c| c.starts_with("pr merge"))
        .unwrap_or_else(|| panic!("no merge: {calls:?}\n{}", describe(&out)));
    assert!(comment < merge, "{calls:?}");

    // Nothing edited the title or description, or returned the PR to draft.
    for c in &calls {
        assert!(!c.starts_with("pr edit"), "the run edited the PR: {c}");
        assert!(
            !c.contains("--undo"),
            "the run returned the PR to draft: {c}"
        );
    }

    // Both outcomes are on record as met, and the run merged on them.
    let listed = h.reading("002")["outcomes"].clone();
    for name in [KEPT, EVIDENCE] {
        let entry = listed
            .as_array()
            .and_then(|a| a.iter().find(|o| o["name"] == json!(name)))
            .unwrap_or_else(|| panic!("{name} not in {listed}"));
        assert_eq!(entry["status"], json!("met"), "{entry}");
    }
    assert_eq!(h.merge_calls().len(), 1, "{calls:?}");
    assert_eq!(
        h.result("002")["outcome"],
        json!("merged"),
        "{}",
        describe(&out)
    );

    // The stand-in PR's description ("Fixture body") holds no evidence, and
    // the merge went ahead on it as it was: the kept title and description
    // became the commit message unchanged, and the evidence stayed in the
    // comment.
    let (subject, body) = h.merge_message();
    assert_eq!(subject, "Fixture (#7)");
    assert_eq!(body, "Fixture body");
    assert_eq!(h.loop_end()["end"], json!("merged"));
    assert!(out.status.success(), "{}", describe(&out));
}
