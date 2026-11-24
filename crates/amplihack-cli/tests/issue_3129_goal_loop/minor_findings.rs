//! #3160: `autodrive_minor_findings` (fix_all by default, judge, file_all).
//!
//! Each test runs the real binary, runner, recipes and tools against the
//! stand-in gh and a stub agent whose policy imitates what rule 6 tells an
//! agent to do. The stand-in review is a planted `crusty-round-001.json`
//! (`MINOR_ONLY`) with one finding in its `.minor` file. The tests show that
//! the loop carries the setting and each finding's outcome and that a merge
//! waits for it. They do not show that a real model follows rule 6.
//!
//! Assertions read only outcomes, the reading's JSON, gh calls, issue
//! records and git refs. Nothing reads Markdown wording, and nothing sleeps.

#![cfg(target_os = "linux")]

use super::support::{Harness, MERGE_WHEN_GREEN, PR_BRANCH, describe};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;

const FINDING: &str = "m1";
const SUMMARY: &str = "The helper's error message names no file.";

fn planted(h: &Harness) {
    h.plant_minor_round(1, &h.initial_head, &[(FINDING, SUMMARY)]);
}

fn first_minor(reading: &Value) -> Value {
    reading["goal"]["reviews"]["crusty"][0]["minor_findings"][0].clone()
}

fn setting(reading: &Value) -> Value {
    reading["goal"]["autodrive_minor_findings"].clone()
}

/// The merge call's `--match-head-commit` argument.
fn merged_head(h: &Harness) -> String {
    let call = &h.merge_calls()[0];
    let words: Vec<&str> = call.split_whitespace().collect();
    let i = words
        .iter()
        .position(|w| *w == "--match-head-commit")
        .unwrap();
    words[i + 1].to_owned()
}

fn file_issue(reason_line: &str) -> String {
    format!(
        r#"
cat > "$STUB_LOG/issue-body.md" <<'BODY'
Follow-up from pull request #7.

```
{SUMMARY}
```
{reason_line}
BODY
url=$(gh issue create --repo o/r --label follow-up \
  --title "Make the helper's error message name the file" \
  --body-file "$STUB_LOG/issue-body.md")
"#
    )
}

#[test]
fn fix_all_fixes_the_minor_finding_before_the_merge() {
    let h = Harness::new();
    planted(&h);
    h.policy(&format!(
        r#"
n=$(basename "$TURN")
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
case "$n" in
  001) echo '{{"action":"continue","args":{{}},"outcomes":[{{"name":"minor:{FINDING}","status":"unmet","evidence":"","reason":""}}],"why":"Every finding is fixed in the PR before the merge."}}' ;;
  002) printf '{{"action":"merge","args":{{"head":"%s"}},"outcomes":[],"why":"Try the merge."}}\n' "$head" ;;
  003)
    echo "a fix" >> widget.txt
    git commit -qam "Name the file in the helper's error message"
    git push -q origin "HEAD:{PR_BRANCH}"
    sha=$(git rev-parse HEAD)
    printf '{{"action":"continue","args":{{}},"outcomes":[{{"name":"minor:{FINDING}","status":"met","treatment":"fixed","evidence":"%s","reason":""}}],"why":"Fixed in the PR."}}\n' "$sha" ;;
  *) sha=$(jq -r '.outcomes[0].evidence' "$TURN/reading.json")
    printf '{{"action":"merge","args":{{"head":"%s"}},"outcomes":[{{"name":"minor:{FINDING}","status":"met","treatment":"fixed","evidence":"%s","reason":""}}],"why":"The finding is fixed."}}\n' "$head" "$sha" ;;
