# [PLANNED - Implementation Pending] Tutorial: Workflow Artifact Guard

**Time to complete**: 15 minutes
**Skill level**: Intermediate

This tutorial describes the intended workflow artifact guard behavior. Remove
the `[PLANNED - Implementation Pending]` marker after the `workflow-finalize`
recipe and regression tests implement this contract.

You create representative workflow artifacts in a disposable worktree, run the
workflow finalization path, and confirm that only legitimate project changes
remain eligible for staging.

## What You'll Learn

By the end of this tutorial you can:

1. Identify workflow-owned artifacts that finalization will remove.
2. Confirm the guard will run before `git add -A`.
3. Distinguish repo-local artifacts from user-home session history.
4. Interpret guard failures.
5. Run the structural regression test for guard ordering after implementation.

## Prerequisites

You need:

- a writable clone of `amplihack-rs`
- `amplihack` on `PATH`
- `git` on `PATH`
- enough permissions to create and delete files in a disposable worktree

## Step 1: Create a Disposable Branch

Start from a clean worktree or a throwaway clone:

```bash
cd /home/user/src/amplihack-rs
git switch -c docs-artifact-guard-demo
```

Create one real project change:

```bash
mkdir -p docs/examples
printf '# Demo\n' > docs/examples/artifact-guard-demo.md
```

## Step 2: Add Workflow Artifacts

Create representative files that can be produced by interrupted recipe, session,
or scratch tooling:

```bash
printf 'recipe runner output\n' > recipe-runner.log
printf '# transient workflow plan\n' > plan.md
mkdir -p session-state ai_working/investigation .copilot/session-state
printf '{}\n' > session-state/events.jsonl
printf 'scratch\n' > ai_working/investigation/notes.txt
printf '{}\n' > .copilot/session-state/events.jsonl
```

Before finalization, `git status --short` shows both the real change and the
transient artifacts:

```bash
git status --short
```

## Step 3: Inspect the Guard Ordering

The guard will live in `workflow-finalize` step `step-20b-push-cleanup`:

```bash
amplihack recipe show workflow-finalize | sed -n '/step-20b-push-cleanup/,/step-20c-quality-audit/p'
```

The artifact guard must appear before:

```bash
git add -A
```

That ordering is the safety property. If broad staging happens first, transient
files can enter the cleanup commit.

## Step 4: Run a Workflow That Reaches Finalization

Run the normal workflow path:

```bash
amplihack recipe run default-workflow \
  -c "repo_path=/home/user/src/amplihack-rs" \
  -c "task_description=Finalize the artifact guard demo branch without committing workflow artifacts" \
  -c "existing_branch=docs-artifact-guard-demo"
```

When `workflow-finalize` reaches `step-20b-push-cleanup`, the guard will remove
the documented transient artifacts before staging.

## Step 5: Confirm Only Project Changes Remain

Inspect the worktree:

```bash
git status --short
```

The demo documentation change can remain:

```text
?? docs/examples/artifact-guard-demo.md
```

The workflow artifacts must be absent:

```text
recipe-runner.log
plan.md
session-state/
.copilot/session-state/
ai_working/investigation/
```

User-home session history will not be touched:

```bash
test -d "$HOME/.copilot/session-state" && echo "home session state still exists"
```

## Step 6: Run the Structural Regression Test

After implementation, the structural test locks the recipe invariant:

```bash
cargo test --test default_workflow_decomposition_test workflow_finalize_artifact_guard_runs_before_broad_staging
```

The test verifies:

- the guard exists in `workflow-finalize`
- it appears before `git add -A`
- the documented artifacts are represented
- critical Git operations are not hidden behind unsafe `|| true` fallbacks

## Next Steps

- Use the [verification guide](../howto/verify-workflow-artifact-guard.md) when reviewing the guard contract.
- Use the [reference](../reference/workflow-artifact-guard.md) when changing recipe structure or test coverage.
- Use the [feature overview](../features/workflow-artifact-guard.md) for the high-level safety guarantees.
