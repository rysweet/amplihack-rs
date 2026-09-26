//! Issue #1482: the `amplihack claude` launch path — the one recipe agent steps
//! use — decides on IS_SANDBOX before spawning and applies it to the child only.

use super::*;
use crate::binary_finder::BinaryInfo;
use amplihack_utils::root_sandbox::{IS_SANDBOX_ENV, SKIP_PERMISSIONS_FLAG, SkipPermissionsEnv};
use std::path::PathBuf;

fn claude_binary() -> BinaryInfo {
    BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: Some("1.0.0".to_string()),
    }
}

fn child_is_sandbox(command: &std::process::Command) -> Option<String> {
    command
        .get_envs()
        .find(|(key, _)| *key == IS_SANDBOX_ENV)
        .and_then(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned())
}

#[test]
fn root_in_a_sandbox_sets_is_sandbox_on_the_launched_claude_only() {
    let decision =
        root_sandbox_for_launch("claude", true, &[], || SkipPermissionsEnv::SetSandbox {
            signal: "/.dockerenv",
        })
        .unwrap()
        .expect("claude with the flag needs a decision");

    let parent_before = std::env::var_os(IS_SANDBOX_ENV);
    let mut cmd = build_command(&claude_binary(), false, false, true, &[]);
    assert!(
        cmd.get_args().any(|arg| arg == SKIP_PERMISSIONS_FLAG),
        "the launch passes the flag"
    );
    decision.apply(&mut cmd).unwrap();

    assert_eq!(child_is_sandbox(&cmd).as_deref(), Some("1"));
    assert_eq!(
        std::env::var_os(IS_SANDBOX_ENV),
        parent_before,
        "amplihack's own environment is not modified"
    );
}

#[test]
fn root_outside_a_sandbox_stops_the_launch_naming_is_sandbox() {
    for decision in [
        SkipPermissionsEnv::RootOutsideSandbox,
        SkipPermissionsEnv::ExplicitlyNotSandboxed {
            value: "0".to_string(),
        },
    ] {
        let error = root_sandbox_for_launch("claude", true, &[], || decision.clone())
            .expect_err("the launch must stop before spawning");
        assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
    }
}

#[test]
fn a_user_passed_flag_is_covered_too() {
    let args = vec![SKIP_PERMISSIONS_FLAG.to_string(), "-p".to_string()];
    let error = root_sandbox_for_launch("claude", false, &args, || {
        SkipPermissionsEnv::RootOutsideSandbox
    })
    .expect_err("the flag in claude_args counts");
    assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
}

#[test]
fn launches_without_the_flag_or_for_other_tools_do_not_probe() {
    let probe = || -> SkipPermissionsEnv { panic!("must not probe for this launch") };
    assert_eq!(
        root_sandbox_for_launch("claude", false, &[], probe).unwrap(),
        None
    );
    for tool in ["copilot", "codex", "amplifier"] {
        assert_eq!(
            root_sandbox_for_launch(tool, true, &[], probe).unwrap(),
            None
        );
    }
}

#[test]
fn non_root_and_explicit_sandbox_leave_the_child_environment_alone() {
    for decision in [
        SkipPermissionsEnv::NotRoot,
        SkipPermissionsEnv::AlreadySandboxed,
    ] {
        let decision = root_sandbox_for_launch("claude", true, &[], || decision.clone())
            .unwrap()
            .expect("claude with the flag needs a decision");
        let mut cmd = build_command(&claude_binary(), false, false, true, &[]);
        decision.apply(&mut cmd).unwrap();
        assert_eq!(child_is_sandbox(&cmd), None);
    }
}

// --- the calls in run_launch_with -------------------------------------------

#[test]
fn launch_environment_puts_is_sandbox_on_the_child_last() {
    let set_sandbox = SkipPermissionsEnv::SetSandbox {
        signal: "/.dockerenv",
    };
    // Even an inherited or builder-set value is overridden by the decision.
    let mut cmd = std::process::Command::new("claude");
    apply_launch_environment(
        &mut cmd,
        EnvBuilder::new().set(IS_SANDBOX_ENV, "0"),
        None,
        Some(&set_sandbox),
    )
    .unwrap();
    assert_eq!(child_is_sandbox(&cmd).as_deref(), Some("1"));

    for decision in [None, Some(&SkipPermissionsEnv::NotRoot)] {
        let mut cmd = std::process::Command::new("claude");
        apply_launch_environment(&mut cmd, EnvBuilder::new(), None, decision).unwrap();
        assert_eq!(child_is_sandbox(&cmd), None, "{decision:?}");
    }

    let mut cmd = std::process::Command::new("claude");
    let error = apply_launch_environment(
        &mut cmd,
        EnvBuilder::new(),
        None,
        Some(&SkipPermissionsEnv::RootOutsideSandbox),
    )
    .expect_err("a refusal is never applied");
    assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
}

/// The decision in `run_launch_with` stops `amplihack claude` before anything
/// else runs (update check, bootstrap, binary lookup, spawn).
#[test]
fn claude_launch_stops_up_front_when_claude_code_would_refuse() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if std::env::var_os("AMPLIHACK_USE_DOCKER").is_some()
        || amplihack_utils::litellm_proxy::ProxyConfig::from_env()
            .ok()
            .flatten()
            .is_some()
    {
        // Those launches hand off before the root-sandbox decision.
        return;
    }
    for decision in [
        (|| SkipPermissionsEnv::RootOutsideSandbox) as fn() -> SkipPermissionsEnv,
        || SkipPermissionsEnv::ExplicitlyNotSandboxed {
            value: "0".to_string(),
        },
    ] {
        let error = run_launch_with(
            "claude",
            "claude",
            false,
            false,
            false,
            true,
            true,
            false,
            true,
            None,
            vec!["-p".to_string(), "hello".to_string()],
            amplihack_utils::launch_target::OverrideOrigin::User,
            None,
            decision,
        )
        .expect_err("the launch must stop before spawning");
        assert!(format!("{error:#}").contains("IS_SANDBOX=1"), "{error:#}");
    }
}
