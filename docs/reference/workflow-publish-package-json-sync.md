---
title: "Workflow Publish package.json Sync Reference"
description: "Contract for the offline package.json version sync step that keeps the root package.json version in sync with the bumped workspace version so the CI version-contract test passes."
last_updated: 2026-07-15
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Workflow Publish package.json Sync Reference

> [Home](../index.md) > Reference > Workflow Publish package.json Sync

The publish phase of `default-workflow` (recipe
`amplifier-bundle/recipes/workflow-publish.yaml`) bumps the workspace version in
`Cargo.toml` before it commits, pushes, and opens a pull request. Step
`step-14c-sync-package-json` runs after the version bump (and after
`step-14b-sync-lockfile`) and rewrites the root `package.json` `version` field to
match the bumped `[workspace.package].version`. It reads the version offline and
edits the JSON structurally so the CI version-contract test passes
deterministically instead of failing on stale version drift.

## Problem it solves

`step-14-bump-version` rewrites the `version = "X.Y.Z"` line in the workspace
`Cargo.toml`. `step-14b-sync-lockfile` keeps `Cargo.lock` in sync, but nothing
synced the root `package.json` `version`. The contract test
`package_json_version_matches_root_workspace_version` requires
`package.json.version == [workspace.package].version`, so any bump left
`package.json` stale and CI Test failed deterministically (exit 100). See
[issue #925](https://github.com/rysweet/amplihack-rs/issues/925).

The fix keeps `package.json` **in sync** with the authoritative Cargo workspace
version. It does not change the version contract or the source of truth.

## Step contract

`step-14c-sync-package-json` is a `bash` step positioned between
`step-14b-sync-lockfile` and `step-14g-artifact-guard`. It must satisfy the
following invariants:

| Invariant | Requirement |
| --- | --- |
| Ordering | `step-14-bump-version` < `step-14b-sync-lockfile` < `step-14c-sync-package-json` < `step-14g-artifact-guard` < `step-15-commit-push`. |
| Offline | The version is read offline. The scoped `awk` parse touches no network; the `cargo metadata` fallback uses `--offline`. |
| Robust read | Reads `[workspace.package].version` with a **scoped** `[workspace.package]` `awk` parse (never an unscoped `grep` of `version`, which the version-contract tests forbid), falling back to `cargo metadata --offline` only when the scoped parse is empty. |
| Robust edit | Edits `package.json` with `jq` (or a `python3` fallback) — never `sed`/`grep` on JSON. Preserves 2-space indentation and a trailing newline. |
| Atomic write | Both write paths write to a **same-directory** temp file and atomically replace `package.json` (`mv`/`os.replace`), so a mid-write crash cannot truncate or corrupt the file. |
| Idempotent | Safe to run when no bump occurred; re-running produces no change. |
| Guarded | Runs only when `package.json` and `Cargo.toml` both exist; non-JS / non-Rust workspaces are skipped (`exit 0`), not errored. |
| Worktree resolution | Resolves the target tree with the same `WORKTREE_SETUP_WORKTREE_PATH` fallback chain as the sibling `step-14b`/`step-14g` steps. |
| No double-staging | The step does not `git add` `package.json`. `step-15-commit-push` stages it via `git add -A`. |
| Condition guard | Shares the same `condition` as its sibling steps so it only runs when publishing. |

### Command

```bash
set -euo pipefail
: "${WORKTREE_SETUP_WORKTREE_PATH:=${RECIPE_VAR_worktree_setup__worktree_path:-${REPO_PATH:-}}}"
cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-14c requires worktree_setup.worktree_path from step-04 (workflow-worktree); ensure parent recipe ran worktree-setup and propagated outputs}"
if [ ! -f package.json ]; then
  echo "step-14c: no package.json present; skipping version sync (non-JS workspace)"
  exit 0
fi
if [ ! -f Cargo.toml ]; then
  echo "step-14c: no Cargo.toml present; cannot resolve workspace version; skipping"
  exit 0
fi
# Scoped [workspace.package] parse (cheapest-first, ~1ms), never an unscoped grep.
WS_VERSION=$(awk '
  /^\[workspace\.package\]/ { in_section = 1; next }
  /^\[/ { in_section = 0 }
  in_section && /^[[:space:]]*version[[:space:]]*=/ {
    line = $0; sub(/^[^"]*"/, "", line); sub(/".*$/, "", line); print line; exit
  }
' Cargo.toml)
if [ -z "$WS_VERSION" ] && command -v cargo >/dev/null 2>&1 && command -v jq >/dev/null 2>&1; then
  WS_VERSION=$(cargo metadata --no-deps --format-version=1 --offline 2>/dev/null \
    | jq -r '[.packages[] | select(.source == null) | .version] | unique | .[0] // empty' \
    2>/dev/null || true)
fi
if [ -z "$WS_VERSION" ]; then
  echo "ERROR: step-14c could not resolve [workspace.package].version from Cargo.toml" >&2
  exit 1
fi
echo "step-14c: syncing package.json version to workspace version ${WS_VERSION} (offline)"
if command -v jq >/dev/null 2>&1; then
  _tmp=$(mktemp ./.package.json.XXXXXX)
  jq --arg v "$WS_VERSION" '.version = $v' package.json > "$_tmp"
  mv "$_tmp" package.json
elif command -v python3 >/dev/null 2>&1; then
  WS_VERSION="$WS_VERSION" python3 -c 'import json, os, tempfile; p = "package.json"; d = json.load(open(p, encoding="utf-8")); d["version"] = os.environ["WS_VERSION"]; fd, tmp = tempfile.mkstemp(dir=os.path.dirname(os.path.abspath(p)) or ".", prefix=".package.json."); f = os.fdopen(fd, "w", encoding="utf-8"); json.dump(d, f, indent=2); f.write("\n"); f.close(); os.replace(tmp, p)'
else
  echo "ERROR: step-14c needs jq or python3 to edit package.json safely (no sed on JSON)" >&2
  exit 2
fi
```

### Why the scoped read

`step-14` bumps exactly `[workspace.package].version`, so a **scoped** parse of
that TOML table reads the authoritative value in a single ~1ms pass. This is
cheapest-first: the near-free `awk` parse runs before the more expensive
`cargo metadata` subprocess (which spawns cargo and resolves the whole workspace
graph). `cargo metadata --offline` is retained only as a fallback for edge cases
such as single-quoted TOML literals. An unscoped `grep` of `version` is
explicitly forbidden by the version-contract tests because it could match an
unrelated dependency's version.

### Why the atomic, structural edit

`package.json` is edited with `jq` (or `python3` as a fallback), never `sed` or
`grep`, so the JSON is always re-serialized from a parsed structure. Both write
paths write to a **same-directory** temp file and then atomically replace
`package.json` via `mv` (`jq` path) or `os.replace` (`python3` path). A
same-directory temp keeps the replace on one filesystem so it is a true atomic
rename rather than a cross-device copy+unlink; a crash mid-write therefore
cannot truncate or corrupt `package.json` in the publish path. Both paths
preserve 2-space indentation and a trailing newline.

## Configuration

The step needs no feature-specific flag. It uses the standard publish context:

| Context key / variable | Required | How the step uses it |
| --- | --- | --- |
| `worktree_setup.worktree_path` (via `WORKTREE_SETUP_WORKTREE_PATH`) | Yes | Directory to `cd` into before reading/writing. Populated by the `workflow-worktree` sub-recipe (step-04). |
| `RECIPE_VAR_worktree_setup__worktree_path` | Fallback | Secondary source for the worktree path. |
| `REPO_PATH` | Fallback | Final fallback if the worktree path is unavailable. |
| `condition` (`goal_already_met != 'true' && terminal_state.terminal_success != 'true' && terminal_state.should_publish == 'true'`) | Yes | Gates the step to publish-only runs, matching its sibling steps verbatim. |

## Behavior on non-JS / non-Rust workspaces

If `package.json` is absent, the step prints a skip message and exits `0`. If
`Cargo.toml` is absent (no workspace version to resolve), it likewise skips and
exits `0`. Repositories that are not JS-and-Cargo workspaces continue through
the publish phase unchanged.

## Examples

### JS + Rust workspace (version synced)

```text
step-14c: syncing package.json version to workspace version 0.11.1 (offline)
```

`step-15-commit-push` then runs `git add -A`, staging the updated `package.json`
alongside the bumped `Cargo.toml` and `Cargo.lock`, and the CI version-contract
test passes.

### Non-JS workspace (skipped)

```text
step-14c: no package.json present; skipping version sync (non-JS workspace)
```

## Non-goals

- The step does not change the source of truth. `[workspace.package].version`
  in `Cargo.toml` remains authoritative; `package.json` is derived from it.
- The step does not use the network. Offline read failures surface loudly
  (`exit 1`) rather than falling back to an online lookup.
- The step does not edit JSON with `sed`/`grep`; structural editors (`jq` /
  `python3`) are required.
- The step does not stage `package.json` itself; staging is owned by
  `step-15-commit-push` via `git add -A`.

## Regression expectations

Tests covering this step should assert the semantic contract, not exact prose:

- `step-14c-sync-package-json` exists in `workflow-publish.yaml`.
- Step ordering holds:
  `step-14-bump-version` < `step-14b-sync-lockfile` <
  `step-14c-sync-package-json` < `step-14g-artifact-guard` < `step-15-commit-push`.
- The version read is scoped to `[workspace.package]` (no unscoped `grep` of
  `version`) with an `--offline` `cargo metadata` fallback.
- The JSON edit uses `jq`/`python3` (never `sed`) and writes atomically via a
  same-directory temp file.
- The existence guards (`package.json`, `Cargo.toml`) are present so
  non-JS / non-Rust workspaces are skipped.
- The step shares the publish `condition` with its sibling steps.

The integration test
`tests/integration/workflow_publish_terminal_gate.rs` encodes these assertions
using the shared `load_publish_recipe()` and `step_index()` helpers
(`publish_syncs_package_json_version_after_bump_before_locked_gates`).

## See also

- [Workflow Publish Lockfile Sync Reference](workflow-publish-lockfile-sync.md)
- [Default Workflow Step 13 Validation Reference](default-workflow-step-13-validation.md)
- [Workflow Terminal-State Reference](workflow-terminal-state.md)
- [Worktree Setup Propagation Reference](worktree-setup-propagation.md)
