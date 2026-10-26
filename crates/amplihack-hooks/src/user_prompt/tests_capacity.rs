//! UserPromptSubmit and the no-capacity notice: shown once per state change
//! per session as a top-level `systemMessage`, with model context added to
//! `additionalContext`; byte-identical output when there is no notice; never
//! an error.

use super::*;
use crate::capacity_notice::tests::{
    Capacity, FORGED_OVER, FORGED_WRITER, HOUR, MODEL_HINT, forged,
};
use crate::test_support::{EnvVarGuard, env_lock};
use amplihack_types::HookInput;
use amplihack_utils::claude_capacity::State;
use serde_json::Value;

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock().lock().unwrap_or_else(|p| p.into_inner())
}

fn prompt(text: &str, session: Option<&str>) -> Value {
    UserPromptSubmitHook::with_prompt_root(|| None)
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some(text.to_string()),
            session_id: session.map(str::to_string),
            extra: Value::Null,
        })
        .expect("the hook never fails")
}

fn system_message(v: &Value) -> Option<&str> {
    v.get("systemMessage").and_then(Value::as_str)
}

fn context(v: &Value) -> &str {
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("")
}

/// The output for `text` with no profile root at all: today's behaviour.
fn baseline(text: &str, session: &str) -> Vec<u8> {
    let _no_root = EnvVarGuard::set(
        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
        "/nonexistent-capacity-root",
    );
    serde_json::to_vec(&prompt(text, Some(session))).unwrap()
}

#[test]
fn notice_once_per_state_change_in_one_session() {
    let _l = lock();
    let c = Capacity::new(true);
    let s = Some("ups-session");

    c.plant(State::Low, Some(HOUR));
    let first = prompt("hello there", s);
    let msg = system_message(&first).expect("first low prompt shows the notice");
    assert!(
        msg.starts_with("⚠ all Claude accounts ≥90% · next reset "),
        "{msg}"
    );
    assert!(msg.contains("amplihack profile usage"), "{msg}");
    assert_eq!(
        first["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    assert!(context(&first).contains(MODEL_HINT), "{first}");

    let second = prompt("and again", s);
    assert_eq!(system_message(&second), None, "repeat is quiet: {second}");
    assert!(!context(&second).contains(MODEL_HINT), "{second}");

    c.plant(State::Exhausted, Some(HOUR));
    let worse = prompt("still going", s);
    let msg = system_message(&worse).expect("low -> exhausted shows again");
    assert!(msg.contains("at limit"), "{msg}");

    c.plant(State::Ok, None);
    let ok = prompt("recovered?", s);
    assert_eq!(system_message(&ok), None);

    c.plant(State::Low, Some(HOUR));
    let again = prompt("dropped again", s);
    assert!(system_message(&again).is_some(), "ok -> low shows again");
}

#[test]
fn notice_is_emitted_even_with_no_other_context() {
    // The "no other context" path used to return `{}`.
    let _l = lock();
    let c = Capacity::new(true);
    c.plant(State::Low, Some(HOUR));
    let out = prompt("plain prompt", Some("lonely"));
    assert!(system_message(&out).is_some(), "{out}");
    assert!(context(&out).contains(MODEL_HINT), "{out}");
    assert_eq!(
        out["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
}

#[test]
fn existing_context_is_kept_alongside_the_notice() {
    let _l = lock();
    let c = Capacity::new(true);
    c.plant(State::Low, Some(HOUR));
    let out = prompt("/dev continue the audit", Some("with-dev"));
    let ctx = context(&out);
    assert!(ctx.contains("/dev workflow detected"), "{ctx}");
    assert!(ctx.contains(MODEL_HINT), "{ctx}");
    assert!(system_message(&out).is_some());
}

#[test]
fn an_empty_prompt_still_returns_empty() {
    let _l = lock();
    let c = Capacity::new(true);
    c.plant(State::Exhausted, Some(HOUR));
    let out = prompt("", Some("empty"));
    assert_eq!(out, serde_json::json!({}));
    // ...and the empty prompt did not use up the session's notice.
    assert!(system_message(&prompt("real prompt", Some("empty"))).is_some());
}

#[test]
fn off_is_silent_and_byte_identical() {
    let _l = lock();
    let c = Capacity::new(false);
    let want = baseline("hello there", "off-session");
    c.plant(State::Exhausted, Some(HOUR));
    let got = serde_json::to_vec(&prompt("hello there", Some("off-session"))).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&got),
        String::from_utf8_lossy(&want)
    );
}

#[test]
fn missing_or_corrupt_file_is_success_with_byte_identical_output() {
    let _l = lock();
    let c = Capacity::new(true);
    let want = baseline("hello there", "golden");
    let missing = serde_json::to_vec(&prompt("hello there", Some("golden"))).unwrap();
    assert_eq!(missing, want, "missing file");
    for bytes in [&b"{corrupt"[..], b"", b"[]", b"\xff\xfe"] {
        c.plant_raw(bytes);
        let got = serde_json::to_vec(&prompt("hello there", Some("golden"))).unwrap();
        assert_eq!(got, want, "{:?}", String::from_utf8_lossy(bytes));
    }
    c.plant(State::Ok, None);
    let ok = serde_json::to_vec(&prompt("hello there", Some("golden"))).unwrap();
    assert_eq!(ok, want, "ok state");
}

#[test]
fn a_symlinked_capacity_file_is_ignored() {
    let _l = lock();
    let c = Capacity::new(true);
    let real = c.root.join("real-capacity.json");
    std::fs::write(
        &real,
        serde_json::to_vec(&Capacity::file(State::Exhausted, Some(HOUR))).unwrap(),
    )
    .unwrap();
    std::os::unix::fs::symlink(&real, c.capacity_path()).unwrap();
    let out = prompt("hello there", Some("sym"));
    assert_eq!(system_message(&out), None, "{out}");
}

#[test]
fn a_nested_recipe_session_gets_model_context_only() {
    let _l = lock();
    let mut c = Capacity::new(true);
    c.set_depth("1");
    c.plant(State::Low, Some(HOUR));
    let out = prompt("agent step prompt", Some("nested"));
    assert_eq!(system_message(&out), None, "{out}");
    assert!(context(&out).contains(MODEL_HINT), "{out}");
}

#[test]
fn forged_file_strings_never_reach_the_output() {
    let _l = lock();
    let c = Capacity::new(true);
    c.plant_file(&forged());
    let out = prompt("hello there", Some("forged"));
    let text = serde_json::to_string(&out).unwrap();
    assert!(system_message(&out).is_some(), "{text}");
    assert!(!text.contains(FORGED_OVER), "{text}");
    assert!(!text.contains(FORGED_WRITER), "{text}");
}

#[test]
fn without_a_session_id_every_prompt_shows() {
    let _l = lock();
    let c = Capacity::new(true);
    c.plant(State::Low, Some(HOUR));
    assert!(system_message(&prompt("one", None)).is_some());
    assert!(system_message(&prompt("two", None)).is_some());
    assert!(
        system_message(&prompt("three", Some(""))).is_some(),
        "empty id"
    );
}

#[test]
fn a_passed_reset_stops_the_notice() {
    let _l = lock();
    let c = Capacity::new(true);
    c.plant(State::Exhausted, Some(-60));
    let out = prompt("hello there", Some("reset"));
    assert_eq!(system_message(&out), None, "{out}");
}
