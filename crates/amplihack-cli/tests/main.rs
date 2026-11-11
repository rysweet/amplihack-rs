//! Single integration-test binary for the `amplihack-cli` package.
//!
//! Every top-level `.rs` file in this directory used to be its own test target,
//! so Cargo compiled and linked one executable per file (none of which sccache
//! can cache: they are bin crate types). They are now modules of this one
//! crate root, wired as the single `[[test]] name = "cli_integration"` target
//! (`autotests = false` in the manifest). Each module keeps the name of its
//! old target minus any `_test` suffix, so a former
//! `cargo test -p amplihack-cli --test <name>` is now
//! `cargo test -p amplihack-cli --test cli_integration <module>::`.
//!
//! All modules share one process under `cargo test`. A test that must change
//! the environment or the working directory runs its body in a child process
//! of this binary (`support::quiet_process::in_quiet_process`) and makes the
//! change there with the `child_*` functions; stubs that tests exec are
//! written with `support::exec_stub`.
//!
//! To add a new integration test file: create `<name>.rs` here and add a
//! `#[path = "<name>.rs"] mod <name>;` line below. Do NOT add a new
//! `[[test]]` entry to the manifest. The guards in
//! `tests/integration/test_harness_binary_path_contract_test.rs`, which run in
//! the `amplihack` package's `integration` binary, fail on anything this root
//! does not reach and on process-state changes outside a quiet child.

#[path = "../../../tests/support/isolated_amplihack.rs"]
mod isolated_amplihack;
mod support;

#[path = "issue_1753_expired_live.rs"]
mod issue_1753_expired_live;
#[path = "issue_1753_login_identity.rs"]
mod issue_1753_login_identity;
#[path = "issue_1753_refusal_named_once.rs"]
mod issue_1753_refusal_named_once;
#[path = "issue_1947_login_identity.rs"]
mod issue_1947_login_identity;
#[path = "issue_1962_candide_cause.rs"]
mod issue_1962_candide_cause;
#[path = "issue_1962_causes.rs"]
mod issue_1962_causes;
#[path = "issue_1962_dir_rule.rs"]
mod issue_1962_dir_rule;
#[path = "issue_1962_dry_run.rs"]
mod issue_1962_dry_run;
#[path = "issue_1962_refresh_ledger.rs"]
mod issue_1962_refresh_ledger;
#[path = "issue_1962_seed.rs"]
mod issue_1962_seed;
#[path = "issue_1962_seed_cmd.rs"]
mod issue_1962_seed_cmd;
#[path = "issue_1962_seed_output.rs"]
mod issue_1962_seed_output;
#[path = "issue_1962_seed_scan.rs"]
mod issue_1962_seed_scan;
#[path = "issue_1976_orphaned_nested_run.rs"]
mod issue_1976_orphaned_nested_run;
#[path = "issue_1976_real_runner_nested_run.rs"]
mod issue_1976_real_runner_nested_run;
#[path = "issue_1976_removed_should_publish.rs"]
mod issue_1976_removed_should_publish;
#[path = "issue_2030_guards.rs"]
mod issue_2030_guards;
#[path = "issue_2030_no_prompt_in_argv.rs"]
mod issue_2030_no_prompt_in_argv;

