---
title: "Workflow Publish package.json Version Sync Reference"
description: "Contract for the offline step that syncs root package.json version to the bumped [workspace.package].version so the version-contract test and CI Test job pass."
last_updated: 2026-07-15
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Workflow Publish package.json Version Sync Reference

> [Home](../index.md) > Reference > Workflow Publish package.json Version Sync

The publish phase of `default-workflow` (recipe
`amplifier-bundle/recipes/workflow-publish.yaml`) bumps the workspace version in
`Cargo.toml` before it commits, pushes, and opens a pull request. Step
`step-14c-sync-package-json` runs immediately after `step-14b-sync-lockfile` and
rewrites the root `package.json` `version` field to equal the
`[workspace.package].version` value read directly from `Cargo.toml`. It uses an
offline, network-free, deterministic version read and a robust JSON edit so the
version-contract test — and therefore the CI `Test` job — pass deterministically
instead of failing on stale metadata.

## Problem it solves

`step-14-bump-version` rewrites the `version = "X.Y.Z"` line under
`[workspace.package]` in the workspace `Cargo.toml`. `step-14b-sync-lockfile`
keeps `Cargo.lock` in sync, but **nothing** updated the root `package.json`
`version`. That file must stay equal to the workspace version because the
integration contract test
`package_json_version_matches_root_workspace_version` in
`tests/integration/version_contract_test.rs` asserts:

```text
package.json.version == [workspace.package].version
```

