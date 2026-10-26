//! Single integration-test binary for the `amplihack` package.
//!
//! Every file in this directory that belongs to the `amplihack` package used to
//! be its own `[[test]]` target in `bins/amplihack/Cargo.toml` (three others,
//! `hook_dispatch_test.rs`, `hook_aliases_test.rs` and `lbug_ffi_test.rs`, belong
//! to `amplihack-hooks` and `amplihack-cli`), so Cargo compiled and linked ~74 separate
//! test executables (none of which sccache can cache: they are bin crate
//! types). They are now modules of this one crate root, wired as the
//! single `[[test]] name = "integration"` target. Each module keeps the
//! name its old target had, so a former
//! `cargo test -p amplihack --test <name>` is now
//! `cargo test -p amplihack --test integration <name>::`.
//!
//! To add a new integration test file: create `<name>.rs` here and add a
//! `#[path = "<name>.rs"] mod <name>;` line below. Do NOT add a new
//! `[[test]]` entry to the manifest.

#[path = "amplihack_path.rs"]
mod amplihack_path;
#[path = "investigation_workflow_decomposition_test.rs"]
mod investigation_workflow_decomposition;
#[path = "../support/isolated_amplihack.rs"]
mod isolated_amplihack;
#[path = "isolated_amplihack_scan.rs"]
mod isolated_amplihack_scan;

#[path = "smart_orchestrator_decomposition_test.rs"]
mod smart_orchestrator_decomposition;

#[path = "issue_1025_pr_green_shortcircuit_test.rs"]
mod issue_1025_pr_green_shortcircuit;

#[path = "automated_action_failure_investigation_contract_test.rs"]
mod automated_action_failure_investigation_contract;

#[path = "cli_launch_test.rs"]
mod cli_launch;

#[path = "cargo_lock_cxx_consistency_test.rs"]
mod cargo_lock_cxx_consistency;

// WS1: CI cxx-build pin assertion structural tests
#[path = "ci_cxx_build_pin_test.rs"]
mod ci_cxx_build_pin;

// Issue #744: CI speed-up + robustness contract (toolchain pin, nextest +
// doctests, CARGO_PROFILE_TEST_DEBUG=0, SHA-pinned free-disk-space, cache
// save-if main-only). Structural — reads ci.yml / rust-toolchain.toml only.
#[path = "ci_speedup_optimization_test.rs"]
mod ci_speedup_optimization;

// Signal shipping contract: release/snapshot/required CI build jobs must compile
// the shipped hook binary with the Signal feature enabled.
#[path = "signal_release_feature_build_test.rs"]
mod signal_release_feature_build;

// Issue #744: integration-test binary-path robustness contract. Fragile tests
// must resolve the binary via env!(CARGO_BIN_EXE_*) instead of hand-rolled
// target/debug paths, and the orphan skip-update-check test must be wired.
#[path = "test_harness_binary_path_contract_test.rs"]
mod test_harness_binary_path_contract;

// Issue #744: previously an orphan file (never compiled/run in CI). Wiring it
// as a [[test]] target both runs its assertions and makes CARGO_BIN_EXE_amplihack
// resolve for it, so it can locate the binary robustly.
#[path = "skip_update_check_flag_test.rs"]
mod skip_update_check_flag;

// WS3: Recipe runner E2E integration tests
// These tests verify the full recipe delegation chain:
// amplihack recipe list / validate / run --dry-run
// Tests recipe_run_dry_run_* FAIL until recipe-runner-rs is installed.
#[path = "recipe_e2e_test.rs"]
mod recipe_e2e;

// Issue #77 AC9: no-Python probe integration tests (TC-04 through TC-07).
// These tests verify that index-code, index-scip, and query-code subcommands
// work correctly in a Python-free environment.
// Tests tc04-tc07 FAIL until the probe script extension is implemented.
#[path = "no_python_probe_test.rs"]
mod no_python_probe;

// Default-workflow decomposition parity tests.
// Locks the v3 contract: composer calls 9 phase sub-recipes, every original
// step ID is preserved in order, no duplicates, every brick stays under 400
// LOC, and mandatory steps (0/14/17/18) remain present. Pure structural,
// no recipe-runner needed.
#[path = "default_workflow_decomposition_test.rs"]
mod default_workflow_decomposition;

