//! Issue #2589 (TDD): the Copilot CLI half of the dangerous-command policy.
//!
//! Copilot CLI has no user-level file for command deny rules:
//! `permissions-config.json` holds saved approvals and does not support deny
//! rules, and `--deny-tool` applies to the current session only. Its
//! documented precedence is that deny wins over allow, including over
//! `--allow-all`, which amplihack already injects. So the renderer is an
//! argv injection at launch, not a file writer, and that difference is
//! recorded in the PR (D1).
//!
//! Expected change in `command.rs`: for `binary.name == "copilot"`,
//! `build_command_for_dir` injects `policy::copilot::deny_args(..)` beside
//! the existing `--allow-all` injection, and does so whether or not
//! `--allow-all` itself is injected.

use super::command::build_command_for_dir;
use crate::binary_finder::BinaryInfo;
use crate::policy;
use crate::test_support::{EnvGuard, home_env_lock};
use std::path::PathBuf;

fn binary(name: &str) -> BinaryInfo {
    BinaryInfo {
        name: name.to_string(),
        path: PathBuf::from(format!("/usr/bin/{name}")),
        version: None,
    }
}

fn argv(cmd: &std::process::Command) -> Vec<String> {
    cmd.get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect()
}

fn build(name: &str, extra: &[String]) -> Vec<String> {
    let cmd = build_command_for_dir(
        &binary(name),
        false,
        false,
        false,
        extra,
        None,
        false,
        None,
        Some(std::path::Path::new("/tmp/amplihack-contract-test")),
        None,
    )
    .expect("a contract path is supplied");
    argv(&cmd)
}

/// Returns the value that follows each `--deny-tool` in `args`.
fn denied(args: &[String]) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == "--deny-tool")
        .map(|w| w[1].clone())
        .collect()
}

#[test]
fn copilot_launch_carries_a_deny_tool_flag_for_every_policy_prefix() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let args = build("copilot", &[]);
    let expected: Vec<String> = policy::copilot::deny_args(&[], &[])
        .windows(2)
        .filter(|w| w[0] == "--deny-tool")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(denied(&args), expected, "{args:?}");
    assert!(
        denied(&args).contains(&"shell(pkill:*)".to_string()),
        "{args:?}"
    );
}

/// The deny flags are what makes `--allow-all` survivable, so they are not
/// conditional on the allow-all decision.
#[test]
fn the_deny_flags_are_injected_even_when_allow_all_is_suppressed() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::set([("AMPLIHACK_COPILOT_NO_ALLOW_ALL", "1")]);
    let args = build("copilot", &[]);
    assert!(
        !args.iter().any(|a| a == "--allow-all"),
        "the opt-out still suppresses allow-all: {args:?}"
    );
    assert!(
        denied(&args).contains(&"shell(pkill:*)".to_string()),
        "but the policy still applies: {args:?}"
    );
}

#[test]
fn the_deny_flags_are_injected_alongside_allow_all_by_default() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::unset(["AMPLIHACK_COPILOT_NO_ALLOW_ALL"]);
    let args = build("copilot", &[]);
    assert!(args.iter().any(|a| a == "--allow-all"), "{args:?}");
    assert!(!denied(&args).is_empty(), "{args:?}");
}

#[test]
fn a_users_own_deny_tool_for_the_same_prefix_is_not_duplicated() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let extra = vec!["--deny-tool".to_string(), "shell(pkill:*)".to_string()];
    let args = build("copilot", &extra);
    assert_eq!(
        denied(&args)
            .iter()
            .filter(|v| *v == "shell(pkill:*)")
            .count(),
        1,
        "{args:?}"
    );
    assert!(
        denied(&args).contains(&"shell(killall:*)".to_string()),
        "the rest still render: {args:?}"
    );
}

#[test]
fn the_joined_deny_tool_spelling_also_suppresses_the_duplicate() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let extra = vec!["--deny-tool=shell(pkill:*)".to_string()];
    let args = build("copilot", &extra);
    assert!(
        !denied(&args).contains(&"shell(pkill:*)".to_string()),
        "{args:?}"
    );
}

