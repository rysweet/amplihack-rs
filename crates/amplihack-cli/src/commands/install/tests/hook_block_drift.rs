//! Issue #2279 (C1): `hook_block::drift` classifies each event as `missing`,
//! `differing` or `extra`. `missing` and `differing` hold event names;
//! `extra` covers amplihack commands outside the spec (an XPIA wrapper, a
//! stale StopFailure). `drift` is pure and never prints file content.

use super::*;
use hook_block::HookTarget;
use serde_json::{Value, json};
use std::path::PathBuf;

const BIN: &str = "/opt/amplihack-test/bin/amplihack-hooks";

fn target(limit_resume: bool) -> HookTarget {
    HookTarget {
        bin: PathBuf::from(BIN),
        limit_resume,
    }
}

fn clean(limit_resume: bool) -> Value {
    let mut s = json!({});
    hook_block::merge(&mut s, &target(limit_resume));
    s
}

fn own_wrapper<'a>(s: &'a mut Value, event: &str, suffix: &str) -> &'a mut Value {
    s["hooks"][event]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|w| {
            w["hooks"][0]["command"]
                .as_str()
                .is_some_and(|c| c.ends_with(suffix))
        })
        .unwrap()
}

fn names(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

#[test]
fn clean_block_has_no_drift() {
    let d = hook_block::drift(&clean(true), &target(true));
    assert!(d.is_clean());
    assert!(d.missing.is_empty() && d.differing.is_empty() && d.extra.is_empty());
}

#[test]
fn empty_object_is_missing_every_event() {
    let d = hook_block::drift(&json!({}), &target(true));
    let mut expected: Vec<&str> = hook_block::specs(true).iter().map(|s| s.event).collect();
    expected.sort();
    expected.dedup();
    let mut got = names(&d.missing);
    got.sort();
    assert_eq!(got, expected);
    assert!(d.differing.is_empty());
    assert!(!d.is_clean());
}

#[test]
fn non_object_hooks_value_is_missing_every_event() {
    let d = hook_block::drift(&json!({"hooks": []}), &target(false));
    assert_eq!(
        d.missing.len(),
        8 - 1,
        "seven events without StopFailure: {d:?}"
    );
}

#[test]
fn one_removed_event_is_missing_only_that_event() {
    let mut s = clean(true);
    s["hooks"].as_object_mut().unwrap().remove("SessionEnd");
    let d = hook_block::drift(&s, &target(true));
    assert_eq!(names(&d.missing), vec!["SessionEnd"]);
    assert!(d.differing.is_empty());
    assert!(d.extra.is_empty());
}

#[test]
fn one_of_two_user_prompt_submit_wrappers_removed_is_missing() {
    let mut s = clean(true);
    s["hooks"]["UserPromptSubmit"]
        .as_array_mut()
        .unwrap()
        .retain(|w| {
            !w["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("workflow-classification-reminder")
        });
    let d = hook_block::drift(&s, &target(true));
    assert_eq!(names(&d.missing), vec!["UserPromptSubmit"]);
}

#[test]
fn a_different_bin_path_is_differing() {
    let mut s = json!({});
    hook_block::merge(
        &mut s,
        &HookTarget {
            bin: PathBuf::from("/opt/amplihack-old/bin/amplihack-hooks"),
            limit_resume: true,
        },
    );
    let d = hook_block::drift(&s, &target(true));
    assert!(d.missing.is_empty(), "{d:?}");
    assert_eq!(d.differing.len(), 8, "{d:?}");
}

#[test]
fn a_different_timeout_is_differing() {
    let mut s = clean(true);
    own_wrapper(&mut s, "Stop", " stop")["hooks"][0]["timeout"] = json!(5);
    let d = hook_block::drift(&s, &target(true));
    assert_eq!(names(&d.differing), vec!["Stop"]);
    assert!(d.missing.is_empty());
}

#[test]
fn a_missing_async_rewake_is_differing() {
    let mut s = clean(true);
    own_wrapper(&mut s, "StopFailure", "limit-resume")["hooks"][0]
        .as_object_mut()
        .unwrap()
        .remove("asyncRewake");
    let d = hook_block::drift(&s, &target(true));
    assert_eq!(names(&d.differing), vec!["StopFailure"]);
}

#[test]
fn a_different_matcher_is_differing() {
    let mut s = clean(true);
    own_wrapper(&mut s, "PreToolUse", "pre-tool-use")["matcher"] = json!("Bash");
    let d = hook_block::drift(&s, &target(true));
    assert_eq!(names(&d.differing), vec!["PreToolUse"]);
    assert!(d.missing.is_empty(), "{d:?}");
}

#[test]
fn a_duplicate_own_wrapper_is_differing() {
    let mut s = clean(true);
    let dup = own_wrapper(&mut s, "Stop", " stop").clone();
    s["hooks"]["Stop"].as_array_mut().unwrap().push(dup);
    let d = hook_block::drift(&s, &target(true));
    assert_eq!(names(&d.differing), vec!["Stop"]);
}

#[test]
fn an_xpia_wrapper_is_extra() {
    let mut s = clean(true);
    s["hooks"]["PreToolUse"].as_array_mut().unwrap().push(json!({
        "matcher": "*",
        "hooks": [{"type": "command", "command": "/opt/x/.claude/tools/xpia/hooks/pre_tool_use.py"}]
    }));
    let d = hook_block::drift(&s, &target(true));
    assert_eq!(names(&d.extra), vec!["PreToolUse"]);
    assert!(d.missing.is_empty() && d.differing.is_empty());
}

#[test]
fn a_stale_stop_failure_without_limit_resume_is_extra() {
    let s = clean(true);
    let d = hook_block::drift(&s, &target(false));
    assert_eq!(names(&d.extra), vec!["StopFailure"]);
    assert!(d.missing.is_empty() && d.differing.is_empty());
}

#[test]
fn missing_and_extra_on_one_event() {
    let mut s = clean(true);
    s["hooks"]["PreToolUse"] = json!([{
        "matcher": "*",
        "hooks": [{"type": "command", "command": "/opt/x/tools/xpia/hooks/pre_tool_use.py"}]
    }]);
    let d = hook_block::drift(&s, &target(true));
    assert_eq!(names(&d.missing), vec!["PreToolUse"]);
    assert_eq!(names(&d.extra), vec!["PreToolUse"]);
}

#[test]
fn foreign_wrappers_are_not_drift() {
    let mut s = clean(true);
    s["hooks"]["Stop"].as_array_mut().unwrap().push(json!({
        "hooks": [{"type": "command", "command": "/opt/foreign/notify"}]
    }));
    s["hooks"]["Notification"] =
        json!([{"hooks": [{"type": "command", "command": "/opt/foreign/n"}]}]);
    assert!(hook_block::drift(&s, &target(true)).is_clean());
}

#[test]
fn lines_name_events_and_never_file_content() {
    let mut s = clean(true);
    s["env"] = json!({"SENTINEL_SECRET": "sk-test-do-not-print"});
    s["hooks"].as_object_mut().unwrap().remove("PreCompact");
    own_wrapper(&mut s, "Stop", " stop")["hooks"][0]["timeout"] = json!(5);
    let lines = hook_block::drift(&s, &target(true)).lines();
    let joined = lines.join("\n");
    assert!(joined.contains("PreCompact"), "{joined}");
    assert!(joined.contains("Stop"), "{joined}");
    assert!(!joined.contains('{'), "no JSON in drift lines: {joined}");
    assert!(!joined.contains("sk-test-do-not-print"));
}