After any bump, `package.json` was left stale, so the contract test failed and
the CI `Test` job aborted with exit code `100`. The pre-commit gates did not
catch the drift, so the failure only surfaced in CI after the pull request was
opened. See
[issue #925](https://github.com/rysweet/amplihack-rs/issues/925).

The fix keeps `package.json` **in sync** with the workspace version on every
publish. It does not weaken any `--locked` gate and does not bump the version
itself — it only mirrors the value already written by `step-14-bump-version`.

## Step contract

`step-14c-sync-package-json` is a `bash` step positioned between
`step-14b-sync-lockfile` and `step-14g-artifact-guard`. It must satisfy the
following invariants:

| Invariant | Requirement |
| --- | --- |
| Ordering | `step-14-bump-version` < `step-14b-sync-lockfile` < `step-14c-sync-package-json` < `step-14g-artifact-guard` < `step-15-commit-push`. |
| Offline | The workspace version is read locally with no network access. The read never fetches from crates.io or a git remote. |
| Robust version read | Reads `[workspace.package].version` via a **scoped** `[workspace.package]` TOML parse — the exact field the contract test compares against — so the result is deterministic even when workspace members carry divergent versions. No unscoped `grep` on `Cargo.toml`. |
| Robust JSON edit | Sets `package.json` `.version` via `jq` (with a `python3`/`node` fallback), never `sed`/`grep` on JSON. Preserves indentation and a trailing newline. |
| Guarded | Runs only when `package.json` exists; non-JS workspaces are skipped, not errored. Also skips cleanly (`exit 0`) if the version cannot be read. |
| Idempotent | Safe to run when the value already matches; re-running produces no diff. |
| Worktree resolution | Resolves the target tree with the same `WORKTREE_SETUP_WORKTREE_PATH` fallback chain as the sibling `step-14b-sync-lockfile`. |
| No double-staging | The step does not `git add` `package.json`. `step-15-commit-push` stages it via `git add -A`. |
| Condition guard | Shares the same `condition` as its sibling steps so it only runs when publishing. |
| No `--locked` change | The step never weakens, removes, or bypasses a `--locked` flag. |

### Command

```bash
set -euo pipefail
: "${WORKTREE_SETUP_WORKTREE_PATH:=${RECIPE_VAR_worktree_setup__worktree_path:-${REPO_PATH:-}}}"
cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-14c requires worktree_setup.worktree_path from step-04 (workflow-worktree); ensure parent recipe ran worktree-setup and propagated outputs}"

# Issue #925: step-14 bumps [workspace.package].version in Cargo.toml and
# step-14b keeps Cargo.lock in sync, but the root package.json "version" was
# left stale. The version-contract test requires
# package.json.version == [workspace.package].version, so every bump broke the
# CI Test job (exit 100). Sync package.json here (offline, robust JSON edit).
# Skip gracefully on non-JS workspaces. Do NOT bump; only mirror Cargo.toml.

if [ ! -f package.json ]; then
  echo "step-14c: no package.json present; skipping version sync (non-JS workspace)"
  exit 0
fi

# Robust, offline, deterministic version read from [workspace.package].version.
# This is the exact field the version-contract test compares against, so read it
# directly with a SCOPED parse of the [workspace.package] table (never an
# unscoped grep that could match a dependency's version line).
VERSION=""
if [ -f Cargo.toml ]; then
  VERSION=$(awk '
    /^\[workspace\.package\]/ {inpkg=1; next}
    /^\[/ {inpkg=0}
    inpkg && /^[[:space:]]*version[[:space:]]*=/ {
      gsub(/.*=[[:space:]]*"?/, ""); gsub(/".*/, ""); print; exit
    }' Cargo.toml)
fi
# Fallback: if the scoped parse yielded nothing (e.g. a non-standard layout),
# resolve the root crate offline via cargo metadata. Selecting the workspace
# root package by its resolved id keeps this deterministic across
# multi-version workspaces.
if [ -z "$VERSION" ] && command -v cargo >/dev/null 2>&1 && command -v jq >/dev/null 2>&1; then
  VERSION=$(cargo metadata --no-deps --format-version=1 --offline 2>/dev/null \
    | jq -r '.resolve.root as $r | (.packages[] | select(.id == $r) | .version) // empty')
fi

if [ -z "$VERSION" ]; then
  echo "step-14c: could not read [workspace.package].version; skipping package.json sync"
  exit 0
fi

echo "step-14c: syncing package.json version to $VERSION (offline)"
if command -v jq >/dev/null 2>&1; then
  # Temp file in the SAME directory so `mv` is an atomic same-fs rename;
  # preserve the original file mode (e.g. 0644) so CI can still read it.
  tmp=$(mktemp ./package.json.XXXXXX)
  trap 'rm -f "$tmp"' EXIT
  chmod --reference=package.json "$tmp" 2>/dev/null || true
  jq --arg v "$VERSION" '.version = $v' package.json > "$tmp"  # jq emits a trailing newline
  mv "$tmp" package.json
  trap - EXIT
else
  # Same atomic contract via os.replace on a same-dir temp file.
  VERSION="$VERSION" python3 -c 'import json, os, tempfile
p = "package.json"
d = json.load(open(p, encoding="utf-8"))
d["version"] = os.environ["VERSION"]
_dir = os.path.dirname(os.path.abspath(p))
_fd, _tmp = tempfile.mkstemp(dir=_dir, prefix="package.json.")
os.write(_fd, (json.dumps(d, indent=2) + "\n").encode("utf-8"))
os.close(_fd)
os.chmod(_tmp, os.stat(p).st_mode & 0o7777)
os.replace(_tmp, p)'
fi
git diff --stat -- package.json || true
```

### Why a scoped `[workspace.package]` parse

The version-contract test compares `package.json.version` against the literal
`[workspace.package].version` field in `Cargo.toml`, so the step reads *that
exact field* as its source of truth. A **scoped** `awk` parse enters only the
`[workspace.package]` table (resetting on the next `[section]` header) and reads
its `version` key. This is:

- **Deterministic** — it returns a single, unambiguous value even if individual
  workspace members declare divergent explicit versions. (An earlier design read
  `cargo metadata | jq '[.packages[] | select(.source == null) | .version]'`,
  but `select(.source == null)` matches *all* local members, so `unique | .[0]`
  is nondeterministic when members diverge. Reading `[workspace.package].version`
  directly avoids that ambiguity entirely.)
- **Offline** — it touches only the local `Cargo.toml`; no crates.io or git
  fetch occurs, consistent with `step-14b-sync-lockfile`'s
  `cargo update --workspace --offline`.
- **Scoped** — it never runs an unscoped `grep 'version'` that could match a
  dependency's version line.

If the scoped parse yields nothing (non-standard layout, or `Cargo.toml`
missing), the step falls back to `cargo metadata --no-deps --format-version=1
--offline`, selecting the **workspace root package by id**
(`.resolve.root`) so the fallback stays deterministic and offline. If neither
path produces a version, the step skips cleanly (`exit 0`).

### Why `jq` for the JSON edit

`package.json` is JSON, so it is edited as JSON. `jq --arg v "$VERSION"
'.version = $v'` sets the field structurally, treating the version as data
(never string-interpolated into the document), which is injection-safe. The
`python3`/`node` fallbacks use the same load-set-dump approach. All paths write
to a temp file **in the same directory** and replace `package.json` via a
same-filesystem rename (`mv`/`os.replace`) — atomic on the same filesystem — so
a crash mid-write can never leave a truncated file. The original file mode
(e.g. `0644`) is preserved so CI and downstream tooling can still read it.
Both paths preserve 2-space indentation and a trailing newline. `sed`/`grep` on
JSON is explicitly prohibited because it is brittle against formatting and key
ordering.

## Configuration

The step needs no feature-specific flag. It uses the standard publish context:

| Context key / variable | Required | How the step uses it |
| --- | --- | --- |
| `worktree_setup.worktree_path` (via `WORKTREE_SETUP_WORKTREE_PATH`) | Yes | Directory to `cd` into before reading `Cargo.toml` and rewriting `package.json`. Populated by the `workflow-worktree` sub-recipe (step-04). |
| `RECIPE_VAR_worktree_setup__worktree_path` | Fallback | Secondary source for the worktree path. |
| `REPO_PATH` | Fallback | Final fallback if the worktree path is unavailable. |
| `condition` (`goal_already_met != 'true' && terminal_state.terminal_success != 'true' && terminal_state.should_publish == 'true'`) | Yes | Gates the step to publish-only runs, matching its sibling steps verbatim. |

## Behavior on non-JS workspaces

If `package.json` is absent, the step prints a skip message and exits `0`.
Repositories that are not JavaScript/Node workspaces continue through the
publish phase unchanged. The step also exits `0` without modifying anything if
the `[workspace.package].version` cannot be read.

## Examples

### Rust + JS workspace (package.json synced)

```text
step-14c: syncing package.json version to 0.11.2 (offline)
 package.json | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)
```

`step-15-commit-push` then runs `git add -A`, staging the updated `package.json`
alongside the bumped `Cargo.toml` and the synced `Cargo.lock`, and the CI `Test`
job's `package_json_version_matches_root_workspace_version` contract test passes.

### Already in sync (idempotent, no diff)

```text
step-14c: syncing package.json version to 0.11.2 (offline)
```

No `package.json` diff is produced because the value already matched.

### Non-JS workspace (skipped)

```text
step-14c: no package.json present; skipping version sync (non-JS workspace)
```

## Non-goals

- The step does not bump the version. It only mirrors the value already written
  to `[workspace.package].version` by `step-14-bump-version`.
- The step does not weaken, remove, or bypass any `--locked` flag, and it leaves
  `step-14b-sync-lockfile` untouched. Reproducible builds remain enforced.
- The step does not fetch from the network. The version read is `--offline`.
- The step does not edit JSON with `sed`/`grep`, and it does not read the
  version with an unscoped `grep` on `Cargo.toml`.
- The step does not stage `package.json` itself; staging is owned by
  `step-15-commit-push` via `git add -A`.

## Regression expectations

Tests covering this step should assert the semantic contract, not exact prose:

- `step-14c-sync-package-json` exists in `workflow-publish.yaml`.
- Step ordering holds:
  `step-14-bump-version` < `step-14b-sync-lockfile` <
  `step-14c-sync-package-json` < `step-14g-artifact-guard`.
- The recipe text reads the version offline and scoped to
  `[workspace.package].version` (no unscoped `grep`, no network fetch).
- The recipe text performs a robust JSON edit (`jq` on `package.json`), not
  `sed`/`grep`.
- The `package.json`-existence guard is present so non-JS workspaces are
  skipped.
- No `--locked` flag is removed from `.pre-commit-config.yaml`.

The integration test
`publish_syncs_package_json_version_after_bump_before_locked_gates` in
`tests/integration/workflow_publish_terminal_gate.rs` encodes these assertions
using the shared `load_publish_recipe()` and `step_index()` helpers. It is
verified to fail if `step-14c-sync-package-json` is removed, so the sync cannot
regress silently. The pre-existing contract test
`package_json_version_matches_root_workspace_version` in
`tests/integration/version_contract_test.rs` remains the source of truth for the
`package.json` ↔ `[workspace.package]` equality it protects.

## See also

- [Workflow Publish Lockfile Sync Reference](workflow-publish-lockfile-sync.md)
- [Default Workflow Step 13 Validation Reference](default-workflow-step-13-validation.md)
- [Workflow Terminal-State Reference](workflow-terminal-state.md)
- [Worktree Setup Propagation Reference](worktree-setup-propagation.md)
