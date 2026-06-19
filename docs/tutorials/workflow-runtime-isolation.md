# Tutorial: Workflow Runtime Isolation

This tutorial shows how a workflow keeps generated runtime output out of a task
worktree while preserving strict Artifact Guard behavior.

Status: Planned implementation contract.
Updated: 2026-06-18

## What you will learn

- Run `default-workflow` with the default external runtime root.
- Override `AMPLIHACK_RUNTIME_ROOT` for a single run.
- Verify that workflow-owned leftovers are cleaned narrowly.
- Confirm that unrelated generated artifacts still fail Artifact Guard.

## Before you start

You need:

- an amplihack checkout
- `git`
- `amplihack` on `PATH`
- `AMPLIHACK_HOME` pointing at the installed amplihack home when using bundled
  helper scripts directly

Run commands from a disposable branch or scratch worktree.

## 1. Run a workflow normally

```bash
amplihack recipe run default-workflow \
  -c task_description="Update README wording" \
  -c repo_path=. \
  --format json > result.json 2> progress.log
```

The workflow writes generated runtime state outside the Git worktree and passes
the selected runtime root to all child workflows. The task worktree contains
source changes only.

## 2. Override the runtime root

Use an external path when CI needs predictable cleanup.

```bash
export AMPLIHACK_RUNTIME_ROOT="/tmp/amplihack-runtime/$USER/tutorial-run"
install -d -m 700 "$AMPLIHACK_RUNTIME_ROOT"

amplihack recipe run default-workflow \
  -c task_description="Update README wording" \
  -c repo_path=.
```

Inspect the runtime directories:

```bash
find "$AMPLIHACK_RUNTIME_ROOT" -maxdepth 1 -type d | sort
```

Expected directories:

```text
/tmp/amplihack-runtime/<user>/tutorial-run
/tmp/amplihack-runtime/<user>/tutorial-run/locks
/tmp/amplihack-runtime/<user>/tutorial-run/logs
/tmp/amplihack-runtime/<user>/tutorial-run/metrics
/tmp/amplihack-runtime/<user>/tutorial-run/provenance
/tmp/amplihack-runtime/<user>/tutorial-run/reflection
```

## 3. Simulate legacy runtime leftovers

Create the two known workflow-owned paths that old runs could leave inside a
task worktree. In managed task worktrees, root-level `worktrees/` is reserved
for workflow-owned nested scratch worktrees.

```bash
mkdir -p .claude/runtime
mkdir -p worktrees/generated-child-worktree

mkdir -p .claude
printf '{"permissions":{}}\n' > .claude/settings.json
```

Run the workflow runtime preflight:

```bash
. "$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
preflight_known_workflow_runtime_artifacts "$PWD"
```

Verify the result:

```bash
test ! -e .claude/runtime
test ! -e worktrees
test -f .claude/settings.json
```

The known runtime leftovers are gone. The user-authored `.claude` configuration
file remains.

## 4. Confirm strict guard behavior

Create an unrelated generated artifact:

```bash
mkdir -p dist
printf 'generated bundle\n' > dist/plugin.js
```

Run Artifact Guard:

```bash
amplihack hygiene artifact-guard --repo . --mode all
```

The guard fails because `dist/plugin.js` is not a known workflow runtime
artifact. Remove it before publishing:

```bash
rm -- dist/plugin.js
rmdir dist
amplihack hygiene artifact-guard --repo . --mode all
```

## What happened

The workflow runtime isolation contract has two layers:

1. Workflow runtime output is written outside the task worktree through
   `AMPLIHACK_RUNTIME_ROOT`.
2. Lifecycle preflight removes only known workflow-owned leftovers before
   checkpoint, staging, publish, pre-commit, and finalization gates.

Artifact Guard stays strict. It still blocks unexpected generated artifacts that
are not part of the narrow runtime cleanup contract. General clean-worktree gates
also remain strict because the runtime preflight does not remove arbitrary
untracked files.

## Related documentation

- [Workflow Runtime Isolation](../features/workflow-runtime-isolation.md)
- [Configure Workflow Runtime Isolation](../howto/configure-workflow-runtime-isolation.md)
- [Workflow Runtime Artifacts Reference](../reference/workflow-runtime-artifacts.md)
- [Artifact Guard](../artifact-guard.md)