#[test]
fn the_users_own_arguments_stay_last() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let extra = vec!["--banner".to_string(), "mine".to_string()];
    let args = build("copilot", &extra);
    assert_eq!(
        &args[args.len() - 2..],
        ["--banner", "mine"],
        "every injection goes before the user's arguments: {args:?}"
    );
}

#[test]
fn no_other_harness_receives_a_deny_tool_flag() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for name in ["claude", "codex", "amplifier"] {
        let args = build(name, &[]);
        assert!(
            !args.iter().any(|a| a.starts_with("--deny-tool")),
            "{name} does not take --deny-tool: {args:?}"
        );
    }
}

/// An instance addition reaches the launch rendering too (T6).
#[test]
fn an_instance_addition_reaches_the_copilot_argv() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".amplihack")).unwrap();
    std::fs::write(
        home.join(".amplihack/config.toml"),
        "[policy]\ndeny_commands = [\"shutdown\"]\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            home.join(".amplihack/config.toml"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let previous = crate::test_support::set_home(&home);

    let args = build("copilot", &[]);
    crate::test_support::restore_home(previous);

    assert!(
        denied(&args).contains(&"shell(shutdown:*)".to_string()),
        "{args:?}"
    );
    assert!(
        denied(&args).contains(&"shell(pkill:*)".to_string()),
        "an addition removes no default: {args:?}"
    );
}

/// SEC-D5 and #2030: the flags carry short command names, never task text, so
/// the argv surface that made `pkill -f` dangerous stays small.
#[test]
fn the_injected_values_are_short_command_names_only() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for value in denied(&build("copilot", &[])) {
        assert!(value.starts_with("shell("), "{value}");
        // The `:*` suffix is what GitHub documents as the shell-rule prefix
        // form. A bare `shell(<prefix>)` is documented as matching that text
        // literally, which would spare every invocation that has arguments.
        assert!(value.ends_with(":*)"), "{value}");
        assert!(value.len() <= 64, "{value}");
    }
}

/// The documented side effect of the `:*` suffix: a caller who denied the
/// bare stem is not recognised, and the suffixed form is added beside it.
/// Both deny, so the duplicate is harmless; this pins that it is a duplicate
/// and not a gap.
#[test]
fn a_users_bare_stem_deny_tool_does_not_suppress_the_suffixed_form() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let extra = vec!["--deny-tool=shell(pkill)".to_string()];
    let args = build("copilot", &extra);
    assert!(
        denied(&args).contains(&"shell(pkill:*)".to_string()),
        "the policy still renders its own form: {args:?}"
    );
}

// --- the Claude-family harnesses (F9, D7) -----------------------------------

/// `amplifier`, `rusty` and `rustyclawd` need no writer of their own: each is
/// launched with `CLAUDE_CONFIG_DIR` pinned to the directory whose
/// `settings.json` the Claude writer fills, so one file covers all four. This
/// is why the install output claims no missing mechanism for them.
#[test]
fn every_claude_family_harness_is_pinned_to_the_directory_the_policy_is_written_into() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let config_dir = tmp.path().join("profile");
    std::fs::create_dir_all(&config_dir).unwrap();

    for name in ["claude", "rusty", "rustyclawd", "amplifier"] {
        let cmd = build_command_for_dir(
            &binary(name),
            false,
            false,
            false,
            &[],
            None,
            false,
            Some(&config_dir),
            Some(std::path::Path::new("/tmp/amplihack-contract-test")),
            None,
        )
        .expect("a contract path is supplied");
        let pinned = cmd
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new("CLAUDE_CONFIG_DIR"))
            .and_then(|(_, value)| value)
            .map(std::path::PathBuf::from);
        assert_eq!(
            pinned.as_deref(),
            Some(config_dir.as_path()),
            "{name} must read the settings file the Claude writer fills"
        );
    }
}
