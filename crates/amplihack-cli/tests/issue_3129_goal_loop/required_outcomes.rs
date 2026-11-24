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
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

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

/// The turn tool's `outcomes` step on one turn folder, with a seeded ledger.
fn outcomes_step(h: &Harness, ledger: &[serde_json::Value], new: &[serde_json::Value]) -> Output {
    let turn = h.state.join("turns").join("001");
    for d in [h.state.join("turns"), turn.clone()] {
        fs::create_dir_all(&d).unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let ledger = json!({"schema_version": 1, "outcomes": ledger});
    fs::write(h.state.join("outcomes.json"), ledger.to_string()).unwrap();
    let decision = json!({"schema_version": 1, "valid": true, "action": "continue", "args": {},
        "why": "", "problem": null, "outcomes": new});
    fs::write(turn.join("decision.json"), decision.to_string()).unwrap();
    let bundle = h.bundle_root.join("amplifier-bundle");
    Command::new("bash")
        .arg(bundle.join("tools/autodrive_goal_turn.sh"))
        .args(["outcomes"])
        .arg(&turn)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", h.bin.display()))
        .env("HOME", &h.home)
        .env("AUTODRIVE_STATE_DIR", &h.state)
        .output()
        .unwrap()
}

fn entry(name: &str, evidence: &str) -> serde_json::Value {
    json!({"name": name, "status": "met", "evidence": evidence, "reason": "", "turn": 1,
        "answered_reason": "", "answered": false})
}

/// Nothing truncates the ledger, so no named outcome drops out of the merge
/// check, and its one bound, the read cap, is reported when a turn passes it.
/// Fails on 47b21de0, whose ledger kept only its first 256 names and said
/// nothing.
#[test]
fn the_outcome_ledger_keeps_every_name_and_says_so_when_it_passes_its_read_cap() {
    let h = Harness::new();
    let seeded: Vec<_> = (1..=256)
        .map(|i| entry(&format!("outcome {i}"), ""))
        .collect();
    let newest =
        json!({"name": "the newest outcome", "status": "unmet", "evidence": "", "reason": ""});
    let out = outcomes_step(&h, &seeded, &[newest]);
    assert!(out.status.success(), "{}", describe(&out));
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("outcomes.json")).unwrap()).unwrap();
    let names: Vec<&str> = ledger["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["name"].as_str())
        .collect();
    assert_eq!(names.len(), 257, "{names:?}");
    assert!(names.contains(&"the newest outcome"), "{names:?}");

    // Just under the 512 KiB read cap, then one turn's outcomes take it past.
    let long = "x".repeat(2000);
    let seeded: Vec<_> = (1..=230)
        .map(|i| entry(&format!("outcome {i}"), &long))
        .collect();
    let more: Vec<_> = (1..=25)
        .map(|i| json!({"name": format!("later outcome {i}"), "status": "met", "evidence": long, "reason": ""}))
        .collect();
    let out = outcomes_step(&h, &seeded, &more);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("255 names") && stderr.contains("read cap"),
        "{stderr}"
    );
}