esac
"#
    ));
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);

    assert_eq!(setting(&h.reading("001")), json!("fix_all"));
    assert_eq!(first_minor(&h.reading("001"))["outcome"], json!("open"));
    // The merge is refused while the finding is unmet, and no merge call is made.
    assert_eq!(h.result("002")["outcome"], json!("outcomes-unmet"));
    // The fix is on origin, nothing is filed, and the merge was of the fixed head.
    let head = h.origin_head(PR_BRANCH);
    assert_ne!(head, h.initial_head, "{}", describe(&out));
    assert!(h.is_ancestor(&h.initial_head, &head));
    assert!(h.issues().is_empty(), "{:?}", h.issues());
    assert_eq!(h.merge_calls().len(), 1, "{:?}", h.calls());
    assert_eq!(merged_head(&h), head);
    let last = h.turns().pop().unwrap();
    let minor = first_minor(&h.reading(&last));
    assert_eq!(minor["outcome"], json!("fixed"), "{minor}");
    assert_eq!(minor["id"], json!(FINDING));
    assert_eq!(h.loop_end()["end"], json!("merged"));
}

#[test]
fn judge_files_an_unfixed_minor_finding_with_a_reason_before_the_merge() {
    let h = Harness::new();
    planted(&h);
    let filing = file_issue("Not fixed here: the message is shared with another tool.");
    h.policy(&format!(
        r#"
n=$(basename "$TURN")
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
case "$n" in
  001) echo '{{"action":"continue","args":{{}},"outcomes":[{{"name":"minor:{FINDING}","status":"unmet","evidence":"","reason":""}}],"why":"I judge each minor finding."}}' ;;
  002)
{filing}
    printf '{{"action":"continue","args":{{}},"outcomes":[{{"name":"minor:{FINDING}","status":"met","treatment":"filed","evidence":"%s","reason":"The message is shared with another tool."}}],"why":"Filed as a follow-up."}}\n' "$url" ;;
  *) url=$(jq -r '.outcomes[0].evidence' "$TURN/reading.json")
    printf '{{"action":"merge","args":{{"head":"%s"}},"outcomes":[{{"name":"minor:{FINDING}","status":"met","treatment":"filed","evidence":"%s","reason":"The message is shared with another tool."}}],"why":"The finding is filed."}}\n' "$head" "$url" ;;
esac
"#
    ));
    let out = h.drive(
        "auto-drive-to-merge",
        &["no_merge=false", "autodrive_minor_findings=judge"],
    );

    assert_eq!(setting(&h.reading("001")), json!("judge"));
    let issues = h.issues();
    assert_eq!(issues.len(), 1, "{issues:?}\n{}", describe(&out));
    assert!(
        issues[0]["labels"]
            .as_array()
            .unwrap()
            .contains(&json!("follow-up"))
    );
    assert!(
        issues[0]["body"].as_str().unwrap().contains("#7"),
        "{issues:?}"
    );
    assert!(!issues[0]["title"].as_str().unwrap().trim().is_empty());
    // Filed before the merge.
    let calls = h.calls();
    let filed = calls
        .iter()
        .position(|c| c.starts_with("issue create"))
        .unwrap();
    let merge = calls
        .iter()
        .position(|c| c.starts_with("pr merge"))
        .unwrap();
    assert!(filed < merge, "{calls:?}");
    let last = h.turns().pop().unwrap();
    let minor = first_minor(&h.reading(&last));
    assert_eq!(minor["outcome"], json!("filed"), "{minor}");
    assert!(
        !minor["reason"].as_str().unwrap_or_default().is_empty(),
        "{minor}"
    );
    assert!(
        minor["evidence"].as_str().unwrap().contains("/issues/"),
        "{minor}"
    );
    assert_eq!(h.loop_end()["end"], json!("merged"));
}

