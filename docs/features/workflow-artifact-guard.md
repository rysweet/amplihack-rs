# [PLANNED - Implementation Pending] Workflow Artifact Guard

This document describes the intended behavior of the workflow artifact guard.
Remove the `[PLANNED - Implementation Pending]` marker after the
`workflow-finalize` recipe and regression tests implement this contract.

**`workflow-finalize` will remove known workflow/session artifacts before broad
Git staging so transient runner files do not leak into cleanup commits.**

> [Home](../index.md) > [Features](README.md) > Workflow Artifact Guard

## Quick Navigation

- [How to verify workflow artifact guarding](../howto/verify-workflow-artifact-guard.md)
- [Tutorial: workflow artifact guard](../tutorials/workflow-artifact-guard.md)
- [Workflow artifact guard reference](../reference/workflow-artifact-guard.md)

## What This Feature Does

The planned workflow artifact guard is a deterministic cleanup gate in
`workflow-finalize`. It will run in `step-20b-push-cleanup` immediately before
the first broad staging command:

```bash
git add -A
```

The guard will delete only fixed, repo-local artifacts that are produced by
recipe runners, session tooling, or workflow scratch space. It will not inspect
user input, expand user-provided globs, or silently continue after an unsafe
cleanup failure.

## Operational Guarantees

| Guarantee | Behavior |
| --- | --- |
| Ordered before broad staging | The artifact guard will run after final cleanup and before `git add -A`. |
| Repo-local only | Cleanup will be scoped to the resolved workflow worktree. User-home session history will never be touched. |
| Fixed artifact set | Only documented literal paths will be eligible for cleanup. |
| No broad globs | The guard will not use recursive wildcard cleanup such as `rm -rf *`, `find .`, or `git clean`. |
| Fail closed | If the guard cannot prove it is in a Git worktree or cannot remove a known artifact safely, finalization will stop before staging. |
| Existing workflow behavior preserved | Normal cleanup commits, pre-commit hooks, pull/rebase, push, PR readiness checks, and final status reporting will keep their existing semantics. |

## Quick Start

No feature flag will be required. Run the normal workflow; artifact protection
will be part of finalization:

```bash
amplihack recipe run default-workflow \
  -c "repo_path=/home/user/src/amplihack-rs" \
  -c "task_description=Finalize the workflow without committing transient artifacts" \
  -c "existing_branch=feature/workflow-cleanup"
```

## Cleanup Scope

The guard will handle only workflow-owned transient paths at the worktree root
or in documented workflow scratch directories:

```text
recipe-runner.log
plan.md
session-state/
.copilot/session-state/
.claude/runtime/locks/.workflow_active
ai_working/ddd/
ai_working/consensus/
ai_working/n-version/
ai_working/investigation/
ai_working/cascade/
```

Nested project documentation such as `docs/plan.md` is outside the cleanup
scope. User-home session state such as `~/.copilot/session-state/` is also
outside the cleanup scope.

## Failure Behavior

Finalization will stop before staging when:

- the workflow cannot enter the resolved worktree
- the current directory is not a Git worktree
- a cleanup target resolves outside the worktree
- a cleanup target is not one of the documented fixed paths
- removal of an existing cleanup target fails

The workflow will report the concrete path and reason. It will not hide cleanup
failures behind `|| true`, weaken the path checks, or continue to `git add -A`
after a guard failure.

## Where To Go Next

- Use the [verification guide](../howto/verify-workflow-artifact-guard.md) to inspect the planned guard contract.
- Use the [tutorial](../tutorials/workflow-artifact-guard.md) for a disposable-worktree walkthrough.
- Use the [reference](../reference/workflow-artifact-guard.md) when updating recipes or regression tests.
