use super::super::check::should_skip_update_check;
use super::super::*;

/// When AMPLIHACK_NONINTERACTIVE=1 is set, ALL subcommands — including launch
/// commands — must skip the update check to avoid polluting scripted output.
#[test]
fn test_skip_update_check_when_noninteractive_env_set() {
    unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", "1") };
    let result = should_skip_update_check_for_subcommand("launch");
    unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") };
    assert!(
        result,
        "should_skip_update_check_for_subcommand('launch') must return true \
         when AMPLIHACK_NONINTERACTIVE=1"
    );
}

/// When AMPLIHACK_PARITY_TEST=1 is set, the update check must be suppressed.
#[test]
fn test_skip_update_check_when_parity_test_env_set() {
    unsafe { std::env::set_var("AMPLIHACK_PARITY_TEST", "1") };
    let result = should_skip_update_check_for_subcommand("launch");
    unsafe { std::env::remove_var("AMPLIHACK_PARITY_TEST") };
    assert!(
        result,
        "should_skip_update_check_for_subcommand('launch') must return true \
         when AMPLIHACK_PARITY_TEST=1"
    );
}

/// The `mode` subcommand is not a launch command — update checks must be skipped.
#[test]
fn test_skip_update_check_for_mode_subcommand() {
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    assert!(
        should_skip_update_check_for_subcommand("mode"),
        "should_skip_update_check_for_subcommand('mode') must return true — \
         'mode' is not a launch command"
    );
}

/// The `plugin` subcommand is not a launch command — update checks must be skipped.
#[test]
fn test_skip_update_check_for_plugin_subcommand() {
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    assert!(
        should_skip_update_check_for_subcommand("plugin"),
        "should_skip_update_check_for_subcommand('plugin') must return true"
    );
}

/// Unknown subcommands must skip the update check.
#[test]
fn test_skip_update_check_for_unknown_subcommand() {
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    assert!(
        should_skip_update_check_for_subcommand("totally-unknown-command"),
        "should_skip_update_check_for_subcommand('totally-unknown-command') must return true"
    );
}

/// The `launch` subcommand IS a launch command — update check must proceed.
#[test]
fn test_allow_update_check_for_launch_subcommand() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let prev_ci = std::env::var_os("CI");
    let prev_ab = std::env::var_os("AMPLIHACK_AGENT_BINARY");
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
        std::env::remove_var("CI");
        std::env::remove_var("AMPLIHACK_AGENT_BINARY");
    }
    let result = !should_skip_update_check_for_subcommand("launch");
    unsafe {
        match prev_ci {
            Some(v) => std::env::set_var("CI", v),
            None => std::env::remove_var("CI"),
        }
        match prev_ab {
            Some(v) => std::env::set_var("AMPLIHACK_AGENT_BINARY", v),
            None => std::env::remove_var("AMPLIHACK_AGENT_BINARY"),
        }
    }
    assert!(
        result,
        "should_skip_update_check_for_subcommand('launch') must return false"
    );
}

/// The `claude` subcommand IS a launch command.
#[test]
fn test_allow_update_check_for_claude_subcommand() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let prev_ci = std::env::var_os("CI");
    let prev_ab = std::env::var_os("AMPLIHACK_AGENT_BINARY");
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
        std::env::remove_var("CI");
        std::env::remove_var("AMPLIHACK_AGENT_BINARY");
    }
    let result = !should_skip_update_check_for_subcommand("claude");
    unsafe {
        match prev_ci {
            Some(v) => std::env::set_var("CI", v),
            None => std::env::remove_var("CI"),
        }
        match prev_ab {
            Some(v) => std::env::set_var("AMPLIHACK_AGENT_BINARY", v),
            None => std::env::remove_var("AMPLIHACK_AGENT_BINARY"),
        }
    }
    assert!(
        result,
        "should_skip_update_check_for_subcommand('claude') must return false"
    );
}