#[test]
fn file_all_files_the_minor_finding_and_fixes_nothing() {
    let h = Harness::new();
    planted(&h);
    let filing = file_issue("");
    h.policy(&format!(
        r#"
n=$(basename "$TURN")
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
case "$n" in
  001)
{filing}
    printf '{{"action":"continue","args":{{}},"outcomes":[{{"name":"minor:{FINDING}","status":"met","treatment":"filed","evidence":"%s","reason":""}}],"why":"Filed, not fixed."}}\n' "$url" ;;
  *) url=$(jq -r '.outcomes[0].evidence' "$TURN/reading.json")
    printf '{{"action":"merge","args":{{"head":"%s"}},"outcomes":[{{"name":"minor:{FINDING}","status":"met","treatment":"filed","evidence":"%s","reason":""}}],"why":"The finding is filed."}}\n' "$head" "$url" ;;
esac
"#
    ));
    let out = h.drive(
        "auto-drive-to-merge",
        &["no_merge=false", "autodrive_minor_findings=file_all"],
    );

    assert_eq!(setting(&h.reading("001")), json!("file_all"));
    let issues = h.issues();
    assert_eq!(issues.len(), 1, "{issues:?}\n{}", describe(&out));
    assert!(
        issues[0]["labels"]
            .as_array()
            .unwrap()
            .contains(&json!("follow-up"))
    );
    assert_eq!(h.origin_head(PR_BRANCH), h.initial_head, "no fix commit");
    let last = h.turns().pop().unwrap();
    assert_eq!(first_minor(&h.reading(&last))["outcome"], json!("filed"));
    assert_eq!(h.loop_end()["end"], json!("merged"));
}

#[test]
fn an_absent_setting_reads_as_fix_all() {
    let h = Harness::new();
    planted(&h);
    h.policy(MERGE_WHEN_GREEN);
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert!(out.status.success(), "{}", describe(&out));
    let r = h.reading("001");
    assert_eq!(setting(&r), json!("fix_all"));
    assert_eq!(r["goal"]["autodrive_minor_findings_error"], Value::Null);
}

