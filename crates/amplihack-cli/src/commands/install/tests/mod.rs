use super::*;

mod binary_tests;
mod claude_command_staging_tests;
mod deploy_binary_tests;
mod helpers;
mod hook_specs;
mod hook_staging_tests;
mod install_flow;
mod issue_1266_restage_loop;
mod issue_1438_skill_publication;
mod issue_1449_partial_registration;
mod issue_1526_implicit_downgrade;
mod issue_1526_implicit_mode;
mod issue_1526_launch_bootstrap;
#[cfg(unix)]
mod issue_1745_launch_repair_statusline;
// Issue #1699: the real-profile-root guard also runs in this binary, where
// the in-process install tests run.
#[cfg(unix)]
#[path = "../../../../tests/issue_1699_real_root_guard.rs"]
mod issue_1699_real_root_guard;
#[cfg(unix)]
mod issue_1954_install_lock;
#[cfg(unix)]
mod issue_1954_release_layout;
mod issue_1955_fetch_backoff;
mod issue_1955_force_refresh;
mod issue_1955_manifest;
#[cfg(unix)]
mod issue_1955_modes;
mod issue_1955_self_heal;
mod issue_1955_source_gate;
mod issue_1955_support;
#[cfg(unix)]
mod issue_1955_tag_fetch;
mod issue_527_tests;
mod issue_911_durable_hooks_path;
#[cfg(unix)]
mod limit_resume_install;
mod mermaid_cli_tests;
mod output_regression_tests;
mod path_conflict_tests;
mod path_precedence_tests;
#[cfg(unix)]
mod profile_refresh_install;
mod same_path_tests;
mod settings_tests;
mod stale_wrapper_repair;
mod uninstall_tests;
mod wrapper_tests;

mod interactive_tests;

#[cfg(unix)]
mod codex_alias_paths;
#[cfg(unix)]
mod codex_disabled_rollback;
#[cfg(unix)]
mod codex_failure_recovery;
#[cfg(unix)]
mod codex_plugin_tests;
#[cfg(unix)]
mod codex_runner_delivery;
#[cfg(unix)]
mod native_registration_fixture;

#[cfg(unix)]
mod issue_2182_binary_isolation;
