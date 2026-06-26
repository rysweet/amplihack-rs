# Workflow Runtime Artifacts Reference

This reference defines the runtime-root, cleanup, and guard-point contracts used
by `default-workflow`, recovery flows, launchers, provenance logging, publish
helpers, and finalization helpers.

Updated: 2026-06-25

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
| `AMPLIHACK_AGENT_BINARY` | Propagated | Active agent runtime for child agentic steps. |
| `AMPLIHACK_NONINTERACTIVE` | Propagated | Forces child workflow and helper processes into non-interactive mode. |

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

Process metadata for nested agents, workstreams, provider helper calls, and
simulation runs is stored under the runtime root. Recipes use that metadata for
scoped closure and finalization instead of relying on host-global process lists
or stale PID-only records.

## Known worktree-local artifacts

Only these worktree-local paths are classified as known workflow runtime
artifacts:

| Repo-relative path | Owner | Cleanup behavior |
| --- | --- | --- |
| `.claude/runtime` | legacy amplihack launcher and nested agent runtime state | Removed by workflow runtime preflight when present. |
| `worktrees` | reserved root for amplihack workflow-created nested scratch worktrees under a managed task worktree | Nested git worktrees are deregistered (`git worktree remove --force` + `git worktree prune`) and the directory is removed by workflow runtime preflight when present and not tracked source. A bare `rm -rf` is never sufficient: it leaves dangling worktree registrations behind (issue #808 — a regression of the #780/#755 local-leak fixes). |

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

### Helper path resolution

The helper is not required to live under the target repository. Recipe lifecycle
steps resolve `workflow_runtime_artifacts.sh` by checking these locations in
order and using the first that exists:

| Priority | Candidate | Purpose |
| --- | --- | --- |
| 1 | `$WORKFLOW_RUNTIME_ARTIFACT_HELPER` | Explicit override (publish/tool steps). |
| 2 | `$AMPLIHACK_HOME/amplifier-bundle/tools/...` | Honor an explicit Amplihack home. |
| 3 | `$REPO_PATH/amplifier-bundle/tools/...` | Target repo checkout that bundles the tools. |
| 4 | `$(pwd)/amplifier-bundle/tools/...` | Active worktree that bundles the tools. |
| 5 | `$HOME/.copilot/amplifier-bundle/tools/...` | Copilot-installed bundle. |
| 6 | `$HOME/.amplihack/amplifier-bundle/tools/...` | Default installed Amplihack bundle. |

When `AMPLIHACK_HOME` is unset and the target repository does not vendor an
`amplifier-bundle/` directory, the helper still resolves from the installed
`~/.amplihack` (or `~/.copilot`) bundle. The step fails with a clear error only
when no candidate resolves. This prevents a successful implementation from being
reported as failed at `checkpoint-after-implementation` (issue #817).


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

### Deterministic finalization cleanup (issue #808)

When a `default-workflow` run hits a denied force-push, its push-fallback path
could spray throwaway branches to the shared remote and leave nested worktrees
behind, with no finalization cleanup to remove them. The helper exposes a
deterministic, idempotent, fail-soft cleanup for that leak class. Every function
returns success even when individual `git` operations fail, so it is safe to
call from success and failure/early-exit paths (for example an `EXIT` trap). The
intended PR branch, any branch still checked out in a worktree, and the
protected base branches (`main`, `master`, `develop`) are never deleted.

#### `record_run_created_branch`

```bash
record_run_created_branch "$repo" "$fallback_branch"
```

Tracks a run-created fallback/intermediate branch in a per-run manifest stored
under the shared git common dir
(`$(git rev-parse --git-common-dir)/amplihack/run-created-branches-<run-id>`,
scoped by `AMPLIHACK_RECIPE_RUN_ID` so concurrent runs never consume each
other's entries). Deduplicated and idempotent. Whenever the workflow pushes a
branch other than the intended PR branch, it records it here so finalization
can delete it.

#### `cleanup_run_created_branches`

```bash
cleanup_run_created_branches "$repo" "$intended_branch"
```

Deletes every manifest-tracked branch from the shared remote
(`git push origin --delete`) and locally (`git branch -D`), then consumes the
manifest. Preserves the intended/checked-out/protected branches.

#### `cleanup_nested_worktrees`

```bash
cleanup_nested_worktrees "$repo"
```

Removes every registered git worktree that lives under `<repo>/worktrees/` with
`git worktree remove --force` and prunes dangling registrations with
`git worktree prune`. Operating on the per-task worktree, this only touches
worktrees nested **inside** it — never the task worktree itself, a sibling task
worktree, or the main worktree.

#### `finalize_workflow_runtime_artifacts`

```bash
finalize_workflow_runtime_artifacts "$task_worktree" "$intended_branch"
```

Finalization entry point. It captures the branches of nested worktrees, removes
the nested worktrees, deletes those now-orphaned branches plus any
manifest-tracked fallback branches from the shared remote and locally, and then
runs the narrow runtime-artifact sweep. **Call it with a dedicated per-task
worktree** as `<repo>`: nested-worktree removal only touches `worktrees/`
children of that worktree, never the worktree itself, a sibling task worktree,
or the main worktree.

#### `finalize_workflow_cleanup_entry`

```bash
finalize_workflow_cleanup_entry "$repo_root" "$worktree_path" "$intended_branch"
```

Recipe-facing entry point (invoked from `workflow_agentic_finalization.sh
collect`). It runs `finalize_workflow_runtime_artifacts` only when
`worktree_path` is a *linked* worktree whose canonical toplevel differs from the
repo root; otherwise it runs only the manifest-keyed branch cleanup (which never
removes a worktree). Deleted branches never include the intended PR branch, a
checked-out branch, or a protected/default branch.

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
| `workflow-finalize.yaml` | Before final status gates and final broad staging. The deterministic finalization cleanup runs inside `workflow_agentic_finalization.sh collect` (invoked unconditionally by `collect-finalization-evidence`) via `finalize_workflow_cleanup_entry`, deleting run-created fallback branches (remote + local) and removing nested worktrees before evidence is collected. |
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
7. Child workflows inherit `AMPLIHACK_RUNTIME_ROOT`, `AMPLIHACK_AGENT_BINARY`,
   and non-interactive settings without recomputing or leaking runtime state into
   the commit worktree.
8. Lifecycle steps resolve `workflow_runtime_artifacts.sh` from the installed
   `~/.amplihack` (or `~/.copilot`) bundle when `AMPLIHACK_HOME` is unset and the
   target repository has no `amplifier-bundle/` directory (issue #817).
9. Finalization deletes run-created fallback branches from the shared remote and
   locally, removes nested worktrees without leaving dangling worktree
   registrations, deletes the nested worktree's orphaned branch (remote + local),
   and always preserves the intended PR branch and protected base branches
   (issue #808). The cleanup is idempotent and never aborts the caller.