// Issue #615: agentic work-verifier contract tests.
// Locks the behavioral contract for the step-08c pair that replaced the
// legacy six-escape-hatch bash hollow-success guard. Asserts (a) the agent
// step's type/wiring/prompt content (investigation surfaces, fast-path
// opt-outs, three verdicts, JSON-on-last-line), (b) the bash gate's
// type/wiring, (c) condition-clause preservation, and (d) executes the
// actual gate command against synthetic verdicts to lock the exit-code
// mapping (WORK_VERIFIED->0, INSUFFICIENT_EVIDENCE->0+WARN, HOLLOW_SUCCESS->1,
// empty/unparseable->0+WARN, ALLOW_NO_OP->0, sentinel->0).
#[path = "issue_615_work_verifier_test.rs"]
mod issue_615_work_verifier;

#[path = "issue_1538_context_transport.rs"]
mod issue_1538_context_transport;

#[path = "issue_1538_startup_provider.rs"]
mod issue_1538_startup_provider;

// Issue #1062: brittle-parsing findings from the default-workflow audit. Pins
// the migration of free-text agent-output scraping to the tested
// `amplihack orch helper extract-json | extract-field | normalise-verdict`
// path, plus the additive `session-tree register --json` mode and the
// structured no_merge/goal_status control signals.
#[path = "issue_1062_brittle_parsing_test.rs"]
mod issue_1062_brittle_parsing;

#[path = "workflow_prep_git_recovery_test.rs"]
mod workflow_prep_git_recovery;

#[path = "version_contract_test.rs"]
mod version_contract;

#[path = "workflow_publish_pr_metadata_test.rs"]
mod workflow_publish_pr_metadata;

#[path = "workflow_publish_resilience.rs"]
mod workflow_publish_resilience;

#[path = "issue_978_step15_rebase_base_test.rs"]
mod issue_978_step15_rebase_base;

#[path = "workflow_publish_azdo_contract_test.rs"]
mod workflow_publish_azdo_contract;

#[path = "default_workflow_terminal_state.rs"]
mod default_workflow_terminal_state;

#[path = "workflow_publish_terminal_gate.rs"]
mod workflow_publish_terminal_gate;

#[path = "workflow_finalize_terminal_state.rs"]
mod workflow_finalize_terminal_state;

#[path = "workflow_finalize_resilience.rs"]
mod workflow_finalize_resilience;

#[path = "issue_808_finalization_cleanup.rs"]
mod issue_808_finalization_cleanup;

#[path = "hygiene_cleanup.rs"]
mod hygiene_cleanup;

// Issue #755: Artifact Guard CLI, workflow, and pre-commit contracts.
#[path = "artifact_guard_cli_tests.rs"]
mod artifact_guard_cli;

#[path = "artifact_guard_workflow_tests.rs"]
mod artifact_guard_workflow;

#[path = "pre_commit_artifact_guard_tests.rs"]
mod pre_commit_artifact_guard;

// Issue #1528: auto-drive phases 2-3 commit only inside the PR's linked
// worktree. PR-worktree resolver behaviour, loop step-02 and round step-04
// contracts.
#[path = "autodrive_pr_worktree_tests.rs"]
mod autodrive_pr_worktree;

#[path = "doctor_node_remediation_test.rs"]
mod doctor_node_remediation;

// PR #1530: brick-budget hook reads its limit with POSIX sed (macOS/BSD).
#[path = "brick_budget_hook_portability_test.rs"]
mod brick_budget_hook_portability;

// PR #1530: Linux-only auto-switch test helpers allow dead_code on macOS.
#[path = "linux_only_test_helpers_test.rs"]
mod linux_only_test_helpers;

// Consensus-workflow decomposition parity tests.
// Locks the v3 contract: composer calls 11 phase sub-recipes, all 60 step IDs
// preserved in order, brick budget enforced, and 7 mandatory consensus gates
// (requirements/design debates, N-version vote, refactor/PR/philosophy/final
// panels) remain present.
#[path = "consensus_workflow_decomposition_test.rs"]
mod consensus_workflow_decomposition;

// Issue #78 / v0.5.0: Fleet local dashboard probe tests (TC-09 through TC-12).
// These tests validate the local session management dashboard:
// TC-09: fleet --help exits 0
// TC-10: no crash on empty locks directory
// TC-11: FleetDashboardSummary JSON schema roundtrip
// TC-12: run_fleet_dashboard(None) completes inline without spawning threads
// Tests TC-10 and TC-12 FAIL until fleet_local implementation is complete.
#[path = "fleet_probe.rs"]
mod fleet_probe;

