---
title: Issue 1554 delivery evidence
description: Implementation checks, independent review and outstanding acceptance gates.
doc_type: reference
last_updated: 2026-11-12
---

# Issue 1554 delivery evidence

This continuation implements [issue #1554](https://github.com/rysweet/amplihack-rs/issues/1554)
on `feat/issue-1554-new-repositories-with-no`. It retains the earlier work and
does not claim acceptance or merge readiness. Subsequent PR driving belongs to
candide.

## Implemented behaviour

Checkout validation never initialises Git. Common-directory exclusions precede
checkout-local runtime writes. Launcher context, sessions and locks use external
checkout-specific state; power-steering state remains clone-shared.

Matching versioned requirements and reviewed design authorise exact source
filenames. Checkpoints use a private index, preserve protected entry bytes and
metadata, run normal hooks and validate again at the prepared reference
transaction boundary. Unsupported index metadata is refused before committing.
The independent artifact guard rejects runtime throughout the complete index
and at deletion and rename endpoints; ordinary generated-file cleanup remains
permitted.

Publication freezes explicit branch updates, obtains the actual destination's
advertisement, acquires complete history and checks every outgoing commit for
every ref before pushing. Local tracking refs and `FETCH_HEAD` cannot narrow the
range. Failed acquisition and partial publication produce failing structured
results. Recipes explicitly disable automatic staging and consume one versioned
publication result, including the push outcome used by PR creation.

## Executed checks

Checks use disposable repositories, bare local remotes, local fixture agents and
isolated profiles. Synchronisation uses process completion and events; these
controls add no timers, interval probes, timed retries or GitHub polling.
Raw logs and numeric exit results are retained outside the checkout in
`/var/tmp/issue-1554-build/` on the build host.

| Check | Recorded result |
| --- | --- |
| `cargo test --locked --offline -p amplihack-git --all-targets` | 30 passed; `last-git.log`. |
| Utils agent-resolution unit tests | 40 passed; `repair-agent-unit.log`. |
| Utils launcher-context unit tests | 5 passed; `repair-context-unit.log`. |
| Utils worktree unit tests | 3 passed; `verified-worktree-last.log`. |
| Utils artifact-guard unit tests | 80 passed; `final-guard-current.log`. |
| `issue_1554_artifact_policy` | 4 passed; `final-guard-policy-current.log`. |
| Launch without `HOME`, `XDG_CONFIG_HOME` or `.gitignore` | Passed with unchanged inventory and the original repository hook; `quick-launch.log`. |
| Git-crate all-target Clippy with `-D warnings` | Passed; `last-git-clippy.log`. |
| `tests/issue_1278_git_env_isolation.sh` | 80 assertions passed; `static-git-env.log`. |
| Shell syntax with Homebrew Bash 5 | Passed for the changed shell adapters. |
| Brick budget and sanitised Git subprocess checks | Passed. |
| CLI issue 1554 integration controls (native hold diagnostic excluded) | 74 passed; `release-cli.log`. |
| CLI session-tracker unit tests | 5 passed; `verified-sessions-last.log`. |
| CLI nesting unit tests | 11 passed; `release-nesting.log`. |
| Launcher library unit tests | 275 passed; `verified-launcher-last.log`. |
| Hook launcher-context fixtures | 11 passed; `repair-hooks.log`. |
| External resolver, security and configuration integration fixtures | 66 passed; `release-resolver.log`. |
| Launcher persistence fixtures | 28 passed; `release-launch-context.log`. |
| Launch and recipe root-sandbox fixtures | 17 passed; `release-root-sandbox.log`. |
| Memory indexing-job tests with CLI dependency unification | 4 passed; `combined-memory-index.log`. |
| Hook-library Clippy and final executable build | Passed; `repair-hook-clippy.log` and `release-binary.log`. |
| Final formatting and whitespace checks | Passed; `release-format.log` and `static-whitespace.log`. |
| All applicable configured source pre-commit hooks | Workspace Clippy stopped at the unchanged macOS `RESUME_MSG` import; all other applicable hooks passed. `release-precommit.log` records the final source run. |

Failed intermediate runs remain in the log directory. They are not presented
as successful acceptance evidence. The earlier artifact-guard unit run exposed
an ordinary cleanup regression, which the passing final guard run covers
separately from strict runtime endpoint enforcement. An intermediate CLI run
used an older executable and failed the new prepared-reference mutation control;
the rebuilt executable and final full issue filter pass it.

The final 74-control issue filter includes a normal-Git comparison for nested reference
transactions. Each reference hook gets a separate input file, so a hook's own
reference update cannot overwrite the outer transaction's input. The updated
external-context diagnostic reader is covered by 40 resolver unit tests, five
context unit tests and 66 resolver/security/configuration integration tests.
Eleven hook fixtures cover nearest-checkout discovery and external persistence.
Seventeen launch/recipe root-sandbox fixtures also pass with real isolated
checkouts. The recipe fixture resolves the macOS temporary-directory alias
before the pinned runner opens its no-follow recovery storage.
Earlier zero-match filters remain recorded and provide no test evidence.

## Independent review and scope

A separate reviewer, `/root/publication_review`, reviewed the advertisement and
every-ref rules independently of their original proposer. Review found and
verified a chained `pushInsteadOf` destination mismatch, empty and malformed
configured refspecs, implicit submodule publication and relative-source transfer
handling. The implementation addresses those findings and adds regressions.
No remaining defect in the reviewed range rules was identified; this is source
review evidence, not runner acceptance. The final local-only review confirmed
SHA-256 `3b38278169de543ca8543190232be1efd3569aea7cf7a9d1bfab3e65a80c6a29`
for `publication_validation.rs` and
`b73fd1953a515b0d47af3c3d70222b5979feb38405405fe45b37813d673f7470`
for `isolated_history.rs`.

The same review explicitly amended the design and requirements scope to 102
exact source, adapter, recipe, fixture, dependency and documentation paths.
The external version-1 records identify `rysweet/amplihack-rs#1554` and retain
their separate requirements and reviewed-design roles. A changed-file inventory
does not grant staging authority. No generic runner semantic change or runner
pin change is authorised.

## Deferred platform checks

The following Linux checks were not executed on macOS:

- CLI integration: `issue_1554_runtime_boundaries::guard_rejects_a_runtime_filename_that_apfs_cannot_represent`.
- CLI library: `commands::recipe::run::tests_parent_watch::the_parser_agrees_with_this_process_own_stat`.
- CLI library: `commands::recipe::run::tests_parent_watch::a_live_parent_with_the_matching_start_time_is_alive`.
- CLI library: `commands::recipe::run::tests_parent_watch::a_live_pid_with_another_start_time_is_gone`.
- CLI library: `commands::recipe::run::tests_parent_watch::a_reaped_pid_is_gone`.
- CLI integration: `issue_1976_orphaned_nested_run::a_nested_run_whose_parent_dies_mid_step_stops_with_exit_75`.
- CLI integration: `issue_1976_orphaned_nested_run::a_nested_run_whose_parent_is_already_dead_never_starts_the_runner`.
- CLI integration: `issue_1976_orphaned_nested_run::a_reused_parent_pid_is_treated_as_gone`.
- CLI integration: `issue_1976_orphaned_nested_run::a_run_exports_its_pid_and_start_time_to_the_runner`.
- CLI integration: `issue_1976_orphaned_nested_run::invalid_parent_values_warn_and_never_stop_the_run`.
- CLI integration: `issue_1976_real_runner_nested_run::a_step_under_the_real_runner_gets_the_parent_variables_and_its_nested_run_stops_with_75`.

The existing parent-watch and nested-run cases require Linux `/proc`, pidfds or
FIFO synchronisation. They are listed as CI follow-up, not as execution evidence
for this change. Exact identifiers from any separate original FIFO-pool packet
remain unrecovered and are part of the historical evidence gap below.

The configured workspace Clippy hook also fails on macOS for existing Linux-only
liveness helpers and imports, including the unchanged
`bins/amplihack-hooks/tests/limit_resume_binary.rs:24` import of `RESUME_MSG`.
The focused Git-crate and hook-library Clippy checks are separate from that
workspace limitation.
No hook is disabled to conceal it.

## Historical acceptance still outstanding

The original nine publication, review-feedback and cleanup combinations have
not been recovered with provenance. The reconstructed nine-case matrix and new
adversarial regressions remain diagnostics and cannot replace that packet.
Related earlier source was found at `5f3073be` and `4e0aeefb`; neither supplies
the complete original acceptance packet.

The original 25 native template-based opt-out removal controls have not passed
with a corrected, released and authorised runner. The newly derived native
diagnostic is explicitly ignored while the ownership hold applies. Static
recipe checks, materialised-directory diagnostics and historical approvals do
not satisfy native acceptance.

## Dependency hold

Runner pin `90286bc50b84ded773795d2c8301b689f680db0b` is unchanged. The
[runner #201 ownership hold](https://github.com/rysweet/amplihack-recipe-runner/issues/201)
remains in force. Required sequence: prerequisite merges, runner correction,
release, authorised pin adoption, then the original 25 native controls.

The issue comments identify archive
`20261105T1710Z-STAGING1554-RUNNER-DEPENDENCY`, SHA-256
`8d0dac802bba4bbe680c39ea94026dc35933bbb382e689bbddfd47afc32b2b41`,
held on candide. Its absence here does not lift the hold. These outstanding
gates permit an honest draft PR and prevent acceptance or merge-readiness claims.

## Recorded exact-path amendment

The independent reviewer approved this versioned amendment against the task
requirements. The paired requirements record uses the same task identity and
exact paths. This records the completed review; an editable inventory cannot
extend it or lift acceptance gates.

Requirements record SHA-256: `22df354f7e5ef642289977c6f3b6fc506cacad8456900cfb141c075dd0b89c10`.

```json
{
  "version": 1,
  "task": "rysweet/amplihack-rs#1554",
  "files_to_change": [
    "Cargo.lock",
    "amplifier-bundle/recipes/autodrive-crusty-round.yaml",
    "amplifier-bundle/recipes/autodrive-merge-evidence.yaml",
    "amplifier-bundle/recipes/autodrive-merge-round.yaml",
    "amplifier-bundle/recipes/consensus-design.yaml",
    "amplifier-bundle/recipes/consensus-issue-worktree.yaml",
    "amplifier-bundle/recipes/consensus-merge-finalize.yaml",
    "amplifier-bundle/recipes/consensus-panel-cleanup.yaml",
    "amplifier-bundle/recipes/consensus-pr-feedback.yaml",
    "amplifier-bundle/recipes/consensus-pr-review.yaml",
    "amplifier-bundle/recipes/consensus-preflight.yaml",
    "amplifier-bundle/recipes/consensus-publish.yaml",
    "amplifier-bundle/recipes/consensus-requirements-debate.yaml",
    "amplifier-bundle/recipes/consensus-tdd-impl.yaml",
    "amplifier-bundle/recipes/consensus-test-fix.yaml",
    "amplifier-bundle/recipes/hook-rejection-repair.yaml",
    "amplifier-bundle/recipes/loop-health-evaluator.yaml",
    "amplifier-bundle/recipes/workflow-design.yaml",
    "amplifier-bundle/recipes/workflow-finalize.yaml",
    "amplifier-bundle/recipes/workflow-pr-review.yaml",
    "amplifier-bundle/recipes/workflow-precommit-test.yaml",
    "amplifier-bundle/recipes/workflow-prep.yaml",
    "amplifier-bundle/recipes/workflow-publish.yaml",
    "amplifier-bundle/recipes/workflow-refactor-review.yaml",
    "amplifier-bundle/recipes/workflow-tdd.yaml",
    "amplifier-bundle/recipes/workflow-terminal-adjudication.yaml",
    "amplifier-bundle/recipes/workflow-worktree.yaml",
    "amplifier-bundle/tools/autodrive_trust.sh",
    "amplifier-bundle/tools/git-repo-env.sh",
    "amplifier-bundle/tools/workflow_checkpoint_commit.sh",
    "amplifier-bundle/tools/workflow_fetch_origin.sh",
    "amplifier-bundle/tools/workflow_hook_repair.sh",
    "amplifier-bundle/tools/workflow_pr_head_fetch.sh",
    "amplifier-bundle/tools/workflow_pr_head_sync.sh",
    "amplifier-bundle/tools/workflow_runtime_artifacts.sh",
    "amplifier-bundle/tools/workflow_stage_changes.sh",
    "crates/amplihack-cli/src/cli_subcommands.rs",
    "crates/amplihack-cli/src/commands/hygiene/artifact_guard.rs",
    "crates/amplihack-cli/src/commands/launch/mod.rs",
    "crates/amplihack-cli/src/commands/launch/tests_env.rs",
    "crates/amplihack-cli/src/commands/launch/tests_root_sandbox.rs",
    "crates/amplihack-cli/src/commands/mod.rs",
    "crates/amplihack-cli/src/commands/recipe/run/execute.rs",
    "crates/amplihack-cli/src/commands/recipe/run/tests_root_sandbox.rs",
    "crates/amplihack-cli/src/nesting/helpers.rs",
    "crates/amplihack-cli/src/nesting/mod.rs",
    "crates/amplihack-cli/src/session_tracker.rs",
    "crates/amplihack-cli/tests/issue_1554/fixture.rs",
    "crates/amplihack-cli/tests/issue_1554_checkpoint_preservation.rs",
    "crates/amplihack-cli/tests/issue_1554_launch_inventory.rs",
    "crates/amplihack-cli/tests/issue_1554_publication.rs",
    "crates/amplihack-cli/tests/issue_1554_recipe_controls.rs",
    "crates/amplihack-cli/tests/issue_1554_recipe_results.rs",
    "crates/amplihack-cli/tests/issue_1554_runtime_boundaries.rs",
    "crates/amplihack-cli/tests/main.rs",
    "crates/amplihack-cli/tests/support/quiet_process.rs",
    "crates/amplihack-git/Cargo.toml",
    "crates/amplihack-git/README.md",
    "crates/amplihack-git/src/checkpoint_transaction.rs",
    "crates/amplihack-git/src/isolated_history.rs",
    "crates/amplihack-git/src/launcher_state.rs",
    "crates/amplihack-git/src/lib.rs",
    "crates/amplihack-git/src/publication_validation.rs",
    "crates/amplihack-git/src/runtime_preparation.rs",
    "crates/amplihack-git/src/task_scope.rs",
    "crates/amplihack-git/src/workspace_validation.rs",
    "crates/amplihack-git/tests/issue_1554_checkpoint_preservation.rs",
    "crates/amplihack-git/tests/issue_1554_destination_publication.rs",
    "crates/amplihack-git/tests/issue_1554_runtime_boundaries.rs",
    "crates/amplihack-git/tests/issue_1554_task_authority.rs",
    "crates/amplihack-git/tests/publication_boundaries.rs",
    "crates/amplihack-git/tests/support/issue_1554.rs",
    "crates/amplihack-hooks/src/post_tool_use/launcher.rs",
    "crates/amplihack-hooks/src/pre_tool_use/launcher.rs",
    "crates/amplihack-hooks/src/pre_tool_use/tests/launcher_tests.rs",
    "crates/amplihack-launcher/src/nesting_detector.rs",
    "crates/amplihack-launcher/src/session_tracker.rs",
    "crates/amplihack-memory/Cargo.toml",
    "crates/amplihack-memory/src/cli_memory/helpers.rs",
    "crates/amplihack-memory/src/cli_memory/indexing_job.rs",
    "crates/amplihack-memory/src/cli_memory/scip_indexing/commands.rs",
    "crates/amplihack-types/src/paths/mod.rs",
    "crates/amplihack-types/src/paths/owned_runtime.rs",
    "crates/amplihack-utils/src/agent_binary.rs",
    "crates/amplihack-utils/src/artifact_guard.rs",
    "crates/amplihack-utils/src/launcher_context.rs",
    "crates/amplihack-utils/src/tests/artifact_guard_tests.rs",
    "crates/amplihack-utils/src/tests/worktree_tests.rs",
    "crates/amplihack-utils/src/worktree.rs",
    "crates/amplihack-utils/tests/agent_binary_resolver_test.rs",
    "crates/amplihack-utils/tests/agent_binary_security.rs",
    "crates/amplihack-utils/tests/common/mod.rs",
    "crates/amplihack-utils/tests/issue_1554_artifact_policy.rs",
    "docs/artifact-guard.md",
    "docs/features/workflow-runtime-isolation.md",
    "docs/howto/configure-workflow-runtime-isolation.md",
    "docs/index.md",
    "docs/reference/issue-1554-delivery-evidence.md",
    "docs/reference/workflow-checkpoint-publication.md",
    "docs/reference/workflow-runtime-artifacts.md",
    "docs/tutorials/workflow-runtime-isolation.md",
    "tests/issue_1278_git_env_isolation.sh"
  ],
  "rationale": {
    "runtime_and_consumers": "Requirements 2, 3 and 7 require common-directory exclusions before runtime writes, external checkout-specific launcher/session state, preserved clone-shared power-steering state, and consistent writers, readers, locks and agent resolution. The listed runtime modules, memory entry points, launch adapters, nesting consumers and associated fixtures are necessary connection points.",
    "authority_checkpoint_and_guard": "Requirements 4 and 5 require versioned matching authority, exact filenames, private-index checkpoints, normal hooks, preservation of protected entry metadata and independent complete-index runtime enforcement. The listed Git boundary modules, guard adapter and guard tests implement those contracts.",
    "publication": "Requirements 6 and 7 require actual-destination advertisements, complete isolated histories, every explicit pushed ref, failure propagation and structured outcomes consumed by PR creation. The listed publication modules and publication adapters are necessary.",
    "recipes_and_shell_adapters": "Requirements 4 and 7 require explicit auto_stage:false throughout applicable default, consensus, autodrive and repair roles, upstream authority records, early workspace refusal and bounded checkpoint/publication calls. The listed recipes and workflow helpers are the affected execution routes.",
    "shared_git_environment": "The design's sanitised Git subprocess boundary requires repository, configuration and history overrides to be removed consistently. The Rust Git module, git-repo-env.sh, trusted autodrive copy and isolation regression must remain synchronised; these are directly related security integration changes.",
    "dependencies": "Cargo.lock and the two listed crate manifests are necessary only for the dependencies used by these modules and memory integration. The Git crate exports and README establish module connections and contracts. This scope does not authorise a runner pin or generic runner semantic change.",
    "tests_and_harness": "Requirements 7 through 9 require updated external-runtime fixtures, isolated profiles, actual recipe parsing, preservation checks and distinct native acceptance evidence. The listed tests and module registration are necessary. The quiet-process change permits an explicitly selected ignored diagnostic to execute its exact child test rather than silently skip it.",
    "documentation_and_evidence": "Requirements 8 through 10 require accurate current behaviour, limitations, executed checks, deferred Linux identifiers, independent review, historical evidence gaps and the unchanged runner hold. The listed documentation paths cover those subjects; issue-1554-delivery-evidence.md is approved for creation for that purpose. Runner #201 and the original native/historical acceptance obligations remain open.",
    "hook_launcher_consumer": "Requirements 2, 3 and 7 explicitly require external launcher state and complete reader/agent-resolution integration. The PreToolUse consumer currently discovers framework asset roots, which no longer locate the identity-bound external context from nested working directories. Approve exactly crates/amplihack-hooks/src/pre_tool_use/launcher.rs to resolve the nearest checkout using one sanitised git rev-parse --show-toplevel call from the supplied working directory, read only that checkout's external launcher context, preserve environment-marker precedence and staleness checks, and avoid filesystem writes or ancestor-checkout fallback. This necessary consumer amendment adds no runner, pin or acceptance authority.",
    "post_tool_launcher_fixture": "Requirements 7 and 9 require existing session and agent-resolution fixtures to follow external runtime placement without accessing real profiles. Approve the persisted-context unit fixture in crates/amplihack-hooks/src/post_tool_use/launcher.rs to establish a disposable real Git checkout and isolated external state before invoking the real launcher-context writer. Production post-tool message routing and environment-marker precedence remain unchanged.",
    "resolver_fixture_envelopes": "Requirements 7 and 9 require agent-resolution fixtures to use external identity-bound context, disposable real Git boundaries and isolated profiles. Approve crates/amplihack-utils/tests/agent_binary_resolver_test.rs to replace checkout-local raw/aged context fixtures with the external identity envelope, establish real nested repository boundaries, and retain existing malformed-input, staleness and precedence assertions. This amendment does not change production resolver semantics.",
    "recipe_root_sandbox_fixture": "Requirements 7 and 9 require recipe agent-resolution fixtures compatible with external launcher state and isolated profiles. Approve crates/amplihack-cli/src/commands/recipe/run/tests_root_sandbox.rs to replace empty .git-directory stand-ins with disposable real Git repositories and isolated external state before real context writes. Preserve root-sandbox and agent-resolution production behaviour and existing contract assertions.",
    "launch_persistence_fixture_family": "Requirements 7 and 9 require positive launcher-persistence and resolver round-trip fixtures to follow the external identity-bound storage contract. Approve exactly crates/amplihack-cli/src/commands/launch/tests_env.rs to establish disposable real Git checkouts and isolated profile guards before persistence/resolution, update file assertions to external state, and retain existing positive, precedence, non-session and non-launcher controls. This is a fixture amendment with no production routing change.",
    "external_context_security_fixtures": "Requirements 7 and 9 require meaningful security controls against the actual external context reader. Approve exactly crates/amplihack-utils/tests/agent_binary_security.rs to target identity-bound external files for raw JSON, size and symlink mutations, use disposable real repositories and isolated profiles, and retain valid positive controls proving that each negative mutation reaches the intended boundary rather than passing because legacy files are ignored. Production routing and security semantics are not changed by this fixture amendment.",
    "shared_resolver_fixture_identity": "Requirements 7 and 9 require shared agent-binary fixtures to represent real checkout identity and avoid operator profiles. Approve exactly crates/amplihack-utils/tests/common/mod.rs to replace empty .git boundary directories with disposable real Git repositories and establish lifetime-scoped isolated external/profile state before context resolution. The shared helper change is necessary to migrate the identified resolver/security fixture family consistently, without adding production or directory-wide authority.",
    "root_sandbox_child_launch_fixture": "Requirements 7 and 9 require the real run_launch_core test boundary to use a valid disposable Git checkout and isolated external/profile state. Approve exactly crates/amplihack-cli/src/commands/launch/tests_root_sandbox.rs for fixture-only amendments, particularly FakeClaudeLaunch and claude_launch_passes_the_decision_to_the_spawned_child. Keep checkout and profile in separate sibling directories rather than initialising the entire HOME as a repository, establish isolated external state before resolution, initialise the disposable checkout through sanitised Git, and retain the positive IS_SANDBOX child-environment assertions. This approval changes no production routing, sandbox decision semantics, runner pin or acceptance hold."
  }
}
```
