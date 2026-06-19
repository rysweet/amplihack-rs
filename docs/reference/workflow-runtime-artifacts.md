---
title: "Workflow Runtime Artifacts Reference"
description: "Reference contract for isolating and preflighting workflow-generated .claude/runtime and nested worktree artifacts before Artifact Guard gates."
last_updated: 2026-06-18
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Workflow Runtime Artifacts Reference

> [Home](../index.md) > Reference > Workflow Runtime Artifacts

Workflow runtime artifact handling keeps generated agent runtime output out of
commit worktrees before strict Artifact Guard gates run. The cleanup helper
removes only known workflow-owned artifacts and fails closed when it cannot
prove a target is safe.

Artifact Guard remains the enforcement boundary. It is still read-only, strict,
and responsible for blocking unexpected artifacts.

## Contents

- [Artifact classes](#artifact-classes)
- [Helper file](#helper-file)
- [Repository resolution](#repository-resolution)
- [External workflow worktree placement](#external-workflow-worktree-placement)
- [Safety contract](#safety-contract)
- [Function reference](#function-reference)
- [Recipe lifecycle coverage](#recipe-lifecycle-coverage)
- [Configuration](#configuration)
- [Regression expectations](#regression-expectations)

## Artifact classes

The workflow helper recognizes two generated artifact classes:

| Artifact | Path | Cleanup rule |
| --- | --- | --- |
| Agent runtime output | `.claude/runtime/` | Remove the runtime directory and its untracked or ignored generated contents after verifying the path is not tracked and not a symlink. Never remove `.claude/` itself. |
| Legacy workflow-created nested worktrees | `worktrees/<name>/` | Remove only untracked or ignored nested worktree directories that contain `.amplihack-workflow-worktree`. Reject unmarked or tracked content. |

The helper does not remove dependency trees, build outputs, caches, logs outside
the recognized paths, or arbitrary untracked files. Those remain Artifact Guard
violations when prohibited.

## Helper file

Recipes source the shared helper from:

```text
amplifier-bundle/tools/workflow_runtime_artifacts.sh
```

The helper exposes these shell functions:

```bash
cleanup_known_workflow_runtime_artifacts [repo]
preflight_known_workflow_runtime_artifacts [repo]
```

Both functions write concise status and error messages to stderr. They do not
print file contents.

## Repository resolution

When a function receives an explicit `repo` argument, that value is authoritative.
When the argument is omitted, the helper resolves the target repository using
this precedence order:

| Precedence | Source |
| --- | --- |
| 1 | explicit function argument |
| 2 | `WORKTREE_SETUP_WORKTREE_PATH` |
| 3 | `RECIPE_VAR_worktree_setup__worktree_path` |
| 4 | `REPO_PATH` |
| 5 | current working directory |

After selecting a candidate, the helper runs:

```bash
git -C "$repo" rev-parse --show-toplevel
```

The resolved top level becomes the cleanup root. Empty paths, `/`, non-git
directories, and paths that cannot be resolved to a real git worktree fail with a
visible error.

## External workflow worktree placement

`workflow-worktree.yaml` must avoid creating new worktrees inside the active
commit worktree. New workflow worktrees use this placement contract:

1. Reuse an existing Git worktree for the target branch when `git worktree list`
   reports one.
2. Use `REPO_PATH` directly when the caller is already on the target branch.
3. Otherwise create the new worktree outside the resolved repository root.

The external parent directory is resolved in this order:

| Precedence | Parent directory |
| --- | --- |
| 1 | `AMPLIHACK_WORKTREE_PARENT`, when set. Relative values resolve from the resolved repository root. |
| 2 | If the resolved repository root's parent directory is named `worktrees`, that parent directory. |
| 3 | Otherwise, `../worktrees` relative to the resolved repository root. |

The final worktree path is:

```text
<external-parent>/<branch-path-slug>
```

`branch-path-slug` is derived from the validated Git branch name by replacing
every character outside `[A-Za-z0-9._-]` with `-`, collapsing repeated `-`, and
appending a short deterministic hash when needed to avoid collisions. The branch
name itself remains unchanged for Git operations.

If the external parent cannot be created, is not writable, resolves inside the
active repository root, or would place the worktree under `.git/`, the workflow
fails closed. It must not silently fall back to `REPO_PATH/worktrees/...`.

Nested placement is only a compatibility path for worktrees that already exist
or for an explicitly requested emergency fallback with
`AMPLIHACK_ALLOW_NESTED_WORKTREE=1`. Any workflow-created nested worktree under
`worktrees/<name>/` must contain
`.amplihack-workflow-worktree` at the nested worktree root before any later
cleanup can remove it. Without that explicit variable, external placement
failure is a hard workflow failure.

## Safety contract

Cleanup is fail-closed. Before deleting anything, the helper verifies:

1. The target is a real git worktree or repository.
2. The resolved repository root is non-empty and not `/`.
3. Cleanup targets are inside the resolved repository root.
4. Cleanup targets are not symlinks.
5. No tracked files exist under `.claude/runtime/` or `worktrees/`.
6. Nested `worktrees/<name>/` directories contain `.amplihack-workflow-worktree`
   before they are removed.

If any check fails, cleanup exits nonzero and leaves the repository unchanged as
far as the failing target is concerned. The workflow then stops before
checkpoint, staging, pre-commit, publish, or finalization can hide the problem.

### Preserved files

The helper always preserves:

- tracked files
- `.claude/settings.json`
- `.claude/` itself
- untracked user files outside `.claude/runtime/` and marked nested worktrees
- unmarked or ambiguous `worktrees/` content

Unmarked nested worktrees are not guessed. They are reported as unsafe so the
owner can inspect them.

### Unsafe cleanup examples

These cases must fail nonzero and leave the target in place:

| Unsafe state | Required behavior |
| --- | --- |
| `.claude/runtime` is a symlink | Refuse cleanup. |
| `.claude/runtime/` contains any tracked file | Refuse cleanup. |
| `worktrees` is a symlink | Refuse cleanup. |
| `worktrees/<name>` is a symlink | Refuse cleanup. |
| `worktrees/<name>/` lacks `.amplihack-workflow-worktree` | Refuse cleanup. |
| `worktrees/<name>/.amplihack-workflow-worktree` is tracked | Refuse cleanup; tracked marker files do not prove workflow ownership. |
| `worktrees/<name>/` contains tracked content | Refuse cleanup. |
| `worktrees/<unmarked-sibling>/` exists beside a marked worktree | Remove only the marked safe target, then fail closed because ambiguous content remains. |

## Function reference

### `cleanup_known_workflow_runtime_artifacts`

```bash
cleanup_known_workflow_runtime_artifacts [repo]
```

Removes known workflow-generated artifacts from the resolved repository root.

| Behavior | Contract |
| --- | --- |
| Repository validation | Must pass before cleanup starts. |
| `.claude/runtime/` | The entire runtime directory is removed only when the path is not a symlink and contains no tracked files. Empty runtime directories are removed too because Artifact Guard treats the path itself as runtime leakage. |
| `.claude/settings.json` | Preserved because `.claude/` is never removed. |
| `worktrees/<name>/` | Removed only when marked with `.amplihack-workflow-worktree`, not symlinked, and free of tracked files. |
| Empty `worktrees/` | Removed after marked children are cleaned. |
| Ambiguous `worktrees/` | Fails nonzero when unmarked content remains. |
| Output | Logs path-level cleanup actions and failure reasons only. |

Exit codes:

| Code | Meaning |
| --- | --- |
| `0` | Known workflow artifacts were absent or cleaned safely. |
| nonzero | Repository resolution, path validation, tracked-file detection, symlink detection, or ownership-marker validation failed. |

### `preflight_known_workflow_runtime_artifacts`

```bash
preflight_known_workflow_runtime_artifacts [repo]
```

Runs cleanup first, then verifies that known workflow artifacts no longer remain
in a state that would trip Artifact Guard.

| Check | Result |
| --- | --- |
| Cleanup fails | Preflight fails with the cleanup error. |
| `.claude/runtime/` remains | Preflight fails. |
| Marked nested workflow worktree remains | Preflight fails. |
| Unmarked nested `worktrees/` content exists | Preflight fails closed instead of deleting it. |
| Unknown prohibited artifact exists | Preflight succeeds; Artifact Guard remains responsible for blocking it. |

Preflight is intentionally narrower than Artifact Guard. It prepares known
workflow-owned paths for strict scanning; it does not authorize other artifacts.

## Recipe lifecycle coverage

Recipes call `preflight_known_workflow_runtime_artifacts` immediately before any
phase that relies on a clean commit worktree:

| Recipe | Placement |
| --- | --- |
| `workflow-tdd.yaml` | Before checkpoint Artifact Guard and immediately before broad `git add -A` staging. |
| `workflow-precommit-test.yaml` | Immediately before pre-commit execution. |
| `workflow-publish.yaml` | Before publish Artifact Guard and before commit or push staging. |
| `workflow-finalize.yaml` | Before final Artifact Guard, final staging, and push cleanup. |
| `workflow-worktree.yaml` | Create new worktrees using the external placement contract. Reuse existing branch worktrees when available. Write `.amplihack-workflow-worktree` only for explicit nested compatibility worktrees. |

The required order around guard-sensitive phases is:

```bash
preflight_known_workflow_runtime_artifacts "$repo"
amplihack hygiene artifact-guard --repo "$repo" --mode all
git -C "$repo" add -A
```

Do not swap the first two lines. Artifact Guard should report unexpected
artifacts after workflow-owned runtime leftovers have been handled.

## Configuration

No feature flag is required. Runtime artifact cleanup is part of normal bundled
workflow execution.

| Configuration | Meaning |
| --- | --- |
| `WORKTREE_SETUP_WORKTREE_PATH` | Preferred workflow worktree path from setup steps. |
| `RECIPE_VAR_worktree_setup__worktree_path` | Recipe-runner flattened context value for the setup worktree. |
| `REPO_PATH` | Repository fallback when worktree setup context is absent. |
| `AMPLIHACK_WORKTREE_PARENT` | Optional explicit parent for new external workflow worktrees. |
| `AMPLIHACK_ALLOW_NESTED_WORKTREE` | Emergency compatibility switch. Only `1` permits creating a new nested workflow worktree, and the created directory still requires `.amplihack-workflow-worktree`. |
| `.amplihack-workflow-worktree` | Ownership marker required before deleting nested workflow-created `worktrees/<name>/`. |

The helper does not modify `NODE_OPTIONS`, `AMPLIHACK_HOME`,
`AMPLIHACK_AGENT_BINARY`, Git configuration, remotes, credentials, or allowlists.

## Regression expectations

Regression tests for workflow runtime artifacts belong in
`amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh`. Add a
clearly named group, such as `workflow-runtime-artifact-preflight`, that covers
these invariants:

| Scenario | Expected result |
| --- | --- |
| `.claude/runtime/` exists with untracked generated content | Preflight removes it before Artifact Guard. |
| `.claude/runtime/` exists but is empty | Preflight removes the empty directory. |
| `.claude/settings.json` exists | Preflight preserves it. |
| `.claude/runtime/` contains tracked content | Cleanup fails closed. |
| `.claude/runtime` is a symlink | Cleanup fails closed. |
| `worktrees/<name>/` contains `.amplihack-workflow-worktree` and no tracked files | Preflight removes it before Artifact Guard. |
| `worktrees/` or `worktrees/<name>/` is a symlink | Cleanup fails closed. |
| `.amplihack-workflow-worktree` is tracked | Cleanup fails closed. |
| `worktrees/<name>/` is unmarked | Cleanup fails closed; Artifact Guard still blocks if scanned. |
| New `workflow-worktree.yaml` worktree setup runs from a repository root | Output `worktree_path` is outside the resolved repository root unless an existing branch worktree is reused. |
| New `workflow-worktree.yaml` worktree setup runs from an existing `worktrees/<current>` worktree | Output `worktree_path` is a sibling under the existing external `worktrees` parent, not nested under the current repo root. |
| Unknown prohibited artifact such as `node_modules/` exists | Preflight does not remove it; Artifact Guard fails. |
| Target path is not a git worktree | Cleanup fails with a visible repository validation error. |

Use test names that expose the contract directly, for example:
`assert_runtime_preflight_removes_untracked_runtime`,
`assert_runtime_preflight_preserves_claude_settings`,
`assert_runtime_preflight_rejects_symlink_targets`,
`assert_runtime_preflight_rejects_tracked_nested_worktree_content`,
`assert_workflow_worktree_creates_external_sibling_path`, and
`assert_artifact_guard_still_blocks_unknown_node_modules`.

## Related documentation

- [Artifact Guard](../artifact-guard.md)
- [Preflight Workflow Runtime Artifacts](../howto/preflight-workflow-runtime-artifacts.md)
- [Tutorial: Workflow Runtime Artifact Isolation](../tutorials/workflow-runtime-artifact-isolation.md)
- [Workflow Execution Guardrails](workflow-execution-guardrails.md)
