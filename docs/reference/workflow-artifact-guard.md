# [PLANNED - Implementation Pending] Workflow Artifact Guard Reference

> [Home](../index.md) > Reference > Workflow Artifact Guard

This reference describes the intended field-level and recipe-level contract for
deterministic workflow artifact cleanup before broad Git staging. Remove the
`[PLANNED - Implementation Pending]` marker after the `workflow-finalize` recipe
and regression tests implement this contract.

## Contents

- [Recipe Surface](#recipe-surface)
- [Workflow Context Inputs](#workflow-context-inputs)
- [Artifact Allowlist](#artifact-allowlist)
- [Ordering Contract](#ordering-contract)
- [Failure Semantics](#failure-semantics)
- [Security Invariants](#security-invariants)
- [Regression Coverage](#regression-coverage)

## Recipe Surface

The public workflow surface will be the `workflow-finalize` recipe:

```text
amplifier-bundle/recipes/workflow-finalize.yaml
```

The guard will live in bash step:

```text
step-20b-push-cleanup
```

There will be no public Rust API and no standalone CLI subcommand for the guard.
The contract is recipe-owned and will be exercised through `default-workflow`,
`smart-orchestrator`, or direct `workflow-finalize` execution.

## Workflow Context Inputs

The guard will use the same finalization context as the cleanup/push step.

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `worktree_setup.worktree_path` | absolute path string | Yes for cleanup/staging finalization | Worktree created or reused by step 04. The guard will run from this directory. |
| `repo_path` | absolute path string | Yes for broader finalization context | Repository path used by surrounding finalization/reporting logic. It is not a cleanup target override after the workflow worktree is resolved. |
| `terminal_state.should_finalize` | string/bool-like | Yes | Finalization will run only when terminal-state logic allows it. |
| `terminal_state.terminal_success` | string/bool-like | Yes | Successful terminal states will skip cleanup/push finalization. |

The artifact guard will not add new user-facing context fields.

## Artifact Allowlist

Only these literal repo-local paths will be eligible for cleanup:

| Path | Type | Meaning |
| --- | --- | --- |
| `recipe-runner.log` | file | Runner output accidentally written in the worktree root. |
| `plan.md` | file | Repo-root workflow/session plan artifact. Nested project docs are not included. |
| `session-state/` | directory | Repo-local session state accidentally rooted in the checkout. |
| `.copilot/session-state/` | directory | Copilot session state accidentally rooted in the checkout. |
| `.claude/runtime/locks/.workflow_active` | file | Repo-local workflow-active semaphore left by interrupted routing. |
| `ai_working/ddd/` | directory | DDD scratch output. |
| `ai_working/consensus/` | directory | Consensus workflow scratch output. |
| `ai_working/n-version/` | directory | N-version workflow scratch output. |
| `ai_working/investigation/` | directory | Investigation workflow scratch output. |
| `ai_working/cascade/` | directory | Cascade workflow scratch output. |

Rules:

- Paths are interpreted relative to the resolved finalization worktree.
- Absolute cleanup targets are invalid.
- Parent-directory references such as `../plan.md` are invalid.
- Globs are invalid.
- User-home paths such as `~/.copilot/session-state/` are out of scope.
- Missing allowlist paths are not errors.

## Ordering Contract

`step-20b-push-cleanup` must follow this order:

1. Enter the resolved finalization worktree.
2. Verify the current directory is inside a Git worktree.
3. Configure pager-safe Git output.
4. Resolve the current branch and reject detached HEAD.
5. Run the deterministic workflow artifact guard.
6. Run `git add -A`.
7. Commit staged cleanup changes if any exist.
8. Pull/rebase and push according to the existing finalization behavior.

The guard must be the last cleanup gate before broad staging. Adding another
artifact-producing command between the guard and `git add -A` violates the
contract unless that command is itself proven not to write worktree files.

## Failure Semantics

| Condition | Result |
| --- | --- |
| Current directory is not a Git worktree | Exit non-zero before cleanup and staging. |
| Cleanup target resolves outside the worktree | Exit non-zero before cleanup and staging. |
| Cleanup target is not in the allowlist | Exit non-zero before cleanup and staging. |
| Existing allowlist target cannot be removed | Exit non-zero before staging. |
| Allowlist target is absent | Continue. |
| No staged changes remain after cleanup | Print the existing no-staged-changes message and continue finalization. |
| `git commit`, `git pull --rebase`, or `git push` fails | Preserve existing fail-fast behavior and print redacted diagnostics. |

The guard must not use `|| true` to hide cleanup failure. Critical Git
operations must also remain fail-fast.

## Security Invariants

The workflow artifact guard will enforce these invariants:

- no deletion outside the resolved worktree
- no deletion based on user-supplied path input
- no dynamic command construction
- no `eval`
- no recursive wildcard cleanup
- no printing artifact contents
- no weakening of GitHub identity, PR publication, issue-closing, or merge gates
- no issue closure caused by artifact cleanup

If an invariant cannot be proven, finalization must fail before staging.

## Regression Coverage

Structural recipe coverage must live with the default-workflow decomposition
tests:

```text
tests/integration/default_workflow_decomposition_test.rs
```

The regression suite must assert:

- `workflow-finalize` contains a workflow artifact guard in `step-20b-push-cleanup`
- the guard appears before the first `git add -A`
- the allowlist includes `recipe-runner.log`, `plan.md`, repo-local session state, and workflow scratch outputs
- the guard does not rely on broad globs
- the guard and critical Git operations are not followed by unsafe `|| true` fallbacks

After implementation, run the focused test with:

```bash
cargo test --test default_workflow_decomposition_test workflow_finalize_artifact_guard_runs_before_broad_staging
```

## See Also

- [Workflow artifact guard overview](../features/workflow-artifact-guard.md)
- [How to verify workflow artifact guarding](../howto/verify-workflow-artifact-guard.md)
- [Tutorial: workflow artifact guard](../tutorials/workflow-artifact-guard.md)
- [Workflow-owned PR recovery readiness](../features/pr-recovery-readiness.md)
