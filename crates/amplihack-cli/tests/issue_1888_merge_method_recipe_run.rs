//! #1888, through `amplihack recipe run auto-drive-to-merge` and the real
//! recipe runner: an authorised merge uses the merge method the run was
//! given, or one the repository allows, and a method that cannot be settled
//! makes no merge request.
//!
//! #3129 re-pointed these cases from the retired finalize step to the goal
//! loop's merge action (`autodrive_pr_goal.sh act merge`, which runs the
//! trusted `autodrive_merge.sh`). What is real: the CLI, the runner, the
//! runner's authorisation capture and `report()`, the drive's recipes and
//! tools from a byte copy of the bundle. What is a stand-in: `gh` (an open PR
//! with green checks, the repository's allowed methods, and a merge that
//! enforces them and records its argv) and the agent, which asks for a merge
//! at the head it read and hands off when the merge is refused. Assertions
//! are on the recorded `gh pr merge` argv, `merge-result.json` and the run's
//! report; no recipe text is read.

#![cfg(target_os = "linux")]

use crate::issue_3129_goal_loop::support::{Harness, describe};
use serde_json::json;

/// Merge at the head this turn read; after a refused merge, hand off.
const POLICY: &str = r#"
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
prev=$(jq -r '.previous.outcome // ""' "$TURN/reading.json")
if [ -n "$prev" ] && [ "$prev" != "null" ]; then
  echo '{"action":"handoff","args":{"owner":"operator","decision":"which merge method to use","text":"The merge was refused. The operator decides which merge method to use."},"why":"The merge was refused."}'
else
  printf '{"action":"merge","args":{"head":"%s"},"why":"Every check passed at this head."}\n' "$head"
fi
"#;

struct Run {
    h: Harness,
    out: std::process::Output,
}

fn drive(merge: bool, squash: bool, rebase: bool, method: Option<&str>) -> Run {
    let h = Harness::new();
    h.allow_methods(merge, squash, rebase);
    h.policy(POLICY);
    let given = method.map(|m| format!("merge_method={m}"));
    let mut extra = vec!["no_merge=false"];
    if let Some(g) = &given {
        extra.push(g);
    }
    let out = h.drive("auto-drive-to-merge", &extra);
    Run { h, out }
}

impl Run {
    fn merge_argv(&self) -> Option<Vec<String>> {
        self.h
            .merge_calls()
            .first()
            .map(|c| c.split_whitespace().map(str::to_owned).collect())
    }
    fn show(&self) -> String {
        format!("{}\ncalls: {:?}", describe(&self.out), self.h.calls())
    }
}

fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let i = argv.iter().position(|a| a == flag)?;
    argv.get(i + 1).map(String::as_str)
}

#[test]
fn a_run_given_merge_merges_with_a_merge_commit_at_the_assessed_head() {
    let r = drive(true, true, true, Some("merge"));
    assert_eq!(r.out.status.code(), Some(0), "{}", r.show());
    let argv = r
        .merge_argv()
        .unwrap_or_else(|| panic!("no merge: {}", r.show()));
    assert!(argv.iter().any(|a| a == "--merge"), "{argv:?}");
    for absent in ["--squash", "--rebase", "--admin", "--auto"] {
        assert!(!argv.iter().any(|a| a == absent), "{absent}: {argv:?}");
    }
    assert_eq!(
        flag_value(&argv, "--match-head-commit"),
        Some(r.h.initial_head.as_str())
    );
    let rec = r.h.merge_result();
    assert_eq!(rec["result"], json!("merged"), "{rec}");
    assert_eq!(rec["merge_method"], json!("merge"), "{rec}");
    assert!(
        rec["merge_method_reason"]
            .as_str()
            .unwrap_or("")
            .contains("given"),
        "{rec}"
    );
    assert_eq!(
        Harness::outcome(&r.out)["stop_reason"],
        json!("completed"),
        "{}",
        r.show()
    );
}

#[test]
fn a_repository_that_allows_only_merge_commits_is_merged_with_one() {
    let r = drive(true, false, false, None);
    assert_eq!(r.out.status.code(), Some(0), "{}", r.show());
    let argv = r
        .merge_argv()
        .unwrap_or_else(|| panic!("no merge: {}", r.show()));
    assert!(argv.iter().any(|a| a == "--merge"), "{argv:?}");
    assert!(!argv.iter().any(|a| a == "--squash"), "{argv:?}");
    assert_eq!(
        flag_value(&argv, "--match-head-commit"),
        Some(r.h.initial_head.as_str())
    );
    let rec = r.h.merge_result();
    assert_eq!(rec["merge_method"], json!("merge"), "{rec}");
    assert!(
        rec["merge_method_reason"]
            .as_str()
            .unwrap_or("")
            .contains("squash false"),
        "the reason names what the repository allows: {rec}"
    );
}

#[test]
fn a_run_without_a_method_still_squashes_where_squash_is_allowed() {
    let r = drive(true, true, false, None);
    assert_eq!(r.out.status.code(), Some(0), "{}", r.show());
    let argv = r
        .merge_argv()
        .unwrap_or_else(|| panic!("no merge: {}", r.show()));
    assert!(argv.iter().any(|a| a == "--squash"), "{argv:?}");
    assert!(!argv.iter().any(|a| a == "--merge"), "{argv:?}");
    assert_eq!(r.h.merge_result()["merge_method"], json!("squash"));
}

#[test]
fn a_method_the_repository_disallows_makes_no_merge_request_and_the_agent_hands_off() {
    let r = drive(true, false, false, Some("squash"));
    assert!(r.merge_argv().is_none(), "no merge request: {}", r.show());
    let rec = r.h.merge_result();
    assert_eq!(rec["result"], json!("refused"), "{rec}");
    let why = rec["reason"].as_str().unwrap_or("");
    assert!(why.contains("'squash'"), "names the given method: {rec}");
    assert!(why.contains("squash false"), "names what is allowed: {rec}");
    assert_eq!(
        r.h.result("001")["outcome"],
        json!("refused"),
        "the next turn reads the refusal"
    );
    assert_eq!(
        Harness::outcome(&r.out)["stop_reason"],
        json!("handoff"),
        "{}",
        r.show()
    );
}
