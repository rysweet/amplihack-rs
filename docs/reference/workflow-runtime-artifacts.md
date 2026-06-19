# Workflow Runtime Artifacts Reference

This reference defines the runtime-root, cleanup, and guard-point contracts used
by `default-workflow`, recovery flows, launchers, provenance logging, publish
helpers, and finalization helpers.

Status: Planned implementation contract.
Updated: 2026-06-18

## Contents

- [Environment variables](#environment-variables)
- [Runtime root resolution](#runtime-root-resolution)
- [Runtime directory layout](#runtime-directory-layout)
- [Known worktree-local artifacts](#known-worktree-local-artifacts)
- [Shell helper API](#shell-helper-api)
- [Lifecycle integration](#lifecycle-integration)
- [Artifact Guard interaction](#artifact-guard-interaction)
- [Regression contract](#regression-contract)

## Environment variables

| Variable | Direction | Meaning |
| --- | --- | --- |
| `AMPLIHACK_RUNTIME_ROOT` | Read and propagated | Absolute directory for workflow-generated runtime files. |
| `AMPLIHACK_RECIPE_RUN_ID` | Set by `amplihack recipe run` | Stable per-run ID used in default runtime-root paths and log correlation. |
| `XDG_RUNTIME_DIR` | Read | Preferred platform runtime base when `AMPLIHACK_RUNTIME_ROOT` is unset. |
| `TMPDIR` | Read indirectly | Host temporary directory preference for tools that create additional scratch output. |

`AMPLIHACK_RUNTIME_ROOT` is treated only as a filesystem path. Recipes and shell
helpers quote it and never evaluate it as shell code.

The top-level workflow establishes `AMPLIHACK_RUNTIME_ROOT` once. Child
workflows, nested recipes, and agents inherit it unchanged; they must not run
runtime-root resolution again and create sibling roots.

## Runtime root resolution

The shared resolver returns one runtime root for a workflow run.

| Priority | Source | Example |
| --- | --- | --- |
| 1 | `AMPLIHACK_RUNTIME_ROOT` | `/var/tmp/amplihack-runtime/alice/run-123` |
| 2 | `$XDG_RUNTIME_DIR/amplihack/runtime/<run-id>` | `/run/user/1000/amplihack/runtime/550e8400-e29b-41d4-a716-446655440000` |
| 3 | `/tmp/amplihack-runtime/<user>/<run-id>` | `/tmp/amplihack-runtime/alice/550e8400-e29b-41d4-a716-446655440000` |

The resolver rejects empty paths, root paths, and paths that cannot be created.
Workflow-managed defaults are outside Git worktrees and are created with
restrictive owner-only permissions where the platform supports them. If a caller
explicitly sets `AMPLIHACK_RUNTIME_ROOT`, they are responsible for selecting a
private external path.

## Runtime directory layout

The resolver creates these subdirectories before child workflows or agents run:

```text
<runtime-root>/
  locks/
  logs/
  metrics/
  provenance/
  reflection/
```

Launchers use the runtime root for generated launcher state. Provenance logging
uses the `provenance/`, `logs/`, `metrics/`, and `reflection/` directories.
Recipe helpers may add run-scoped files under these directories, but must not
write generated runtime output into the task worktree.

Launcher context files used for active-agent routing belong under the runtime
root. `<worktree>/.claude/runtime/launcher_context.json` is legacy fallback
state only and must not be treated as canonical by new workflow code.

## Known worktree-local artifacts

Only these worktree-local paths are classified as known workflow runtime
artifacts:

| Repo-relative path | Owner | Cleanup behavior |
| --- | --- | --- |
| `.claude/runtime` | legacy amplihack launcher and nested agent runtime state | Removed by workflow runtime preflight when present. |
| `worktrees` | reserved root for amplihack workflow-created nested scratch worktrees under a managed task worktree | Removed by workflow runtime preflight when present and not tracked source. |

The cleanup contract does not include `.claude` itself, `.claude/settings.json`,
source files, build output, dependency directories, logs outside the runtime
root, or arbitrary untracked files.

Root-level `worktrees/` is reserved in amplihack-managed task worktrees. If a
repository intentionally tracks source under that path, the workflow must fail
closed and require a repository layout change instead of deleting it.

## Shell helper API

The helper script lives at:

```text
amplifier-bundle/tools/workflow_runtime_artifacts.sh
```

Source it from recipe shell steps or workflow helper scripts:

```bash
. "$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
```

### `cleanup_known_workflow_runtime_artifacts`

```bash
cleanup_known_workflow_runtime_artifacts "$worktree"
```

Removes the known workflow runtime artifact paths if they exist. Missing known
paths are a successful no-op. For each existing target, the helper:

1. Requires a non-empty worktree path.
2. Requires the path to resolve to a Git worktree.
3. Resolves the worktree and existing target paths before deletion.
4. Rejects tracked source paths, symlink escapes, and targets outside the
   worktree.
5. Deletes only exact matches for `.claude/runtime` and `worktrees` under the
   active worktree.
6. Fails non-zero if validation or deletion fails.

### `preflight_known_workflow_runtime_artifacts`

```bash
preflight_known_workflow_runtime_artifacts "$worktree"
```

Runs cleanup, then verifies that the known runtime artifact paths are absent.
Use this function before guard-sensitive lifecycle operations.

## Lifecycle integration

The bundled workflows and helper scripts call
`preflight_known_workflow_runtime_artifacts` before lifecycle operations that
would otherwise fail on workflow-owned leftovers.

| File | Required preflight points |
| --- | --- |
| `workflow-worktree.yaml` | Establish `AMPLIHACK_RUNTIME_ROOT` for the top-level managed task worktree and propagate it unchanged to child workflows. |
| `workflow-tdd.yaml` | Before checkpoint and broad staging. |
| `workflow-refactor-review.yaml` | Before checkpoint and broad staging. |
| `workflow-pr-review.yaml` | Before checkpoint and broad staging. |
| `workflow-publish.yaml` | Before dirty checks, publish staging, commit, push, and PR creation. |
| `workflow-finalize.yaml` | Before final status gates and final broad staging. |
| `workflow_publish_pr.sh` | Before dirty-worktree checks and broad staging. |
| `workflow_final_status.sh` | Before final clean-worktree validation. |

Any new recipe step that invokes `git add -A`, performs publication staging, or
requires a clean worktree must run the same preflight immediately before the
operation.

## Artifact Guard interaction

Artifact Guard is still the authority for unexpected artifacts. It does not
delete files and it is not weakened for workflow runtime isolation.

The order is:

```text
isolate runtime output outside worktree
preflight known workflow runtime leftovers
run strict Artifact Guard or clean-worktree gate
stage, commit, publish, or finalize
```

If `.claude/runtime` or `worktrees/` remains after preflight, the lifecycle step
fails visibly. If any unrelated artifact remains, Artifact Guard fails visibly.

## Regression contract

Regression coverage for workflow runtime isolation verifies:

1. Creating `.claude/runtime` under a task worktree no longer breaks later
   checkpoint, staging, publish, or finalization gates because preflight removes
   the known workflow-owned path.
2. Creating `worktrees/` under a task worktree no longer breaks later lifecycle
   gates because preflight removes the known workflow-owned path.
3. User-authored `.claude` files, including `.claude/settings.json`, remain
   untouched.
4. Unrelated untracked files still fail strict guard behavior.
5. Runtime-root resolution honors `AMPLIHACK_RUNTIME_ROOT` and otherwise chooses
   an external default path.
6. Launcher state and provenance output are written under the shared runtime
   root contract.
