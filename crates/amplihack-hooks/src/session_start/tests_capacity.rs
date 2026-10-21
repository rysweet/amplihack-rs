//! SessionStart and the no-capacity notice: a fresh session shows it and
//! records it, so the first prompt stays quiet; existing context is kept.

use super::*;
use crate::capacity_notice::tests::{Capacity, HOUR, MODEL_HINT};
use crate::test_support::env_lock;
use crate::user_prompt::UserPromptSubmitHook;
use amplihack_utils::claude_capacity::State;

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock().lock().unwrap_or_else(|p| p.into_inner())
}

fn start(session: &str) -> Value {
    SessionStartHook
        .process(HookInput::SessionStart {
            session_id: Some(session.to_string()),
            cwd: None,
            extra: Value::Null,
        })
        .expect("session start never fails")
}

fn prompt(session: &str) -> Value {
    UserPromptSubmitHook
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some("first prompt".to_string()),
            session_id: Some(session.to_string()),
            extra: Value::Null,
        })
        .unwrap()
}

#[test]
fn session_start_emits_the_notice_and_keeps_its_context() {
    let _l = lock();
    let c = Capacity::new(true);
    c.plant(State::Exhausted, Some(HOUR));
    let out = start("fresh-session");
    let msg = out["systemMessage"]
        .as_str()
        .expect("systemMessage at top level");
    assert!(
        msg.starts_with("⚠ all Claude accounts at limit · next reset "),
        "{msg}"
    );
    assert!(msg.contains("amplihack profile usage"), "{msg}");
    let hs = &out["hookSpecificOutput"];
    assert_eq!(hs["hookEventName"], "SessionStart");
    assert!(hs.get("indexing_status").is_some(), "{out}");
    let ctx = hs["additionalContext"].as_str().unwrap();
    assert!(ctx.contains(MODEL_HINT), "{ctx}");
    let workflow = amplihack_types::workflow::DEFAULT_WORKFLOW_SESSION_CONTEXT;
    let at = ctx.find(MODEL_HINT).unwrap();
    assert!(
        ctx[..at].contains(workflow.lines().next().unwrap_or(workflow)),
        "the notice is appended after the existing parts: {ctx}"
    );
}

#[test]
fn the_first_prompt_after_session_start_is_quiet() {
    let _l = lock();
    let c = Capacity::new(true);
    c.plant(State::Low, Some(HOUR));
    assert!(start("s1")["systemMessage"].is_string());
    let p = prompt("s1");
    assert!(p.get("systemMessage").is_none(), "{p}");
}

#[test]
fn session_start_is_unchanged_when_off_or_ok() {
    let _l = lock();
    let c = Capacity::new(false);
    c.plant(State::Exhausted, Some(HOUR));
    let off = start("off");
    assert!(off.get("systemMessage").is_none(), "{off}");
    let ctx = off["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(!ctx.contains(MODEL_HINT), "{ctx}");
    drop(c);

    let c = Capacity::new(true);
    c.plant(State::Ok, None);
    let ok = start("ok");
    assert!(ok.get("systemMessage").is_none(), "{ok}");
}

#[test]
fn session_start_survives_a_corrupt_file() {
    let _l = lock();
    let c = Capacity::new(true);
    c.plant_raw(b"\x00not json at all");
    let out = start("corrupt");
    assert!(out.get("systemMessage").is_none(), "{out}");
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
}

#[test]
fn a_nested_session_start_gets_context_only() {
    let _l = lock();
    let mut c = Capacity::new(true);
    c.set_depth("2");
    c.plant(State::Low, Some(HOUR));
    let out = start("nested");
    assert!(out.get("systemMessage").is_none(), "{out}");
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(ctx.contains(MODEL_HINT), "{ctx}");
}