// Issue #77 pre-merge security hygiene tests (TC-SEC-01 through TC-SEC-24).
// These TDD tests define the security contract for the fleet module and drive
// the four PENDING pre-merge checklist items:
// TC-SEC-01:    Cargo.lock present for `cargo audit`
// TC-SEC-02–04: No `Command::new("sh")` / `sh -c` patterns
// TC-SEC-05–07: tempfile::persist() produces 0o600 files (umask-independent)
// TC-SEC-08–14: CLI session-ID and VM-name args validated against allowlist
// TC-SEC-15–19: shell_single_quote escaping contract
// TC-SEC-20:    No hardcoded high-entropy secrets in source
// TC-SEC-21–22: Error messages do not expose absolute filesystem paths
// TC-SEC-23:    No wildcard version specifiers in Cargo.toml
// TC-SEC-24:    kill(PID) built from numeric type, not user string
//
// Tests TC-SEC-07 and TC-SEC-08–14 FAIL until the corresponding implementation
// work (persist-site annotations, CLI-level input validation) is complete.
#[path = "security_hygiene_test.rs"]
mod security_hygiene;

#[path = "kuzu_path_notice_test.rs"]
mod kuzu_path_notice;

#[path = "cli_golden_tests.rs"]
mod cli_golden;

// Issue #342: Targeting existing PR branch via context (existing_branch / pr_number).
// Verifies step-04-setup-worktree (default-workflow.yaml) and step3-setup-worktree
// (consensus-workflow.yaml) skip branch creation when an existing branch is supplied,
// resolve pr_number via `gh`, validate inputs against shell/path injection, and
// preserve byte-identical legacy behaviour when both vars are empty.
// Issue #453: skip_pre_agent_validation context variable TDD tests.
// Verifies the context variable is declared in smart-orchestrator.yaml and
// default-workflow.yaml with string "true" default, forwarded through
// smart-execute-routing.yaml, and guarded in workflow-prep.yaml step-01.
#[path = "skip_pre_agent_validation_context_test.rs"]
mod skip_pre_agent_validation_context;

#[path = "existing_branch_context_test.rs"]
mod existing_branch_context;

// Issue #858: recipe task worktrees must always be created from a fresh remote
// base ref on a dedicated branch, never inheriting the caller checkout's dirty
// branch / local commits / uncommitted files.
#[path = "clean_worktree_baseline_test.rs"]
mod clean_worktree_baseline;

// Issue #858: workflow-worktree caller-checkout reuse refusal.
// Verifies step-04-setup-worktree refuses fail-closed (exit 1, zero stdout,
// stderr cites #858) when the target branch resolves to the caller's own
// checkout (Gates A/B/C) and that dirty caller state never leaks into a new
// task worktree, while legitimate separate-worktree reuse is preserved.
#[path = "workflow_worktree_caller_refusal_test.rs"]
mod workflow_worktree_caller_refusal;

// Issue #259: Documentation-vs-code drift detection tests.
// Verify docs match source code for flag injection, completions,
// memory index output format, and all new reference docs.
#[path = "doc_code_drift_test.rs"]
mod doc_code_drift;

// Issue #420 Doc Parity Batch E: hive-mind documentation port.
// Verifies the four ported docs (tutorials/getting-started, tutorials/tutorial,
// concepts/design, concepts/eval) exist at kebab-case Diataxis paths, are
// linked from docs/index.md, reference only real amplihack-hive symbols
// (no fabricated APIs), use `amplihack hive` CLI calls (not Python), and
// reference the `lbug` graph backend (not `kuzu`).
#[path = "hive_mind_doc_parity_test.rs"]
mod hive_mind_doc_parity;

// Issue #248: Native `amplihack orch run <WS_FILE>` regression test.
// Verifies the subcommand is wired into clap and short-circuits cleanly on an
// empty workstreams JSON array (matches multitask::run_multitask semantics).
#[path = "orch_run_cli_test.rs"]
mod orch_run_cli;

// sdk-comparison.yaml graceful skip when the native SDK eval runner is unavailable.
#[path = "sdk_comparison_optional_python_test.rs"]
mod sdk_comparison_optional_python;

// self-improvement-loop.yaml graceful-skip contract for native eval runner absence.
#[path = "self_improvement_loop_skip_test.rs"]
mod self_improvement_loop_skip;

// Issue #435: docs/discoveries.md rolling log.
// Verifies the file exists, follows the prescribed format (title, archive
// policy, entry format guide, category tags, dated seed entry, ToC), and
// is linked from docs/index.md under Contributing.
#[path = "discoveries_doc_test.rs"]
mod discoveries_doc;

// Issue #434: Gherkin v2 experiment disposition investigation.
// Verifies investigation doc structure, index.md linkage, and regression guards
// ensuring gherkin-expert skill/agent/allowlist entry remain present.
#[path = "gherkin_investigation_doc_test.rs"]
mod gherkin_investigation_doc;

