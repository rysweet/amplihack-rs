//! #3129 R2: required outcomes are listed in the reading and checked by name
//! before a merge, and one the agent rejects is answered in a PR comment.
//!
//! The real binary, runner, recipes and tools run against the stand-in gh and
//! the stub agent of `support`; the test reads only what the run produced. It
//! fails on ccb445a9, where the second turn's merge went ahead with an outcome
//! named earlier and never checked.

#![cfg(target_os = "linux")]

use super::support::{Harness, describe};
use serde_json::json;

const NAME: &str = "the helper keeps its old name";
const REASON: &str = "The operator later chose the new name, which this pull request uses.";

#[test]
fn a_merge_is_refused_until_every_named_outcome_is_met_or_answered_and_a_rejection_is_posted() {
    let h = Harness::new();
    h.policy(&format!(
        r#"
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
if [ ! -e "$STUB_LOG/t1" ]; then
  : > "$STUB_LOG/t1"
  echo '{{"action":"continue","args":{{}},"outcomes":[{{"name":"{NAME}","status":"unmet","evidence":"not done yet"}}],"why":"The task requires the helper to keep its name. It does not yet."}}'
elif [ ! -e "$STUB_LOG/t2" ]; then
  : > "$STUB_LOG/t2"
  printf '{{"action":"merge","args":{{"head":"%s"}},"outcomes":[],"why":"A merge that forgets the outcome."}}\n' "$head"
else
  printf '{{"action":"merge","args":{{"head":"%s"}},"outcomes":[{{"name":"{NAME}","status":"rejected","reason":"{REASON}"}}],"why":"The outcome no longer applies."}}\n' "$head"
fi
"#
    ));
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);

    // The ledger reaches the next reading, by name.
    let listed = h.reading("002")["outcomes"].clone();
    assert_eq!(
        listed[0]["name"],
        json!(NAME),
        "{listed}\n{}",
        describe(&out)
    );
    assert_eq!(listed[0]["status"], json!("unmet"), "{listed}");

    // A merge that does not account for it is refused, naming it.
    let second = h.result("002");
    assert_eq!(second["outcome"], json!("outcomes-unmet"), "{second}");
    assert!(
        second["summary"]
            .as_str()
            .unwrap_or_default()
            .contains(NAME),
        "{second}"
    );

    // The rejection is answered in a PR comment before the merge goes ahead.
    let answered = h
        .comments()
        .iter()
        .any(|c| c.contains(NAME) && c.contains(REASON));
    assert!(answered, "comments: {:?}", h.comments());
    let calls = h.calls();
    let comment = calls
        .iter()
        .position(|c| c.starts_with("pr comment"))
        .expect("a comment");
    let merge = calls
        .iter()
        .position(|c| c.starts_with("pr merge"))
        .expect("a merge");
    assert!(comment < merge, "{calls:?}");
    assert_eq!(h.merge_calls().len(), 1, "{calls:?}");
    assert_eq!(h.result("003")["outcome"], json!("merged"));
    assert_eq!(h.loop_end()["end"], json!("merged"));
}