#[test]
fn should_skip_update_check_for_update_related_args() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let prev_ni = std::env::var_os("AMPLIHACK_NONINTERACTIVE");
    let prev_pt = std::env::var_os("AMPLIHACK_PARITY_TEST");
    let prev_nuc = std::env::var_os(NO_UPDATE_CHECK_ENV);
    let prev_ci = std::env::var_os("CI");
    let prev_ab = std::env::var_os("AMPLIHACK_AGENT_BINARY");
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
        std::env::remove_var("CI");
        std::env::remove_var("AMPLIHACK_AGENT_BINARY");
    }
    assert!(should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("update")
    ]));
    assert!(should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("version")
    ]));
    assert!(should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("help")
    ]));
    assert!(should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("-V")
    ]));
    assert!(!should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("copilot")
    ]));
    unsafe {
        match prev_ni {
            Some(v) => std::env::set_var("AMPLIHACK_NONINTERACTIVE", v),
            None => std::env::remove_var("AMPLIHACK_NONINTERACTIVE"),
        }
        match prev_pt {
            Some(v) => std::env::set_var("AMPLIHACK_PARITY_TEST", v),
            None => std::env::remove_var("AMPLIHACK_PARITY_TEST"),
        }
        match prev_nuc {
            Some(v) => std::env::set_var(NO_UPDATE_CHECK_ENV, v),
            None => std::env::remove_var(NO_UPDATE_CHECK_ENV),
        }
        match prev_ci {
            Some(v) => std::env::set_var("CI", v),
            None => std::env::remove_var("CI"),
        }
        match prev_ab {
            Some(v) => std::env::set_var("AMPLIHACK_AGENT_BINARY", v),
            None => std::env::remove_var("AMPLIHACK_AGENT_BINARY"),
        }
    }
}

#[test]
fn should_skip_update_check_for_non_launch_subcommands() {
    for subcmd in &["mode", "plugin", "recipe", "memory", "install", "doctor"] {
        assert!(
            should_skip_update_check(&[OsString::from("amplihack"), OsString::from(*subcmd),]),
            "expected update check to be skipped for subcommand '{subcmd}'"
        );
    }
}

#[test]
fn should_not_skip_update_check_for_launch_subcommands() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let prev_ni = std::env::var_os("AMPLIHACK_NONINTERACTIVE");
    let prev_pt = std::env::var_os("AMPLIHACK_PARITY_TEST");
    let prev_nuc = std::env::var_os(NO_UPDATE_CHECK_ENV);
    let prev_ci = std::env::var_os("CI");
    let prev_ab = std::env::var_os("AMPLIHACK_AGENT_BINARY");
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
        std::env::remove_var("CI");
        std::env::remove_var("AMPLIHACK_AGENT_BINARY");
    }
    for subcmd in &["launch", "claude", "copilot", "codex", "amplifier"] {
        assert!(
            !should_skip_update_check(&[OsString::from("amplihack"), OsString::from(*subcmd),]),
            "expected update check to NOT be skipped for launch subcommand '{subcmd}'"
        );
    }
    unsafe {
        match prev_ni {
            Some(v) => std::env::set_var("AMPLIHACK_NONINTERACTIVE", v),
            None => std::env::remove_var("AMPLIHACK_NONINTERACTIVE"),
        }
        match prev_pt {
            Some(v) => std::env::set_var("AMPLIHACK_PARITY_TEST", v),
            None => std::env::remove_var("AMPLIHACK_PARITY_TEST"),
        }
        match prev_nuc {
            Some(v) => std::env::set_var(NO_UPDATE_CHECK_ENV, v),
            None => std::env::remove_var(NO_UPDATE_CHECK_ENV),
        }
        match prev_ci {
            Some(v) => std::env::set_var("CI", v),
            None => std::env::remove_var("CI"),
        }
        match prev_ab {
            Some(v) => std::env::set_var("AMPLIHACK_AGENT_BINARY", v),
            None => std::env::remove_var("AMPLIHACK_AGENT_BINARY"),
        }
    }
}
