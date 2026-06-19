---
title: "Preflight Workflow Runtime Artifacts"
description: "How to use the workflow runtime artifact helper before Artifact Guard, staging, pre-commit, publish, and finalization."
last_updated: 2026-06-18
review_schedule: quarterly
owner: amplihack
doc_type: howto
---

# Preflight Workflow Runtime Artifacts

> [Home](../index.md) > How-To > Preflight Workflow Runtime Artifacts

Use this guide when editing bundled recipes that run checkpoints, broad staging,
pre-commit, publish, or finalization. The workflow runtime artifact helper
removes known generated leftovers before strict Artifact Guard gates run.

## Before you start

You need:

- a real git worktree
- the bundled helper at `amplifier-bundle/tools/workflow_runtime_artifacts.sh`
- Artifact Guard available through `amplihack hygiene artifact-guard`

Do not add `.claude/runtime/` or `worktrees/` to the Artifact Guard allowlist.
The fix is lifecycle cleanup and output isolation, not weakening enforcement.

## 1. Source the helper

In a recipe shell step, source the helper before calling its functions:

```bash
. "$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
```

When `AMPLIHACK_HOME` is not available in a local development shell, source the
checkout-relative file:

```bash
. ./amplifier-bundle/tools/workflow_runtime_artifacts.sh
```

## 2. Preflight the active worktree

Call preflight with the canonical worktree path when the recipe has one:

```bash
preflight_known_workflow_runtime_artifacts "$WORKTREE_SETUP_WORKTREE_PATH"
```

When the path is omitted, the helper resolves the repository from recipe context
variables and then from the current directory:

```bash
preflight_known_workflow_runtime_artifacts
```

Use the explicit argument when possible. It makes the guarded target obvious in
logs and avoids relying on ambient `cwd`.

## 3. Run Artifact Guard after preflight

Place Artifact Guard immediately after preflight:

```bash
preflight_known_workflow_runtime_artifacts "$repo"
amplihack hygiene artifact-guard --repo "$repo" --mode all
```

This order means known workflow-generated `.claude/runtime/` and marked nested
workflow worktrees are removed first. Unknown artifacts are still reported by
Artifact Guard.

## 4. Stage only after both gates pass

For broad staging, the safe sequence is:

```bash
preflight_known_workflow_runtime_artifacts "$repo"
amplihack hygiene artifact-guard --repo "$repo" --mode all
git -C "$repo" add -A
```

Do not insert generated-output steps between Artifact Guard and `git add -A`.
If a command can create runtime output, run preflight and Artifact Guard again
before staging.

## 5. Create new workflow worktrees outside the active repo

`workflow-worktree.yaml` should create new branch worktrees outside the active
commit worktree. The default parent is a sibling `worktrees` directory:

```text
../worktrees/<branch-path-slug>
```

When the active repository is already inside a `worktrees` directory, create the
new worktree as a sibling of the active repository:

```text
../<branch-path-slug>
```

Set `AMPLIHACK_WORKTREE_PARENT` only when a recipe or test needs an explicit
external parent. The resolved final path must still be outside the active
repository root. If no safe external parent is available, fail the workflow
instead of falling back to `"$repo/worktrees/..."`.

## 6. Mark compatibility nested worktrees

`workflow-worktree.yaml` avoids creating nested worktrees inside commit
worktrees. Creating a new nested worktree is allowed only for explicit emergency
compatibility runs with `AMPLIHACK_ALLOW_NESTED_WORKTREE=1`. If that fallback is
used, create the ownership marker at the nested worktree root:

```bash
touch "$repo/worktrees/$branch_slug/.amplihack-workflow-worktree"
```

The marker is the cleanup proof. Without it, cleanup fails closed and leaves the
directory in place for inspection.

Do not track the marker. A tracked marker or tracked nested worktree content
turns the directory into source-controlled content, so cleanup must refuse to
delete it.

## 7. Keep user files out of cleanup paths

The cleanup helper only owns these generated paths:

```text
.claude/runtime/
worktrees/<marked-workflow-worktree>/
```

Do not place user notes, local scratch files, checked-in fixtures, or manual
worktrees under those paths. Use a project-specific location outside the commit
worktree, an ignored `.amplihack/` runtime path, or a normal branch worktree
outside the active commit worktree.

## Recipe placement checklist

Add preflight immediately before these phases:

| Phase | Required placement |
| --- | --- |
| Checkpoint | Before checkpoint Artifact Guard. |
| Broad staging | Before Artifact Guard and `git add -A`. |
| Pre-commit | Immediately before `pre-commit run`. |
| Publish | Before publish Artifact Guard and before commit/push staging. |
| Finalization | Before final Artifact Guard, final staging, and push cleanup. |

The placement must be local to the guarded action. A preflight several steps
earlier is not sufficient because nested agents can create runtime output after
that point.

## Troubleshooting

| Symptom | Meaning | Action |
| --- | --- | --- |
| `not a git worktree` | The target path cannot be validated with `git rev-parse --show-toplevel`. | Pass the real repository or workflow worktree path. |
| `.claude/runtime is a symlink` | Cleanup refuses symlink targets. | Remove the symlink after confirming it is not needed, then rerun. |
| `tracked files under .claude/runtime` | Cleanup refuses to delete tracked content. | Inspect the tracked paths and remove or relocate them intentionally. |
| `worktrees is a symlink` | Cleanup refuses to traverse a symlinked worktree parent. | Replace it with a real directory only after confirming ownership. |
| `worktrees/<name> is a symlink` | Cleanup refuses symlinked nested worktrees. | Remove or relocate the symlink manually after inspection. |
| `unmarked nested worktree` | `worktrees/<name>/` lacks `.amplihack-workflow-worktree`. | Add the marker only if the workflow created it, otherwise move or remove the directory manually. |
| `tracked marker or nested content` | Cleanup refuses to delete source-controlled content under `worktrees/`. | Remove the tracked files through normal Git changes or move them outside cleanup paths. |
| Artifact Guard still reports `node_modules/` | Preflight is not a general artifact cleaner. | Remove or relocate the dependency tree; do not allowlist it broadly. |

## Related documentation

- [Workflow Runtime Artifacts Reference](../reference/workflow-runtime-artifacts.md)
- [Artifact Guard](../artifact-guard.md)
- [Workflow Publish and Finalize Resilience](../operations/workflow-resilience.md)
