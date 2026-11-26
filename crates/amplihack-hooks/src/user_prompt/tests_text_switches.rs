//! UserPromptSubmit with `text.prompt_preferences` off and in a recipe step
//! (issues #3247 and #3199): the preferences block is left out, and the
//! memory context and the capacity notice are still added.
//!
//! The preferences block is detected by comparing with what
//! `load_user_preferences_with_patterns` returns for the same fixture, never
//! by its wording.

use super::*;
use crate::capacity_notice::tests::{Capacity, HOUR, MODEL_HINT};
use crate::test_support::{EnvVarGuard, env_lock};
use amplihack_types::{HookInput, ProjectDirs};
use amplihack_utils::claude_capacity::State;
use serde_json::Value;
use std::fs;

const LEARNING: &str = "The auth middleware rejected expired tokens before refresh";
const PROMPT: &str = "/analyze why the auth middleware rejected expired tokens";

fn context(v: &Value) -> &str {
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("")
}

/// The prompt hook's output for `session`, with a learning stored for that
/// session and the capacity notice due.
fn prompt(capacity: &Capacity, session: &str) -> Value {
    amplihack_memory::cli_memory::store_session_learning(session, "analyzer", LEARNING, None, true)
        .unwrap();
    capacity.plant(State::Low, Some(HOUR));
    UserPromptSubmitHook::with_prompt_root(|| None)
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some(PROMPT.to_string()),
            session_id: Some(session.to_string()),
            extra: Value::Null,
        })
        .expect("the hook never fails")
}

/// What the preferences block is for the fixture, from the code that builds it.
fn preferences_block() -> String {
    let (block, _) = preferences::load_user_preferences_with_patterns(&ProjectDirs::from_cwd());
    block.expect("fixture check: the preferences file gives a block")
}

/// Run `check` with a capacity fixture, a preferences file and stored
/// memory, and every variable this test changes cleared first.
fn with_fixture(check: impl FnOnce(&Capacity)) {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let capacity = Capacity::new(true);
    let framework = tempfile::tempdir().unwrap();
    fs::create_dir_all(framework.path().join(".claude/context")).unwrap();
    fs::write(
        framework.path().join(".claude/context/USER_PREFERENCES.md"),
        "| Setting | Value |\n| --- | --- |\n| Verbosity | concise |\n",
    )
    .unwrap();
    let _guards = [
        EnvVarGuard::set("AMPLIHACK_ROOT", framework.path()),
        EnvVarGuard::set("AMPLIHACK_MEMORY_BACKEND", "sqlite"),
        EnvVarGuard::unset(crate::recipe_step::RECIPE_RUN_ID_ENV),
        EnvVarGuard::unset(crate::recipe_step::STEP_ID_ENV),
        EnvVarGuard::unset("AMPLIHACK_PROMPT_PREFERENCES"),
        EnvVarGuard::unset("AMPLIHACK_ROUTING_TEXT"),
    ];
    check(&capacity);
}

fn assert_block_left_out_and_the_rest_kept(out: &Value, label: &str) {
    let text = context(out);
    assert!(
        !text.contains(&preferences_block()),
        "{label}: no preferences block: {out}"
    );
    assert!(
        text.contains(LEARNING),
        "{label}: memory context kept: {out}"
    );
    assert!(
        text.contains(MODEL_HINT),
        "{label}: capacity context kept: {out}"
    );
}

#[test]
fn a_person_s_prompt_gets_the_preferences_block_by_default() {
    with_fixture(|capacity| {
        let out = prompt(capacity, "default-session");
        let text = context(&out);
        assert!(text.contains(&preferences_block()), "{out}");
        assert!(text.contains(LEARNING), "{out}");
        assert!(text.contains(MODEL_HINT), "{out}");
    });
}

#[test]
fn a_recipe_step_gets_no_preferences_block_whatever_the_setting() {
    for var in [
        crate::recipe_step::RECIPE_RUN_ID_ENV,
        crate::recipe_step::STEP_ID_ENV,
    ] {
        for setting in [None, Some("on")] {
            with_fixture(|capacity| {
                let _step = EnvVarGuard::set(var, "recipe-run-1");
                let _setting =
                    setting.map(|value| EnvVarGuard::set("AMPLIHACK_PROMPT_PREFERENCES", value));
                let session = format!("{var}-{setting:?}");
                let out = prompt(capacity, &session);
                assert_block_left_out_and_the_rest_kept(&out, &session);
            });
        }
    }
}

#[test]
fn an_empty_recipe_variable_is_not_a_recipe_step() {
    with_fixture(|capacity| {
        let _run = EnvVarGuard::set(crate::recipe_step::RECIPE_RUN_ID_ENV, "");
        let _step = EnvVarGuard::set(crate::recipe_step::STEP_ID_ENV, "");
        let out = prompt(capacity, "empty-vars");
        assert!(context(&out).contains(&preferences_block()), "{out}");
    });
}

#[test]
fn prompt_preferences_off_leaves_out_the_block_and_keeps_the_rest() {
    with_fixture(|capacity| {
        let _off = EnvVarGuard::set("AMPLIHACK_PROMPT_PREFERENCES", "off");
        let out = prompt(capacity, "prefs-off");
        assert_block_left_out_and_the_rest_kept(&out, "prefs-off");
    });
}

#[test]
fn learned_patterns_line_is_left_out_with_the_block() {
    with_fixture(|capacity| {
        let framework = std::env::var_os("AMPLIHACK_ROOT").unwrap();
        fs::write(
            std::path::Path::new(&framework).join(".claude/context/USER_PREFERENCES.md"),
            "| Setting | Value |\n| --- | --- |\n| Verbosity | concise |\n\n\
             ## Learned Patterns\n\nPrefer small commits.\n",
        )
        .unwrap();
        let (_, learned) =
            preferences::load_user_preferences_with_patterns(&ProjectDirs::from_cwd());
        assert!(learned, "fixture check: the file has learned patterns");
        let on = prompt(capacity, "learned-on");
        let _off = EnvVarGuard::set("AMPLIHACK_PROMPT_PREFERENCES", "off");
        let off = prompt(capacity, "learned-off");
        // Parts are joined with a blank line. With the switch off, the
        // context loses the block's parts and one more: the learned line.
        let parts = |text: &str| text.split("\n\n").count();
        assert_eq!(
            parts(context(&on)),
            parts(context(&off)) + parts(&preferences_block()) + 1,
            "on: {on}\noff: {off}"
        );
        assert_block_left_out_and_the_rest_kept(&off, "learned-off");
    });
}
