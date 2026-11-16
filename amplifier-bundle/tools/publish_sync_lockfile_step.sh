#!/usr/bin/env bash
# publish_sync_lockfile_step.sh — the body of `step-14b-sync-lockfile` in amplifier-bundle/recipes/workflow-publish.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: it reads the same
# `RECIPE_VAR_*` variables the recipe runner exports and writes the same
# stdout the step's `output:` captures. It is a file rather than an inline
# block so the recipe stays under the 400-line limit; compressing the recipe
# instead is the one thing the brick rule forbids.

set -euo pipefail
WT_TOOL=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/worktree_setup_path.sh" ] && { WT_TOOL="$c/amplifier-bundle/tools/worktree_setup_path.sh"; break; }; done
WORKTREE_SETUP_WORKTREE_PATH="$(bash "${WT_TOOL:?worktree_setup_path.sh not found}" step-14b-sync-lockfile)" || { [ $? -eq 3 ] || exit 1; WORKTREE_SETUP_WORKTREE_PATH="${RECIPE_VAR_repo_path:-}"; }
cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-14b requires worktree_setup.worktree_path from step-04 (workflow-worktree); ensure parent recipe ran worktree-setup and propagated outputs}"
# Issue #915: step-14's version bump leaves Cargo.lock stale, so --locked
# pre-commit gates fail deterministically. Keep the lock IN SYNC (do NOT
# weaken --locked) via an offline update; skip gracefully on non-Rust workspaces.
if [ -f Cargo.toml ] && [ -f Cargo.lock ]; then
  echo "step-14b: syncing Cargo.lock to bumped version (offline)"
  cargo update --workspace --offline
else
  echo "step-14b: no Cargo.toml/Cargo.lock present; skipping lockfile sync (non-Rust workspace)"
fi