// Issue #592: Skill frontmatter name validation.
// Ensures all SKILL.md `name:` fields match ^[a-z0-9-]+$ (no colons).
// Regression guard for amplihack:migrate → amplihack-migrate fix.
#[path = "skill_frontmatter_name_test.rs"]
mod skill_frontmatter_name;

// Issue #890: Skill frontmatter TYPE validation.
// Ensures every SKILL.md encodes name/description/argument-hint as string
// scalars (not YAML lists/maps). Regression guard for the Copilot CLI
// "argument-hint must be a string" failure (merge-ready fix, commit e0abfb4).
#[path = "skill_frontmatter_type_test.rs"]
mod skill_frontmatter_type;

#[path = "npe_hunting_skill_test.rs"]
mod npe_hunting_skill;

// QA-022: Formal specification for multitask orchestrator refactoring.
#[path = "multitask_orchestrator_spec_test.rs"]
mod multitask_orchestrator_spec;

// Issue #799: deterministic metadata-only workflow-log inventory helper.
#[path = "orch_workflow_log_inventory_test.rs"]
mod orch_workflow_log_inventory;

// Issue #891: smart-orchestrator active execution must not require orch_helper.py.
#[path = "active_recipe_guard.rs"]
mod active_recipe_guard;

// Issue #801: generated workflow requirements must omit saved preference notices.
#[path = "workflow_prep_requirements_sanitizer_test.rs"]
mod workflow_prep_requirements_sanitizer;

// Issues #599–#608: Repo-sweep regression tests.
// Guards against re-introduction of stale refs, broken links, missing feature gates.
#[path = "repo_sweep_issues_599_608_test.rs"]
mod repo_sweep_issues_599_608;

// Issue #605: GitHubDistributor TDD tests.
// Defines the contract for the fixed GitHubDistributor implementation.
// Tests FAIL against the feature-gated stubs and PASS after applying the
// implementation (UTF-8 truncation, base64 crate, temp file push, SHA
// idempotency, visibility parameter).
#[path = "github_distributor_tdd_test.rs"]
mod github_distributor_tdd;

// Issues #655/#656: SKILL.md stale Python instructions + git fetch resilience.
// #655 tests PASS (SKILL.md already fixed). #656 tests FAIL (TDD red) until
// workflow-prep.yaml step-01 breaks git fetch out of the &&-chain.
#[path = "issue_655_656_skill_fetch_resilience_test.rs"]
mod issue_655_656_skill_fetch_resilience;

#[path = "p1_workflow_reliability_test.rs"]
mod p1_workflow_reliability;

// Issue #661: Scan SKILL.md files for stale Python recipe-runner patterns.
// Prevents re-introduction of run_recipe_by_name, python3 -c recipe, etc.
#[path = "no_stale_python_patterns_test.rs"]
mod no_stale_python_patterns;

// Issue #746: Default workflow guidance must point to skills/recipes, not
// stale DEFAULT_WORKFLOW.md or UltraThink-era canonical instructions.
#[path = "issue_746_default_workflow_guidance_test.rs"]
mod issue_746_default_workflow_guidance;

// Issue #684: Host-aware workflow steps (commit refs, PR body, summary, percent-decode).
// TDD-RED tests — fail until implementation lands across 4 recipe files.
#[path = "issue_684_host_aware_workflow_test.rs"]
mod issue_684_host_aware_workflow;

// Issue #1595: step-03 tracking-issue selection on GitHub (explicit
// issue_number wins; task-text #N reused only when it is an open issue).
#[path = "issue_1595_tracking_issue_selection_test.rs"]
mod issue_1595_tracking_issue_selection;

// Issues #815/#804: default-workflow local tracking issue-number extraction.
// step-03b must accept hash-based `local-<hash>` references (propagate them),
// never surface a bare number for a local fallback, and never abort the workflow.
#[path = "issue_815_804_local_tracking_extract_test.rs"]
mod issue_815_804_local_tracking_extract;

// Issues #815/#804: downstream PR-scope consumers (pr_ready, final_status) must
// coerce a non-numeric local tracking issue_number to empty before scope matching.
#[path = "issue_815_804_local_tracking_scope_args_test.rs"]
mod issue_815_804_local_tracking_scope_args;

// Issue #1017: PR-scope must match on the (head,base,same-repo) primary key so
// the recipe adopts its own unique-by-branch PR (stale --issue/--head-sha), and
// publish must be collision-tolerant (adopt an existing OPEN PR on create fail).
#[path = "pr_scope_primary_key_test.rs"]
mod pr_scope_primary_key;

