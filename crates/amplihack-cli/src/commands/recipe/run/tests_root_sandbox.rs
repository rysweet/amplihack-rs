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
            &mut std::io::sink(),
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
        &mut std::io::sink(),
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
        SkipPermissionsEnv::NormalizeExplicit {
            value: "yes".to_string(),
        },
    ] {
        preflight_root_sandbox(
            &recipe(AGENT_STEP),
            "claude",
            &decision,
            &mut std::io::sink(),
        )
        .unwrap();
    }
}

#[test]
fn bash_only_recipes_and_other_agents_are_not_blocked() {
    preflight_root_sandbox(
        &recipe(BASH_STEP),
        "claude",
        &SkipPermissionsEnv::RootOutsideSandbox,
        &mut std::io::sink(),
    )
    .unwrap();
    for binary in ["copilot", "codex", "amplifier"] {
        preflight_root_sandbox(
            &recipe(AGENT_STEP),
            binary,
            &SkipPermissionsEnv::RootOutsideSandbox,
            &mut std::io::sink(),
        )
        .unwrap();
    }
}

#[test]
fn an_automatic_enable_is_announced_once_by_the_preflight() {
    let mut notices = Vec::new();
    preflight_root_sandbox(
        &recipe(AGENT_STEP),
        "claude",
        &SkipPermissionsEnv::SetSandbox {
            signal: "/.dockerenv",
        },
        &mut notices,
    )
    .unwrap();
    let notices = String::from_utf8(notices).unwrap();
    assert_eq!(notices.lines().count(), 1, "{notices}");
    assert!(notices.contains("/.dockerenv"), "{notices}");
    assert!(notices.contains("IS_SANDBOX=1"), "{notices}");
    assert!(notices.contains("IS_SANDBOX=0"), "{notices}");
}

#[test]
fn nothing_is_announced_unless_amplihack_enables_it_for_agent_steps() {
    let quiet = [
        (AGENT_STEP, "claude", SkipPermissionsEnv::NotRoot),
        (AGENT_STEP, "claude", SkipPermissionsEnv::AlreadySandboxed),
        (
            AGENT_STEP,
            "claude",
            SkipPermissionsEnv::NormalizeExplicit {
                value: "yes".to_string(),
            },
        ),
        (
            BASH_STEP,
            "claude",
            SkipPermissionsEnv::SetSandbox {
                signal: "/.dockerenv",
            },
        ),
        (
            AGENT_STEP,
            "copilot",
            SkipPermissionsEnv::SetSandbox {
                signal: "/.dockerenv",
            },
        ),
    ];
    for (steps, binary, decision) in quiet {
        let mut notices = Vec::new();
        preflight_root_sandbox(&recipe(steps), binary, &decision, &mut notices).unwrap();
        assert!(notices.is_empty(), "{binary} {decision:?}");
    }
}

// --- the call in run_recipe_with ------------------------------------------

fn agent_recipe_file(dir: &std::path::Path) -> String {
    let path = dir.join("probe.yaml");
    std::fs::write(&path, format!("name: probe\nsteps:\n{AGENT_STEP}")).unwrap();
    path.display().to_string()
}

fn claude() -> String {
    "claude".to_string()
}

#[test]
fn recipe_run_stops_before_the_runner_when_claude_would_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let recipe_path = agent_recipe_file(dir.path());
    for decision in [
        (|| SkipPermissionsEnv::RootOutsideSandbox) as fn() -> SkipPermissionsEnv,
        || SkipPermissionsEnv::ExplicitlyNotSandboxed {
            value: "0".to_string(),
        },
    ] {
        let mut out = Vec::new();
        let result = run_recipe_with(
            &recipe_path,
            &[],
            false,
            false,
            "table",
            Some(&dir.path().display().to_string()),
            None,
            &RootSandboxPreflight {
                agent_binary: claude,
                decision,
            },
            &mut out,
        );
        assert!(result.is_err(), "the run must stop");
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("Error: recipe pre-flight failed for 'probe'"),
            "{out}"
        );
        assert!(out.contains("IS_SANDBOX=1"), "{out}");
    }
}
