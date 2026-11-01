//! The cycle-2 cohesion boundary includes test roots and extracted helper modules.
use std::{fs, path::Path};

fn inspect(path: &Path, violations: &mut Vec<String>) {
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            inspect(&entry.unwrap().path(), violations);
        }
    } else if path
        .extension()
        .is_some_and(|extension| extension == "rs" || extension == "sh")
    {
        let lines = fs::read_to_string(path).unwrap().lines().count();
        if lines > 300 {
            violations.push(format!("{}: {lines} lines", path.display()));
        }
    }
}

#[test]
fn cycle2_new_test_targets_and_extracted_helpers_obey_300_line_boundary() {
    let cli = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();
    for relative in [
        "src/commands/install/stale_wrappers/quarantine_name_tests.rs",
        "src/commands/install/codex_plugin/path_scope.rs",
        "src/commands/install/codex_plugin/path_scope_tests.rs",
        "src/commands/install/codex_plugin/recovery_tests.rs",
        "src/commands/install/codex_plugin/backup_cleanup_tests.rs",
        "src/commands/install/codex_plugin/rollback.rs",
        "src/commands/install/codex_plugin/rollback_durability.rs",
        "../../amplifier-bundle/tools/workflow_context.sh",
        "../../amplifier-bundle/tools/workflow_enforce_verdict.sh",
        "../../amplifier-bundle/tools/workflow_doc_review_checkpoint.sh",
        "../../amplifier-bundle/tools/workflow_finalization_context.sh",
        "../../amplifier-bundle/tools/workflow_finalization_metadata.sh",
        "../../amplifier-bundle/tools/workflow_finalization_collect.sh",
        "../../amplifier-bundle/tools/workflow_finalization_validate.sh",
        "../../amplifier-bundle/tools/workflow_finalization_complete.sh",
        "../../amplifier-bundle/tools/workflow_finalization_pr_signal.sh",
        "../../amplifier-bundle/tools/workflow_finalization_handoff.sh",
        "../../amplifier-bundle/tools/workflow_finalization_open_pr.sh",
        "../../tests/integration/issue_1641_finalization_open_pr.rs",
        "../../tests/integration/issue_1714_finalization_handoff.rs",
        "src/commands/install/tests/codex_plugin_tests.rs",
        "tests/codex_frontdoor_contract.rs",
        "../amplihack-launcher/tests/codex_invocation.rs",
        "src/commands/install/tests/codex_disabled_rollback.rs",
        "src/commands/install/tests/native_registration_fixture.rs",
        "src/commands/install/codex_plugin/rollback_durability_tests.rs",
        "src/commands/install/codex_plugin/recovery_alias_tests.rs",
        "src/commands/install/tests/codex_alias_fixture.rs",
        "src/commands/install/tests/codex_alias_paths.rs",
        "../../tests/integration/issue_1538_context_transport.rs",
        "../../tests/integration/issue_1538_startup_provider.rs",
        "../../tests/integration/issue_1642_github_origin_parsing.rs",
        "src/commands/governor",
        "src/commands/governor/mod.rs",
        "src/commands/governor/model.rs",
        "src/commands/governor/budget.rs",
        "src/commands/governor/rate.rs",
        "src/commands/governor/queue.rs",
        "src/commands/governor/policy.rs",
        "src/commands/governor/config.rs",
        "src/commands/governor/identity.rs",
        "src/commands/governor/inputs.rs",
        "src/commands/governor/store.rs",
        "src/commands/governor/liveness.rs",
        "src/commands/governor/admit.rs",
        "src/commands/governor/admit_state.rs",
        "src/commands/governor/files.rs",
        "src/commands/governor/guard.rs",
        "src/commands/governor/manifest.rs",
        "src/commands/governor/status.rs",
        "src/commands/governor/status_table.rs",
        "src/commands/governor/tests_policy.rs",
        "src/commands/governor/tests_policy_flow.rs",
        "src/commands/governor/tests_budget.rs",
        "src/commands/governor/tests_rate.rs",
        "src/commands/governor/tests_queue.rs",
        "src/commands/governor/tests_identity.rs",
        "src/commands/governor/tests_config.rs",
        "src/commands/governor/tests_store.rs",
        "src/commands/governor/tests_liveness.rs",
        "src/commands/governor/tests_seed.rs",
        "src/commands/governor/tests_status.rs",
        "src/commands/governor/tests_admit.rs",
        "src/commands/governor/tests_source_scan.rs",
        "src/commands/profile/governor_view.rs",
        "src/user_config/schema_governor.rs",
        "tests/issue_1811_fail_open.rs",
        "tests/issue_1811_fixture.rs",
        "tests/issue_1811_governor.rs",
        "tests/issue_1811_status.rs",
    ] {
        let root = cli.join(relative);
        inspect(&root, &mut violations);
        let helpers = root.with_extension("");
        if helpers.is_dir() {
            inspect(&helpers, &mut violations);
        }
    }
    assert!(
        violations.is_empty(),
        "cohesive extraction required:\n{}",
        violations.join("\n")
    );
}
