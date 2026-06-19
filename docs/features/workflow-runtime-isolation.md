# Workflow Runtime Isolation

Workflow runtime isolation keeps generated agent state, provenance, logs,
metrics, reflection output, and workflow-owned nested worktrees out of commit
worktrees.

Status: Planned implementation contract.
Updated: 2026-06-18

## Contents

- [What it protects](#what-it-protects)
- [Runtime root contract](#runtime-root-contract)
- [Lifecycle preflight](#lifecycle-preflight)
- [What cleanup is allowed to remove](#what-cleanup-is-allowed-to-remove)
- [What Artifact Guard still blocks](#what-artifact-guard-still-blocks)
- [Related documentation](#related-documentation)

## What it protects

`default-workflow` and PR recovery runs spawn nested agents, child recipes, shell
helpers, checkpoint steps, publish steps, and finalization checks. Those runtime
activities generate files that are useful while the workflow is running but must
not become source code:

```text
provenance/
logs/
metrics/
reflection/
locks/
agent runtime state
workflow-owned scratch worktrees
```

The workflow isolates that output under a shared runtime root instead of writing
it into the task worktree. The active Git worktree remains reserved for source
changes that can be reviewed, staged, committed, pushed, and merged.

## Runtime root contract

Every workflow run has one runtime root. The top-level workflow establishes
that root once and exports `AMPLIHACK_RUNTIME_ROOT`. Launchers, recipe steps,
provenance logging, child agents, and shell helpers inherit that same value.
Child workflows must not recompute their own runtime roots.

1. If `AMPLIHACK_RUNTIME_ROOT` is set, use it.
2. Otherwise use `$XDG_RUNTIME_DIR/amplihack/runtime/<run-id>` when
   `XDG_RUNTIME_DIR` is available.
3. Otherwise use `/tmp/amplihack-runtime/<user>/<run-id>`.

The runtime root is outside the commit worktree by default. It contains these
standard subdirectories:

```text
locks/
logs/
metrics/
provenance/
reflection/
```

The runtime root and its subdirectories are created with restrictive
owner-only permissions where the platform supports them. Custom
`AMPLIHACK_RUNTIME_ROOT` values should point at a private external directory,
not a shared repository path.

See [Workflow Runtime Artifacts Reference](../reference/workflow-runtime-artifacts.md)
for the exact resolution order, environment variables, and helper APIs.

## Lifecycle preflight

Workflow lifecycle gates clean known workflow-owned artifacts before operations
that inspect or broadly stage the worktree:

| Lifecycle point | Behavior |
| --- | --- |
| Checkpoint | Remove known workflow runtime artifacts before checkpoint status checks. |
| Broad staging | Remove known workflow runtime artifacts before `git add -A` or equivalent broad staging. |
| Publish | Remove known workflow runtime artifacts before dirty-worktree checks, publication staging, commit, push, and PR creation. |
| Pre-commit staging | Remove known workflow runtime artifacts before pre-commit-related staging paths. |
| Finalization | Remove known workflow runtime artifacts before final status and clean-worktree gates. |

This preflight is defense-in-depth. The preferred behavior is still isolation
outside the worktree. Cleanup handles interrupted or legacy child processes that
left known workflow-generated paths behind.

## What cleanup is allowed to remove

Cleanup is intentionally narrow. It removes only these exact paths under the
active task worktree:

```text
<worktree>/.claude/runtime
<worktree>/worktrees
```

In amplihack-managed task worktrees, root-level `worktrees/` is reserved for
workflow-owned nested scratch worktrees. Do not use that directory for
user-authored source. If an implementation detects tracked source under
`worktrees/`, it must fail closed instead of deleting it.

It does not remove:

```text
<worktree>/.claude
<worktree>/.claude/settings.json
<worktree>/.claude/agents
<worktree>/.claude/skills
<worktree>/node_modules
<worktree>/dist
<worktree>/target
unrelated untracked files
```

Absent known paths are a no-op. Existing targets are resolved and validated
before deletion: the worktree must be a Git worktree, the target must be inside
that worktree, and the target must exactly match a known workflow-owned path.

## What Artifact Guard still blocks

[Artifact Guard](../artifact-guard.md) remains strict and non-mutating. It still
fails on generated, runtime, cache, dependency, and build artifacts that are not
part of the narrow workflow runtime cleanup contract.

For example, these paths still block publication or pre-commit gates:

```text
node_modules/
dist/plugin.js
coverage/
.pytest_cache/
logs/generated.log
```

The workflow does not weaken Artifact Guard with broad ignore rules or broad
allowlists. It removes only known workflow-owned runtime leftovers before the
guard runs, then lets the guard fail on anything unexpected.

## Related documentation

- [Configure Workflow Runtime Isolation](../howto/configure-workflow-runtime-isolation.md)
- [Workflow Runtime Artifacts Reference](../reference/workflow-runtime-artifacts.md)
- [Tutorial: Workflow Runtime Isolation](../tutorials/workflow-runtime-isolation.md)
- [Artifact Guard](../artifact-guard.md)