#[test]
fn a_state_folder_from_an_older_run_resumes_as_fix_all() {
    let h = Harness::new();
    // A ledger and a round record as the previous release wrote them: no
    // minor:* names, no treatment field, no setting anywhere.
    let ledger = json!({"schema_version": 1, "outcomes": [
        {"name": "the helper keeps its name", "status": "met", "evidence": "abc", "reason": "",
         "turn": 1, "answered_reason": "", "answered": false}]});
    fs::write(h.state.join("outcomes.json"), ledger.to_string()).unwrap();
    fs::set_permissions(
        h.state.join("outcomes.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    planted(&h);
    h.policy(
        r#"
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
printf '{"action":"merge","args":{"head":"%s"},"outcomes":[{"name":"the helper keeps its name","status":"met","evidence":"abc","reason":""}],"why":"Resumed."}\n' "$head"
"#,
    );
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert!(out.status.success(), "{}", describe(&out));
    let r = h.reading("001");
    assert_eq!(setting(&r), json!("fix_all"));
    assert_eq!(first_minor(&r)["outcome"], json!("open"));
    assert_eq!(h.loop_end()["end"], json!("merged"));
}

#[test]
fn an_unknown_value_is_refused_at_the_start() {
    for recipe in ["auto-drive-to-merge", "auto-drive-light"] {
        for value in ["bogus", "FIX_ALL", "bad\u{1b}[31mvalue"] {
            let h = Harness::new();
            planted(&h);
            h.policy(MERGE_WHEN_GREEN);
            let arg = format!("autodrive_minor_findings={value}");
            let out = h.drive(recipe, &["no_merge=false", &arg]);
            let text = describe(&out);
            assert!(!out.status.success(), "{recipe} {value:?}: {text}");
            for word in ["fix_all", "judge", "file_all"] {
                assert!(
                    text.contains(word),
                    "{recipe} {value:?} lacks {word}: {text}"
                );
            }
            // The echoed value is cleaned. The runner's own coloured notices
            // carry escape sequences too, so only the refusal lines are read.
            let refusals: Vec<&str> = text
                .lines()
                .filter(|l| l.contains("autodrive_minor_findings must be"))
                .collect();
            assert!(!refusals.is_empty(), "{recipe} {value:?}: {text}");
            for line in refusals {
                let at = line.find("autodrive_minor_findings must be").unwrap();
                let tail = line[at..].split("(got '").nth(1).unwrap_or_default();
                let echoed = tail.split("')").next().unwrap_or_default();
                assert!(
                    !echoed.contains('\u{1b}') && !echoed.contains("u001b"),
                    "{recipe} {value:?}: {line}"
                );
            }
            assert!(h.turns().is_empty(), "{recipe}: {:?}", h.turns());
            assert!(h.merge_calls().is_empty(), "{:?}", h.calls());
            assert!(
                !h.calls().iter().any(|c| c.starts_with("pr ")),
                "{recipe}: a PR call was made: {:?}",
                h.calls()
            );
            assert!(
                !h.base.join("agent-turns.log").exists(),
                "{recipe}: an agent ran"
            );
        }
    }
}

#[test]
fn an_unknown_value_given_to_the_goal_loop_directly_is_reported_not_refused() {
    let h = Harness::new();
    planted(&h);
    h.policy(MERGE_WHEN_GREEN);
    let out = h.drive(
        "autodrive-goal-loop",
        &[
            "goal_tool=autodrive_pr_goal.sh",
            "goal_file=skills/auto-drive-to-merge/goal-merge-pr.md",
            &format!("repo_path={}", h.repo.display()),
            "no_merge=false",
            "autodrive_minor_findings=bogus",
        ],
    );
    let r = h.reading("001");
    assert_eq!(setting(&r), json!("fix_all"), "{}", describe(&out));
    let err = r["goal"]["autodrive_minor_findings_error"]
        .as_str()
        .unwrap_or_default();
    for word in ["bogus", "fix_all", "judge", "file_all"] {
        assert!(err.contains(word), "{err}");
    }
}

#[test]
fn a_merge_decision_may_restate_more_than_64_outcomes() {
    let h = Harness::new();
    let ledger: Vec<Value> = (1..=70)
        .map(|i| {
            json!({"name": format!("minor:m{i}"), "status": "met", "treatment": "filed",
            "evidence": format!("https://github.com/o/r/issues/{i}"), "reason": "",
            "turn": 1, "answered_reason": "", "answered": false})
        })
        .collect();
    fs::write(
        h.state.join("outcomes.json"),
        json!({"schema_version": 1, "outcomes": ledger}).to_string(),
    )
    .unwrap();
    fs::set_permissions(
        h.state.join("outcomes.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    h.policy(
        r#"
jq -c --arg h "$(jq -r '.goal.pr.head' "$TURN/reading.json")" \
  '{action:"merge",args:{head:$h},outcomes:[.outcomes[] | {name,status,evidence,reason}],why:"Every named outcome is met."}' "$TURN/reading.json"
"#,
    );
    let out = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    assert_eq!(h.merge_calls().len(), 1, "{}", describe(&out));
    assert_eq!(h.loop_end()["end"], json!("merged"));
}

#[test]
fn a_malformed_finding_or_ledger_does_not_break_the_reading() {
    let h = Harness::new();
    let record = json!({"crusty_verdict": "MINOR_ONLY", "concern_count": 0,
        "head_sha": h.initial_head, "reviewed_head_sha": h.initial_head});
    let long = "e".repeat(500);
    let minor = format!(
        "{}\n{}\n",
        json!({"id": {"not": "a string"}, "summary": "object id"}),
        json!({"id": "m2", "summary": "long evidence"})
    );
    for (f, body) in [
        ("crusty-round-001.json", format!("{record}\n")),
        ("crusty-round-001.json.minor", minor),
        (
            "outcomes.json",
            json!({"schema_version": 1, "outcomes": [
                {"name": 7, "status": "met", "evidence": "", "reason": ""},
                {"name": "minor:m2", "status": "met", "treatment": "filed", "evidence": long, "reason": ""}
            ]})
            .to_string(),
        ),
    ] {
        let p = h.state.join(f);
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    }
    h.policy(r#"echo '{"action":"handoff","args":{"owner":"operator","decision":"what next","text":"One reading is enough."},"why":"Read once."}'"#);
    let _ = h.drive("auto-drive-to-merge", &["no_merge=false"]);
    let r = h.reading("001");
    let findings = r["goal"]["reviews"]["crusty"][0]["minor_findings"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert_eq!(findings[0]["outcome"], json!("open"));
    let shown = findings[1]["evidence"].as_str().unwrap();
    assert!(shown.len() <= 300, "evidence is truncated: {}", shown.len());
    assert_eq!(findings[1]["outcome"], json!("filed"));
}