// Issue #778: merge-ready must be platform-neutral, with explicit GitHub and
// Azure DevOps/AzDO guidance for every merge-readiness aspect.
#[path = "merge_ready_platform_contract_test.rs"]
mod merge_ready_platform_contract;

// code-philosophy-audit recipe structural parity tests.
#[path = "code_philosophy_audit_recipe_test.rs"]
mod code_philosophy_audit_recipe;

// Issue #849: default-workflow SKILL mirror parity + F-ERR-2 stale-citation guard.
// Pins byte-for-byte mirror equality, the extracted Step 17a gate node, absence of
// the stale `default-workflow.yaml:961` reference, and that the audit citations
// resolve to a live `step-17a-testing-evidence-gate` step in workflow-pr-review.yaml.
#[path = "issue_849_skill_mirror_citation_test.rs"]
mod issue_849_skill_mirror_citation;

// Phase-1 of #4610: verus-expert formal-methods skill contract tests. Pins the
// 4-file layout (bundle skill, agent, docs mirror, durable assessment), skill/
// agent frontmatter + agent-id resolution, mandated sections/keywords, evidence-
// grounded Verus vocabulary, primary-source citations, durability (no dated
// framing), the no-Bridge/no-kuzu/no-Python guards, ASCII cleanliness, and the
// mkdocs nav + known-agents registration so the skill loads.
#[path = "verus_expert_skill_test.rs"]
mod verus_expert_skill;

// PR #1027 (spike/ci-wallclock-parallelize): CI wall-clock parallelization +
// conditional release-profile contract. Pins the 7 required check names, the
// removal of `needs: check` from test/install-smoke/cross-compile, the release
// fan-in, install-smoke's unconditional LTO-off, cross-compile's tag-keyed LTO,
// release.yml having no profile overrides, and [profile.release] full LTO.
// Structural — reads ci.yml / release.yml / Cargo.toml only.
#[path = "ci_wallclock_parallelize_test.rs"]
mod ci_wallclock_parallelize;

// Issue #1337: reusable agentic loop-health evaluation contract.
// Pins the loop-health-evaluator brick (three-outcome CONTINUE/DONE/STUCK
// verdict, evidence collection, exit-79 terminal handling, no numeric
// iteration cap, no per-step timeout), the `orch helper
// normalise-loop-verdict` fail-safe-to-STUCK helper, and runs the executable
// STUCK / malformed-verdict contract test.
#[path = "issue_1337_loop_health_evaluator_test.rs"]
mod issue_1337_loop_health_evaluator;

// Issue #1671: a rejected git hook in a checkpoint or publish step goes to a
// fixing agent and the step retries until the loop-health evaluator ends the
// loop. Pins the fixing recipe, its manifest entry, the brick budget, the
// reference page links, and runs the executable shell contract.
#[path = "issue_1671_hook_rejection_repair_test.rs"]
mod issue_1671_hook_rejection_repair;

// Issue #1510: the loop-evidence collector drops agent output lines
// (`[HH:MM:SS] [amplihack:...]`) before its BLOCKED_TERMINAL and exit-79
// detectors, the same rule as terminal_refusal() in autodrive_loop.sh, so a
// review quoting the token is not a terminal policy refusal. Runs the real
// step-01 command through bash; needs bash and git, not the amplihack binary.
#[path = "issue_1510_collector_agent_prose_test.rs"]
mod issue_1510_collector_agent_prose;

// auto-drive-to-merge: wrap default-workflow and drive the PR to merge through
// two agentically-terminated loops (crusty, then merge-ready). Pins the absence
// of any numeric iteration cap, the fail-safe-to-blocking verdict defaults, the
// two forbidden-flag prohibitions, the evidence-gated merge, exit-79 terminality
// and the resume contract, and runs the executable STUCK / malformed-verdict
// contract test. Depends at RUNTIME on loop-health-evaluator (PR #1347); the
// tests themselves need only extract-json / extract-field, already on main.
#[path = "auto_drive_to_merge_test.rs"]
mod auto_drive_to_merge;

// Issue #1641: a resumed run whose branch already has an open, same-repo PR
// with green checks (or pending/no CI plus this run's typed precommit evidence)
// finalizes as EXISTING_OPEN_PR instead of FAILED_IMPLEMENTATION; a failing
// check is BLOCKED_CI, and every fail-closed case (no PR, no verification
// signal, hollow success, blockers) is kept. Runs the
// real validator against a private git repo and a stub gh.
#[path = "issue_1641_finalization_open_pr.rs"]
mod issue_1641_finalization_open_pr;
#[path = "issue_1642_github_origin_parsing.rs"]
mod issue_1642_github_origin_parsing;
