#!/usr/bin/env bash
# autodrive_crusty_concerns_evidence.sh — the body of `step-05-verify-concerns-addressed` in amplifier-bundle/recipes/autodrive-crusty-round.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: it reads the same
# `RECIPE_VAR_*` variables the recipe runner exports and writes the same
# stdout the step's `output:` captures. It is a file rather than an inline
# block so the recipe stays under the 400-line limit; compressing the recipe
# instead is the one thing the brick rule forbids.

set -uo pipefail
cd "${REPO_PATH:-.}" 2>/dev/null || true
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
GIT_OK="true"
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  GIT_OK="false"
  echo "[skip] not a git repo; commit evidence is reported as unobserved, never as progress" >&2
fi
BASE="$(printf '%s' "${RECIPE_VAR_crusty_round_context:-}" \
  | amplihack orch helper extract-json --require-field head_sha \
  | amplihack orch helper extract-field --field head_sha --default '')"
NOW=""
[ "$GIT_OK" = "true" ] && NOW="$(git rev-parse HEAD 2>/dev/null || printf '')"
COMMITS=0
if [ "$GIT_OK" = "true" ] && [ -n "$BASE" ] && git rev-parse --verify --quiet "${BASE}^{commit}" >/dev/null 2>&1; then
  COMMITS="$(git rev-list --count "${BASE}..HEAD" 2>/dev/null || printf 0)"
fi
DIRTY="false"
[ "$GIT_OK" = "true" ] && { git diff --quiet 2>/dev/null || DIRTY="true"; }
if [ "$COMMITS" = "0" ]; then
  echo "WARNING: the round addressed concerns but produced NO commit. That is measured evidence of a round that did nothing; the loop evaluator will see it." >&2
fi
printf '{"base_sha":"%s","head_sha":"%s","commits":%s,"dirty_worktree":"%s","git_observed":"%s"}\n' \
  "$BASE" "$NOW" "${COMMITS:-0}" "$DIRTY" "$GIT_OK"
