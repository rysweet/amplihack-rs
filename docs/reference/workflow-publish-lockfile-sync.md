---
title: "Workflow Publish Lockfile Sync Reference"
description: "Contract for the offline Cargo.lock sync step that keeps the lockfile in sync with the bumped workspace version so --locked pre-commit gates pass."
last_updated: 2026-07-15
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Workflow Publish Lockfile Sync Reference

> [Home](../index.md) > Reference > Workflow Publish Lockfile Sync

The publish phase of `default-workflow` (recipe
`amplifier-bundle/recipes/workflow-publish.yaml`) bumps the workspace version in
`Cargo.toml` before it commits, pushes, and opens a pull request. Step
`step-14b-sync-lockfile` runs immediately after the version bump and
regenerates `Cargo.lock` from that new version using an offline, network-free
command. This keeps the lockfile in sync so the `--locked` pre-commit gates
pass deterministically instead of aborting the run.

## Problem it solves

`step-14-bump-version` rewrites the `version = "X.Y.Z"` line in the workspace
`Cargo.toml`. Without a matching lockfile update, `Cargo.lock` still records the
old version for every `amplihack-*` workspace crate. Every pre-commit gate in
`.pre-commit-config.yaml` — `artifact-guard`, `cargo-clippy`, and `cargo-test` —
runs Cargo with `--locked`. Cargo refuses to reconcile the drift and fails:

```text
error: cannot update the lock file ... because --locked was passed to prevent this
```

`step-15-commit-push` triggers those `--locked` gates through the pre-commit
hook that fires on `git commit`, so the mismatch made `step-15` fail on every
run, blocking pull-request creation for all workflows — not only Rust changes
that touched dependencies. See
[issue #915](https://github.com/rysweet/amplihack-rs/issues/915).

The fix keeps the lock **in sync**. It does not weaken or remove `--locked`, so
reproducible builds are preserved.

## Step contract

`step-14b-sync-lockfile` is a `bash` step positioned between
`step-14-bump-version` and `step-14g-artifact-guard`. It must satisfy the
following invariants:

| Invariant | Requirement |
| --- | --- |
| Ordering | `step-14-bump-version` < `step-14b-sync-lockfile` < `step-14g-artifact-guard` < `step-15-commit-push`. |
| Offline | Sync uses `cargo update --workspace --offline`. No network access is permitted. |
| Guarded | Runs only when both `Cargo.toml` and `Cargo.lock` exist; non-Rust workspaces are skipped, not errored. |
| Worktree resolution | Resolves the target tree with the same `WORKTREE_SETUP_WORKTREE_PATH` fallback chain as the sibling `step-14g-artifact-guard`. |
| No double-staging | The step does not `git add` the lockfile. `step-15-commit-push` stages it via `git add -A`. |
| Condition guard | Shares the same `condition` as its sibling steps so it only runs when publishing. |

### Command

```bash
set -euo pipefail
: "${WORKTREE_SETUP_WORKTREE_PATH:=${RECIPE_VAR_worktree_setup__worktree_path:-${REPO_PATH:-}}}"
cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-14b requires worktree_setup.worktree_path from step-04 (workflow-worktree); ensure parent recipe ran worktree-setup and propagated outputs}"
if [ -f Cargo.toml ] && [ -f Cargo.lock ]; then
  echo "step-14b: syncing Cargo.lock to bumped version (offline)"
  cargo update --workspace --offline
  git diff --stat -- Cargo.lock || true
else
  echo "step-14b: no Cargo.toml/Cargo.lock present; skipping lockfile sync (non-Rust workspace)"
fi
```

### Why `--offline`

`cargo update --workspace --offline` re-resolves only the workspace members and
writes their bumped versions into `Cargo.lock` using packages already present in
the local Cargo cache. `--offline` forbids any network fetch, so the step cannot
silently upgrade transitive dependencies or reach out to a registry. This is the
primary supply-chain control: the lockfile diff is limited to the
`amplihack-*` workspace crate versions and remains reviewable in the pull
request.

`--workspace` scopes the resolution to the workspace so the lockfile rewrite
surface stays minimal. No `--precise` or unpinned upgrade flags are used, so
existing dependency pins are not disturbed.

## Configuration

The step needs no feature-specific flag. It uses the standard publish context:

| Context key / variable | Required | How the step uses it |
| --- | --- | --- |
| `worktree_setup.worktree_path` (via `WORKTREE_SETUP_WORKTREE_PATH`) | Yes | Directory to `cd` into before running the offline update. Populated by the `workflow-worktree` sub-recipe (step-04). |
| `RECIPE_VAR_worktree_setup__worktree_path` | Fallback | Secondary source for the worktree path. |
| `REPO_PATH` | Fallback | Final fallback if the worktree path is unavailable. |
| `condition` (`goal_already_met != 'true' && terminal_state.terminal_success != 'true' && terminal_state.should_publish == 'true'`) | Yes | Gates the step to publish-only runs, matching its sibling steps verbatim. |

## Behavior on non-Rust workspaces

If either `Cargo.toml` or `Cargo.lock` is absent, the step prints a skip message
and exits `0`. Repositories that are not Cargo workspaces continue through the
publish phase unchanged.

## Examples

### Rust workspace (lockfile synced)

```text
step-14b: syncing Cargo.lock to bumped version (offline)
 Cargo.lock | 8 ++++----
 1 file changed, 4 insertions(+), 4 deletions(-)
```

`step-15-commit-push` then runs `git add -A`, staging the updated `Cargo.lock`
alongside the bumped `Cargo.toml`, and the `--locked` gates pass.

### Non-Rust workspace (skipped)

```text
step-14b: no Cargo.toml/Cargo.lock present; skipping lockfile sync (non-Rust workspace)
```

## Non-goals

- The step does not weaken, remove, or bypass any `--locked` flag in
  `.pre-commit-config.yaml`. Reproducible builds remain enforced.
- The step does not fetch from the network. Offline resolution failures must
  surface loudly rather than fall back to an online update.
- The step does not stage `Cargo.lock` itself; staging is owned by
  `step-15-commit-push` via `git add -A`.
- The step does not upgrade transitive dependencies; `--workspace --offline`
  limits the rewrite to workspace crate versions.

## Regression expectations

Tests covering this step should assert the semantic contract, not exact prose:

- `step-14b-sync-lockfile` exists in `workflow-publish.yaml`.
- Step ordering holds:
  `step-14-bump-version` < `step-14b-sync-lockfile` < `step-14g-artifact-guard`.
- The recipe text contains `cargo update --workspace --offline`.
- The lockfile-existence guard (`[ -f Cargo.toml ] && [ -f Cargo.lock ]`) is
  present so non-Rust workspaces are skipped.
- No `--locked` flag is removed from `.pre-commit-config.yaml`.

The integration test
`tests/integration/workflow_publish_terminal_gate.rs` encodes these assertions
using the shared `load_publish_recipe()` and `step_index()` helpers.

## See also

- [Default Workflow Step 13 Validation Reference](default-workflow-step-13-validation.md)
- [Workflow Terminal-State Reference](workflow-terminal-state.md)
- [Worktree Setup Propagation Reference](worktree-setup-propagation.md)
