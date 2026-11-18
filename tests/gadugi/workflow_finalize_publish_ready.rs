//! CI-only qa-team driver. Compile with rustc; no Cargo dependencies required.
//! Runs the same behavioral contracts as integration tests, plus the affected
//! recipe commands through the built binary and installed pinned runner.

#[path = "../integration/workflow_finalize_publish_ready.rs"]
mod contracts;
use contracts::fixture::{Fixture, PUBLISH, READY, success};
use std::fs;
use std::path::{Path, PathBuf};

fn recipe_through_binary(source: &Path, binary: &Path, runner: &Path) {
    let f = Fixture::new(source, binary);
    f.audit(false);
    let commands = [PUBLISH, READY].map(|id| f.step_body(id));
    // Load each production command as data, rather than copying its shell body.
    // The contract suite also executes these commands in their shipped order.
    // CLI recipe defaults are transported as strings. Keep the worktree JSON
    // as a string too: a mapping default is serialized as YAML, which the
    // publication helper correctly rejects as non-JSON.
    let doc = success(f.command("jq", &f.main).args(["-cn", "--arg", "publish"])
        .arg(&commands[0]).args(["--arg", "ready"]).arg(&commands[1])
        .args(["--arg", "wt"]).arg(f.worktree_json())
        .args(["--arg", "repo"]).arg(&f.main)
        .arg(r#"{name:"issue-2787-finalize",description:"Publication before readiness",version:"1.0.0",
            context:{repo_path:$repo,worktree_setup:$wt,base_branch:"main",
                pr_number:"7",pr_url:"https://github.com/acme/widget/pull/7",issue_number:"2787",step_recovery_dir:""},
            steps:[{id:"step-20b-push-cleanup",type:"bash",command:$publish},
                   {id:"step-21-pr-ready",type:"bash",command:$ready}]}"#)
        .output().unwrap());
    let recipe = f.main.join("issue-2787.yaml");
    fs::write(&recipe, doc).unwrap();
    success(
        f.command(binary, &f.main)
            .args(["recipe", "run"])
            .arg(&recipe)
            .env("RECIPE_RUNNER_RS_PATH", runner)
            .output()
            .unwrap(),
    );
    f.assert_ready();
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    assert_eq!(
        args.len(),
        3,
        "usage: driver <repository> <built-amplihack> <pinned-recipe-runner>"
    );
    let source = PathBuf::from(&args[0]).canonicalize().expect("repository");
    let binary = PathBuf::from(&args[1])
        .canonicalize()
        .expect("built amplihack prerequisite");
    let runner = PathBuf::from(&args[2])
        .canonicalize()
        .expect("pinned runner prerequisite");
    let cases: &[(&str, fn(&contracts::fixture::Fixture))] = &[
        (
            "worktree_resolution_ignores_ambient_git_overrides",
            contracts::worktree_resolution_ignores_ambient_git_overrides,
        ),
        (
            "publication_and_readiness_ignore_ambient_git_overrides",
            contracts::publication_and_readiness_ignore_ambient_git_overrides,
        ),
        (
            "publication_handoff_continues_without_readying_audit_edits",
            contracts::publication_handoff_continues_without_readying_audit_edits,
        ),
        (
            "finalization_evidence_retains_operation_outcomes",
            contracts::finalization_evidence_retains_operation_outcomes,
        ),
        (
            "unresolved_operations_reach_completion",
            contracts::unresolved_operations_reach_completion,
        ),
        (
            "recovery_summary_continues_with_unresolved_operations",
            contracts::recovery_summary_continues_with_unresolved_operations,
        ),
        (
            "publication_resolves_installed_runtime_helper",
            contracts::publication_resolves_installed_runtime_helper,
        ),
        (
            "publication_missing_runtime_helper_prevents_push",
            contracts::publication_missing_runtime_helper_prevents_push,
        ),
        (
            "discovery_secondary_mismatches_enter_recovery",
            contracts::discovery_secondary_mismatches_enter_recovery,
        ),
        (
            "noncanonical_discovery_retains_recovery_report",
            contracts::noncanonical_discovery_retains_recovery_report,
        ),
        (
            "final_read_mismatches_enter_recovery",
            contracts::final_read_mismatches_enter_recovery,
        ),
        (
            "hostile_final_metadata_is_protected",
            contracts::hostile_final_metadata_is_protected,
        ),
        (
            "valid_discovery_only_readiness_succeeds",
            contracts::valid_discovery_only_readiness_succeeds,
        ),
        ("unpushed_commit", contracts::unpushed_commit),
        ("uncommitted_audit", contracts::uncommitted_audit),
        (
            "main_checkout_uses_pr_worktree",
            contracts::main_checkout_uses_pr_worktree,
        ),
        (
            "alias_publishes_remote_branch",
            contracts::alias_publishes_remote_branch,
        ),
        (
            "persistent_ancestor_enters_recovery",
            contracts::persistent_ancestor_enters_recovery,
        ),
        (
            "missing_worktree_reaches_publication_recovery",
            contracts::missing_worktree_reaches_publication_recovery,
        ),
        (
            "publication_recovery_reexecutes_after_local_repair",
            contracts::publication_recovery_reexecutes_after_local_repair,
        ),
        (
            "diagnostics_report_all_exclusions",
            contracts::diagnostics_report_all_exclusions,
        ),
        (
            "diagnostics_preserve_missing_observations",
            contracts::diagnostics_preserve_missing_observations,
        ),
        (
            "diagnostics_keep_each_candidates_exclusions",
            contracts::diagnostics_keep_each_candidates_exclusions,
        ),
        (
            "temporary_captures_removed_but_reports_persist",
            contracts::temporary_captures_removed_but_reports_persist,
        ),
        (
            "recipe_publication_precedes_readiness",
            contracts::recipe_publication_precedes_readiness,
        ),
        (
            "invalid_worktree_never_falls_back",
            contracts::invalid_worktree_never_falls_back,
        ),
        (
            "rejected_push_is_not_success",
            contracts::rejected_push_is_not_success,
        ),
        (
            "divergence_preserves_history",
            contracts::divergence_preserves_history,
        ),
        (
            "remote_fast_forward_keeps_audit",
            contracts::remote_fast_forward_keeps_audit,
        ),
        (
            "identity_and_secondary_filters_still_apply",
            contracts::identity_and_secondary_filters_still_apply,
        ),
        (
            "empty_lookup_is_failure",
            contracts::empty_lookup_is_failure,
        ),
        (
            "hostile_candidate_text_is_data",
            contracts::hostile_candidate_text_is_data,
        ),
    ];
    let mut failures = 0;
    for (name, case) in cases {
        let result = std::panic::catch_unwind(|| {
            let f = contracts::fixture::Fixture::new(&source, &binary);
            case(&f);
        });
        if result.is_ok() {
            println!("PASS: {name}");
        } else {
            eprintln!("FAIL: {name}");
            failures += 1;
        }
    }
    if std::panic::catch_unwind(|| recipe_through_binary(&source, &binary, &runner)).is_ok() {
        println!("PASS: recipe_through_built_binary");
    } else {
        failures += 1;
    }
    assert_eq!(failures, 0, "SCENARIO_FAILED: {failures} cases failed");
    println!("ALL_CASES_PASSED");
}