#[path = "active_agent_binary_default_test.rs"]
mod active_agent_binary_default;
#[path = "agentic_workflow_first_present.rs"]
mod agentic_workflow_first_present;
#[path = "auto_mode_prompt_delivery.rs"]
mod auto_mode_prompt_delivery;
#[path = "bugfix_341_install_freshness.rs"]
mod bugfix_341_install_freshness;
#[path = "bugfix_install_tests.rs"]
mod bugfix_install_tests;
#[path = "cargo_cxx_pin_test.rs"]
mod cargo_cxx_pin;
#[path = "claude_code_plugin_manifest.rs"]
mod claude_code_plugin_manifest;
#[path = "claude_install_contract.rs"]
mod claude_install_contract;
#[path = "claude_profile_rotation_skill_present.rs"]
mod claude_profile_rotation_skill_present;
#[path = "codex_frontdoor_contract.rs"]
mod codex_frontdoor_contract;
#[path = "cycle2_module_budget.rs"]
mod cycle2_module_budget;
#[path = "dead_code_guard.rs"]
mod dead_code_guard;
#[path = "doctor_prompt_delivery.rs"]
mod doctor_prompt_delivery;
#[path = "install_update_output_contract.rs"]
mod install_update_output_contract;
#[path = "issue_1274_one_path_traversal.rs"]
mod issue_1274_one_path_traversal;
#[path = "issue_1333_leaf_steps_do_not_orchestrate.rs"]
mod issue_1333_leaf_steps_do_not_orchestrate;
#[path = "issue_1348_documented_cli_commands_exist.rs"]
mod issue_1348_documented_cli_commands_exist;
#[path = "issue_1356_release_tags_last.rs"]
mod issue_1356_release_tags_last;
#[path = "issue_1358_sccache_installed_before_cargo.rs"]
mod issue_1358_sccache_installed_before_cargo;
#[path = "issue_1526_release_version_stamp.rs"]
mod issue_1526_release_version_stamp;
#[path = "issue_1551_merge_guard.rs"]
mod issue_1551_merge_guard;
#[path = "issue_1552_investigation_report_written.rs"]
mod issue_1552_investigation_report_written;
#[path = "issue_1553_max_total_steps_cap.rs"]
mod issue_1553_max_total_steps_cap;
#[path = "issue_1561_step_outputs_prefer_recipe_var.rs"]
mod issue_1561_step_outputs_prefer_recipe_var;
#[path = "issue_1567_nested_profile.rs"]
mod issue_1567_nested_profile;
#[path = "issue_1569_auto_switch.rs"]
mod issue_1569_auto_switch;
#[path = "issue_1570_auto_prompt_file.rs"]
mod issue_1570_auto_prompt_file;
#[path = "issue_1570_autodrive.rs"]
mod issue_1570_autodrive;
#[path = "issue_1570_autodrive_recipes.rs"]
mod issue_1570_autodrive_recipes;
#[path = "issue_1570_body_file.rs"]
mod issue_1570_body_file;
#[path = "issue_1570_context_value.rs"]
mod issue_1570_context_value;
#[path = "issue_1570_recipe_context_file.rs"]
mod issue_1570_recipe_context_file;
#[path = "issue_1570_recipe_text.rs"]
mod issue_1570_recipe_text;
#[path = "issue_1570_run_output_stdin.rs"]
mod issue_1570_run_output_stdin;
#[path = "issue_1578_profile_usage.rs"]
mod issue_1578_profile_usage;
#[path = "issue_1592_daemon.rs"]
mod issue_1592_daemon;
#[path = "issue_1592_oneshot.rs"]
mod issue_1592_oneshot;
#[path = "issue_1592_recovery.rs"]
mod issue_1592_recovery;
#[path = "issue_1592_root.rs"]
mod issue_1592_root;
#[path = "issue_1592_root_rotator.rs"]
mod issue_1592_root_rotator;
#[path = "issue_1606_config_cli.rs"]
mod issue_1606_config_cli;
#[path = "issue_1606_detach.rs"]
mod issue_1606_detach;
#[path = "issue_1618_fixture.rs"]
mod issue_1618_fixture;
#[path = "issue_1618_wait_ends.rs"]
mod issue_1618_wait_ends;
#[path = "issue_1618_wait_profiles.rs"]
mod issue_1618_wait_profiles;
#[path = "issue_1618_wait_recipe.rs"]
mod issue_1618_wait_recipe;
#[path = "issue_1618_wait_settings.rs"]
mod issue_1618_wait_settings;
#[path = "issue_1618_wait_signals.rs"]
mod issue_1618_wait_signals;
#[path = "issue_1618_wait_stdin.rs"]
mod issue_1618_wait_stdin;
#[path = "issue_1618_wait_step.rs"]
mod issue_1618_wait_step;
#[path = "issue_1618_wait_threshold.rs"]
mod issue_1618_wait_threshold;
#[path = "issue_1634_shared_sessions.rs"]
mod issue_1634_shared_sessions;
#[path = "issue_1650_profile_identity.rs"]
mod issue_1650_profile_identity;
#[path = "issue_1659_limit_resume_gate.rs"]
mod issue_1659_limit_resume_gate;
#[path = "issue_1688_routing_gap_report.rs"]
mod issue_1688_routing_gap_report;
#[path = "issue_1690_isolated_autostart.rs"]
mod issue_1690_isolated_autostart;
#[path = "issue_1691_switch_live.rs"]
mod issue_1691_switch_live;
#[path = "issue_1698_probe_load.rs"]
mod issue_1698_probe_load;
#[path = "issue_1699_isolated_profile_root.rs"]
mod issue_1699_isolated_profile_root;
#[path = "issue_1699_real_root_guard.rs"]
mod issue_1699_real_root_guard;
#[path = "issue_1707_claim_check_shell_suites.rs"]
mod issue_1707_claim_check_shell_suites;
#[path = "issue_1740_checks_tool.rs"]
mod issue_1740_checks_tool;
#[path = "issue_1740_diff_limits.rs"]
mod issue_1740_diff_limits;
#[path = "issue_1740_diff_limits_awk.rs"]
mod issue_1740_diff_limits_awk;
#[path = "issue_1740_diff_limits_guards.rs"]
mod issue_1740_diff_limits_guards;
#[path = "issue_1740_diff_limits_shared_config.rs"]
mod issue_1740_diff_limits_shared_config;
#[path = "issue_1740_quick_change_recipe.rs"]
mod issue_1740_quick_change_recipe;
#[path = "issue_1740_quick_change_steps.rs"]
mod issue_1740_quick_change_steps;
#[path = "issue_1740_routing_recipes.rs"]
mod issue_1740_routing_recipes;
#[path = "issue_1740_select_workflow.rs"]
mod issue_1740_select_workflow;
#[path = "issue_1740_state_gates.rs"]
mod issue_1740_state_gates;
#[path = "issue_1741_fixture.rs"]
mod issue_1741_fixture;
#[path = "issue_1741_recipe_recovery.rs"]
mod issue_1741_recipe_recovery;
#[path = "issue_1741_recovery_admission.rs"]
mod issue_1741_recovery_admission;
#[path = "issue_1741_recovery_compatibility.rs"]
mod issue_1741_recovery_compatibility;
#[path = "issue_1741_recovery_security.rs"]
mod issue_1741_recovery_security;
#[path = "issue_1741_runner_contract.rs"]
mod issue_1741_runner_contract;
#[path = "issue_1743_autodrive_budget.rs"]
mod issue_1743_autodrive_budget;
#[path = "issue_1745_home_statusline.rs"]
mod issue_1745_home_statusline;
#[path = "issue_1746_daily_archive_rotator.rs"]
mod issue_1746_daily_archive_rotator;
#[path = "issue_1746_sessions_archive.rs"]
mod issue_1746_sessions_archive;
#[path = "issue_1746_sessions_live.rs"]
mod issue_1746_sessions_live;
#[path = "issue_1746_sessions_lock_restore.rs"]
mod issue_1746_sessions_lock_restore;
#[path = "issue_1760_agent_markers.rs"]
mod issue_1760_agent_markers;
#[path = "issue_1760_agent_profile_resolved.rs"]
mod issue_1760_agent_profile_resolved;
#[path = "issue_1760_agent_register_discover.rs"]
mod issue_1760_agent_register_discover;
#[path = "issue_1760_agent_register_env.rs"]
mod issue_1760_agent_register_env;
#[path = "issue_1760_agent_register_pane.rs"]
mod issue_1760_agent_register_pane;
#[path = "issue_1760_agent_registry.rs"]
mod issue_1760_agent_registry;
#[path = "issue_1760_agent_registry_safety.rs"]
mod issue_1760_agent_registry_safety;
#[path = "issue_1760_agent_source_scan.rs"]
mod issue_1760_agent_source_scan;
#[path = "issue_1760_agent_supervise.rs"]
mod issue_1760_agent_supervise;
#[path = "issue_1760_agent_supervise_checks.rs"]
mod issue_1760_agent_supervise_checks;
#[path = "issue_1760_agent_supervise_dupes.rs"]
mod issue_1760_agent_supervise_dupes;
#[path = "issue_1760_agent_supervise_limits.rs"]
mod issue_1760_agent_supervise_limits;
#[path = "issue_1760_agent_tmux_scope.rs"]
mod issue_1760_agent_tmux_scope;
#[path = "issue_1760_agent_wrapper.rs"]
mod issue_1760_agent_wrapper;
#[path = "issue_1760_agent_wrapper_refusals.rs"]
mod issue_1760_agent_wrapper_refusals;
#[path = "issue_1760_fake_proc.rs"]
mod issue_1760_fake_proc;
#[path = "issue_1760_fake_systemd_run.rs"]
mod issue_1760_fake_systemd_run;
#[path = "issue_1760_fakes.rs"]
mod issue_1760_fakes;
#[path = "issue_1760_fixture.rs"]
mod issue_1760_fixture;
#[path = "issue_1760_fixture_files.rs"]
mod issue_1760_fixture_files;
#[path = "issue_1794_wait.rs"]
mod issue_1794_wait;
#[path = "issue_1797_capacity_summary.rs"]
mod issue_1797_capacity_summary;
#[path = "issue_1801_capacity_wait.rs"]
mod issue_1801_capacity_wait;
#[path = "issue_1801_evaluator_retry.rs"]
mod issue_1801_evaluator_retry;
#[path = "issue_1802_auto_drive_light_eligibility.rs"]
mod issue_1802_auto_drive_light_eligibility;
#[path = "issue_1802_auto_drive_light_recipe.rs"]
mod issue_1802_auto_drive_light_recipe;
#[path = "issue_1802_auto_drive_light_round.rs"]
mod issue_1802_auto_drive_light_round;
#[path = "issue_1803_rotation_owner.rs"]
mod issue_1803_rotation_owner;
#[path = "issue_1808_run_cleanup.rs"]
mod issue_1808_run_cleanup;
#[path = "issue_1811_fail_open.rs"]
mod issue_1811_fail_open;
#[path = "issue_1811_fixture.rs"]
mod issue_1811_fixture;
#[path = "issue_1811_governor.rs"]
mod issue_1811_governor;
#[path = "issue_1811_limits.rs"]
mod issue_1811_limits;
#[path = "issue_1811_nesting.rs"]
mod issue_1811_nesting;
#[path = "issue_1811_status.rs"]
mod issue_1811_status;
#[path = "issue_1811_switch.rs"]
mod issue_1811_switch;
#[path = "issue_1818_no_session_limit.rs"]
mod issue_1818_no_session_limit;
#[path = "issue_1846_exhausted_reading.rs"]
mod issue_1846_exhausted_reading;
#[path = "issue_1870_reread_probe_mode.rs"]
mod issue_1870_reread_probe_mode;
#[path = "issue_1954_first_install_restager.rs"]
mod issue_1954_first_install_restager;
#[path = "issue_1954_install_leaves_live_runs_alone.rs"]
mod issue_1954_install_leaves_live_runs_alone;
#[path = "issue_1954_no_foreign_signals.rs"]
mod issue_1954_no_foreign_signals;
#[path = "issue_1955_install_cli.rs"]
mod issue_1955_install_cli;
#[path = "issue_1973_rotate_fixture.rs"]
mod issue_1973_rotate_fixture;
#[path = "issue_1973_wait_helpers.rs"]
mod issue_1973_wait_helpers;
#[path = "issue_1979_crusty_minor_followups.rs"]
mod issue_1979_crusty_minor_followups;
#[path = "issue_1985_codex_agent_step_flags.rs"]
mod issue_1985_codex_agent_step_flags;
#[path = "issue_2001_codex_usage.rs"]
mod issue_2001_codex_usage;
#[path = "issue_2010_codex_exec_notice.rs"]
mod issue_2010_codex_exec_notice;
#[path = "issue_2022_far_reset_wait.rs"]
mod issue_2022_far_reset_wait;
#[path = "issue_2022_no_default_time_limits.rs"]
mod issue_2022_no_default_time_limits;
#[path = "issue_2042_fake_proc_seam.rs"]
mod issue_2042_fake_proc_seam;
#[path = "issue_2042_host_proc_strace.rs"]
mod issue_2042_host_proc_strace;
#[path = "issue_2042_proc_root_guard.rs"]
mod issue_2042_proc_root_guard;
#[path = "issue_2042_seam_dry_run_and_refusal.rs"]
mod issue_2042_seam_dry_run_and_refusal;
#[path = "issue_2317_codex_step_commits_in_worktree.rs"]
mod issue_2317_codex_step_commits_in_worktree;
#[path = "issue_439_no_per_step_timeout.rs"]
mod issue_439_no_per_step_timeout;
#[path = "issue_507_routing_question_passthrough.rs"]
mod issue_507_routing_question_passthrough;
#[path = "issue_585_copilot_npm_hang.rs"]
mod issue_585_copilot_npm_hang;
#[path = "issue_634_asset_resolution_parity.rs"]
mod issue_634_asset_resolution_parity;
#[path = "issue_646_quality_audit_cycle_bugs.rs"]
mod issue_646_quality_audit_cycle_bugs;
#[path = "issue_672_workflow_reliability_contracts.rs"]
mod issue_672_workflow_reliability_contracts;
#[path = "issue_754_scoped_monitor_contracts.rs"]
mod issue_754_scoped_monitor_contracts;
#[path = "issue_817_runtime_artifact_resolution.rs"]
mod issue_817_runtime_artifact_resolution;
#[path = "issue_820_merge_validations_mixed_output.rs"]
mod issue_820_merge_validations_mixed_output;
#[path = "issue_833_merge_validations_json_tolerance.rs"]
mod issue_833_merge_validations_json_tolerance;
#[path = "issue_860_pr_guide_present.rs"]
mod issue_860_pr_guide_present;
#[path = "issue_899_merge_validations_abstention.rs"]
mod issue_899_merge_validations_abstention;
#[path = "no_capacity_notice_statusline.rs"]
mod no_capacity_notice_statusline;
#[path = "pr_712_named_asset_canonicalization.rs"]
mod pr_712_named_asset_canonicalization;
#[path = "profile_issue_1804.rs"]
mod profile_issue_1804;
#[path = "profile_overflow.rs"]
mod profile_overflow;
#[path = "profile_platform.rs"]
mod profile_platform;
#[path = "profile_rotate_issue_1886.rs"]
mod profile_rotate_issue_1886;
#[path = "profile_usage_direct.rs"]
mod profile_usage_direct;
#[path = "remote_cli_contract.rs"]
mod remote_cli_contract;
#[path = "runner_probe_cleanup.rs"]
mod runner_probe_cleanup;
#[path = "runner_probe_lifecycle.rs"]
mod runner_probe_lifecycle;
#[path = "runner_validation_contract.rs"]
mod runner_validation_contract;
#[path = "signal_azlin_lister.rs"]
mod signal_azlin_lister;
#[path = "signal_chat_it.rs"]
mod signal_chat_it;
#[path = "signal_config_writer.rs"]
mod signal_config_writer;
#[path = "signal_daemon_strategy.rs"]
mod signal_daemon_strategy;
#[path = "signal_distribute_state.rs"]
mod signal_distribute_state;
#[path = "signal_endpoint_precedence.rs"]
mod signal_endpoint_precedence;
#[path = "signal_endpoint_validator_parity.rs"]
mod signal_endpoint_validator_parity;
#[path = "signal_error_taxonomy.rs"]
mod signal_error_taxonomy;
#[path = "signal_identity_mode.rs"]
mod signal_identity_mode;
#[path = "signal_render.rs"]
mod signal_render;
#[path = "signal_seams_injection.rs"]
mod signal_seams_injection;
#[path = "signal_security.rs"]
mod signal_security;
#[path = "signal_setup_idempotency.rs"]
mod signal_setup_idempotency;
#[path = "signal_validator_parity.rs"]
mod signal_validator_parity;
#[path = "spec_conformance.rs"]
mod spec_conformance;
#[path = "system_prompt_append_fragment.rs"]
mod system_prompt_append_fragment;

#[path = "issue_2297_codex_installation.rs"]
mod issue_2297_codex_installation;
