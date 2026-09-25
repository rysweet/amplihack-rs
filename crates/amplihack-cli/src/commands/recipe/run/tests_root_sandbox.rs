//! Issue #1482: `recipe run` pre-flight for `claude --dangerously-skip-permissions`
//! as root.

use super::*;
use amplihack_utils::root_sandbox::SkipPermissionsEnv;

fn recipe(steps: &str) -> RecipeDoc {
    parse_recipe_text(&format!("name: probe\nsteps:\n{steps}")).expect("recipe parses")
}

const AGENT_STEP: &str = "  - id: think\n    type: agent\n    prompt: hi\n";
const SUB_RECIPE_STEP: &str = "  - id: nested\n    type: recipe\n    recipe: other\n";
const BASH_STEP: &str = "  - id: echo\n    type: bash\n    command: echo hi\n";

#[test]
fn root_outside_a_sandbox_fails_before_agent_steps_naming_is_sandbox() {
    for steps in [AGENT_STEP, SUB_RECIPE_STEP] {
        let error = preflight_root_sandbox(
            &recipe(steps),
            "claude",
            &SkipPermissionsEnv::RootOutsideSandbox,
        )
        .expect_err("must fail up front");
        let message = error.to_string();
        assert!(
            message.contains("recipe pre-flight failed for 'probe'"),
            "{message}"
        );
        assert!(message.contains("IS_SANDBOX=1"), "{message}");
    }
}

#[test]
fn explicit_non_1_is_sandbox_fails_as_root() {
    let error = preflight_root_sandbox(
        &recipe(AGENT_STEP),
        "claude",
        &SkipPermissionsEnv::ExplicitlyNotSandboxed {
            value: "0".to_string(),
        },
    )
    .expect_err("must fail up front");
    assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
}

#[test]
fn launchable_decisions_pass() {
    for decision in [
        SkipPermissionsEnv::NotRoot,
        SkipPermissionsEnv::AlreadySandboxed,
        SkipPermissionsEnv::SetSandbox {
            signal: "/.dockerenv",
        },
    ] {
        preflight_root_sandbox(&recipe(AGENT_STEP), "claude", &decision).unwrap();
    }
}

#[test]
fn bash_only_recipes_and_other_agents_are_not_blocked() {
    preflight_root_sandbox(
        &recipe(BASH_STEP),
        "claude",
        &SkipPermissionsEnv::RootOutsideSandbox,
    )
    .unwrap();
    for binary in ["copilot", "codex", "amplifier"] {
        preflight_root_sandbox(
            &recipe(AGENT_STEP),
            binary,
            &SkipPermissionsEnv::RootOutsideSandbox,
        )
        .unwrap();
    }
}
