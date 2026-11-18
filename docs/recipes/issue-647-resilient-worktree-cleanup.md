---
title: Resilient Worktree Cleanup in Default Workflow
description: Why read-only verification can use a fallback checkout while publication and readiness require the recorded worktree.
last_updated: 2026-11-18
review_schedule: as-needed
owner: workflow-team
doc_type: explanation
---

# Resilient Worktree Cleanup in Default Workflow (Issue #647)

Late-stage resilience depends on the operation. Read-only verification can use
another checkout when the task worktree has been removed. Publication and PR
readiness need the recorded task worktree: an open PR may still lack changes
made during the quality audit.

## Directory access by operation

| Step | Recipe | Missing or unusable worktree |
| --- | --- | --- |
| 15 (design), 16 (review), 18c (enforce-verdict) | `workflow-pr-review` | Stop: worktree files are required. |
| 19c (zero-BS verification) | `workflow-pr-review` | Warn and use the read-only fallback chain. |
| `checkpoint-after-implementation` | `workflow-tdd` | Stop before staging or committing. |
| `checkpoint-after-review-feedback` | `workflow-refactor-review` | Stop before staging or committing. |
| 15 (commit-push) | `workflow-publish` | Stop before staging or committing. |
| 20a (artifact-guard) | `workflow-finalize` | Warn and skip; never run preflight in the launch checkout. |
| 20b (push-cleanup) | `workflow-finalize` | Enter essential recovery; publication cannot succeed by skipping. |
| 21 (pr-ready) | `workflow-finalize` | Enter essential recovery before reading delivery inputs or Git identity. |

## Steps that commit never fall back (issue #1528)

`REPO_PATH` is the launch checkout, often on `main`. Staging there could include
unrelated work or the run's worktrees. Commit operations therefore require the
top of the recorded linked task worktree and never substitute the launch path.

Final publication uses `workflow_finalize_publish.sh` to stage and commit audit
edits, then push all local commits. It performs its own runtime preflight and
artifact checks, even if step 20a skipped. A publication failure enters recovery
before readiness runs. Existing commits are preserved without rebasing or
force-pushing.

Readiness also needs the task worktree, even though it does not commit. Its
branch, repository, delivery inputs, and exact published HEAD determine which
PR it may mark ready. Reading these from a checkout on `main` would validate
the wrong context. See
[Finalize Publication and PR Readiness](../reference/workflow-finalize-publication.md).

## Fallback pattern

The read-only step 19c uses the task worktree when available, then `REPO_PATH`,
then the current directory. Each fallback prints a warning. This pattern does
not apply to final publication or readiness.

Those operations resolve `worktree_setup.worktree_path` through
`worktree_setup_path.sh`, which reads the complete `RECIPE_VAR_worktree_setup`
object. An inherited bare `WORKTREE_SETUP_WORKTREE_PATH` cannot replace the
recorded output. Missing or malformed context enters recovery; it is not a
successful no-op.

## Configuration

No setting enables a directory fallback for final publication or readiness.
The caller must propagate the recorded `worktree_setup` object. Both operations
use essential recovery with stop-on-handoff; a supported local repair is followed
by validated re-execution, or a recorded handoff stops the run with exit `81`.

## Security

Worktree selection precedes staging and readiness identity reads. Recovery
performs no GitHub writes; the original publication command owns pushes and the
readiness command owns the validated readiness mutation. A recovery child cannot
repair parent recipe context merely by exporting environment variables.

## Validation

Behavioral fixtures use linked worktrees, local remotes, and isolated configuration
to invoke finalization from a checkout on `main`. They verify that audit changes
reach the PR branch before readiness and that unusable context enters recovery
without changing the launch checkout. See the
[behavioral verification contract](../reference/workflow-finalize-publication.md#behavioral-verification).

## Related

- [Worktree Setup Propagation](../reference/worktree-setup-propagation.md) — Recorded context across recipe boundaries.
- [Workflow Runtime Artifacts](../reference/workflow-runtime-artifacts.md#lifecycle-integration) — Preflight and artifact handling during publication.
- [Step Failure Recovery](../reference/step-failure-recovery.md) — Supported repairs, validated re-execution, and handoff records.
