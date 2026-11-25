//! Issue #2922 (TDD): a quiet child never sees the six profile-redirecting
//! variables, even when its parent deliberately exports them.
//!
//! `in_quiet_process_with_env` exports the named variables to the child's
//! environment. The purge that `quiet_process` fires in the child before the
//! body runs must remove them. The values are synthetic placeholders.

use crate::support::claude_profile_isolation::CLAUDE_PROFILE_ENV_VARS;
use crate::support::quiet_process::{child_set_var, in_quiet_process, in_quiet_process_with_env};

#[test]
fn all_six_variables_are_absent_in_a_child_the_parent_exported_them_to() {
    let exports: Vec<(&str, &str)> = CLAUDE_PROFILE_ENV_VARS
        .iter()
        .map(|name| (*name, "/nonexistent/issue-2922-placeholder"))
        .collect();
    in_quiet_process_with_env(
        module_path!(),
        "all_six_variables_are_absent_in_a_child_the_parent_exported_them_to",
        &exports,
        || {
            for name in CLAUDE_PROFILE_ENV_VARS {
                assert!(
                    std::env::var_os(name).is_none(),
                    "{name} reached the quiet child"
                );
            }
        },
    );
}

#[test]
fn moving_home_clears_codex_home_before_a_deliberate_override() {
    in_quiet_process(
        module_path!(),
        "moving_home_clears_codex_home_before_a_deliberate_override",
        || {
            let scratch = tempfile::tempdir().unwrap();
            let inherited = scratch.path().join("inherited-codex");
            child_set_var("CODEX_HOME", &inherited);
            child_set_var("HOME", scratch.path());
            assert!(std::env::var_os("CODEX_HOME").is_none());

            let selected = scratch.path().join("selected-codex");
            child_set_var("CODEX_HOME", &selected);
            assert_eq!(
                std::env::var_os("CODEX_HOME").as_deref(),
                Some(selected.as_os_str())
            );
        },
    );
}
