# [PLANNED - Implementation Pending] How to Verify Workflow Artifact Guarding

> [Home](../index.md) > How-To > Verify Workflow Artifact Guarding

This guide describes how to verify the planned workflow artifact guard. Remove
the `[PLANNED - Implementation Pending]` marker after the `workflow-finalize`
recipe and regression tests implement this contract.

Use this guide when you run or review a workflow that may produce transient
runner, session, or scratch artifacts before finalization.

## Before You Start

You need:

- a writable Git worktree
- `amplihack` on `PATH`
- the recipe branch context you normally pass to `default-workflow`

The artifact guard will be enabled by default. There will be no runtime flag to
turn it on and no supported runtime flag to bypass it.

## 1. Run the Workflow Normally

Start `default-workflow` or `smart-orchestrator` with the same context you use
for the task:

```bash
amplihack recipe run default-workflow \
  -c "repo_path=/home/user/src/amplihack-rs" \
  -c "task_description=Finalize the workflow without committing transient artifacts" \
  -c "existing_branch=feature/workflow-cleanup"
```

The artifact guard will not read environment variables for behavior changes.

## 2. Know What Gets Removed

The guard will remove only these repo-local transient paths when they exist:

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

The list is intentionally literal. Do not add broad patterns to local wrapper
scripts. If a new workflow artifact needs protection, add it to the recipe and
the structural regression tests together.

## 3. Confirm Guard Ordering

Inspect the finalized recipe when reviewing a change:

```bash
amplihack recipe show workflow-finalize | sed -n '/step-20b-push-cleanup/,/step-20c-quality-audit/p'
```

The planned cleanup block must appear before the first broad staging command:

```bash
# deterministic workflow artifact guard
# ...
git add -A
```

If `git add -A` appears before the guard, the recipe change is invalid.

## 4. Verify a Local Worktree

From a worktree, these commands show the files that would leak if the guard were
missing:

```bash
cd /home/user/src/amplihack-rs
printf 'runner output\n' > recipe-runner.log
mkdir -p session-state ai_working/investigation
printf '# workflow plan\n' > plan.md
git status --short
```

After `workflow-finalize` reaches `step-20b-push-cleanup`, the status output
must not include those artifacts:

```bash
git status --short
```

Legitimate project files must remain visible and can still be staged by
finalization.

## 5. Understand Non-Configurable Behavior

These planned behaviors are fixed:

- cleanup is scoped to the resolved workflow worktree
- the artifact allowlist is maintained in the recipe and tests, not user config
- missing artifact paths are harmless
- removal failures for existing artifact paths stop finalization
- `git add -A`, commit, pull/rebase, and push failures are not suppressed

Do not bypass the guard with a wrapper script, a custom `git add -A`, or a local
`.gitignore` rule that hides the artifact after it has already entered the
working tree.

## Troubleshooting

| Symptom | Meaning | Fix |
| --- | --- | --- |
| `ERROR: workflow artifact guard requires a git worktree` | Finalization is not running in a Git checkout. | Run from the resolved workflow worktree or fix `worktree_setup.worktree_path`. |
| `ERROR: cleanup target resolves outside worktree` | A target path escaped the worktree boundary. | Remove the unsafe path from the recipe change and keep only repo-local literals. |
| `ERROR: failed to remove workflow artifact` | A known artifact exists but could not be removed. | Inspect permissions or locks, then rerun finalization. |
| Artifact still appears in `git status` | The path is not in the documented allowlist or is a legitimate project file. | Add a targeted recipe/test change only if it is workflow-owned transient output. |

## Related Documentation

- [Workflow artifact guard overview](../features/workflow-artifact-guard.md)
- [Workflow artifact guard reference](../reference/workflow-artifact-guard.md)
- [Tutorial: workflow artifact guard](../tutorials/workflow-artifact-guard.md)
