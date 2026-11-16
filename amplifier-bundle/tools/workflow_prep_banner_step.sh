#!/usr/bin/env bash
# workflow_prep_banner_step.sh — the body of `step-00-workflow-preparation` in amplifier-bundle/recipes/workflow-prep.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: it reads the same
# `RECIPE_VAR_*` variables the recipe runner exports and writes the same
# stdout the step's `output:` captures. It is a file rather than an inline
# block so the recipe stays under the 400-line limit; compressing the recipe
# instead is the one thing the brick rule forbids.

# The 23-step plan banner lives in amplifier-bundle/tools/workflow_plan_banner.sh.
# It was extracted, not trimmed: this brick is at its 400-line budget and the rule's
# remedy for a full brick is extraction, never compression (scripts/check-brick-budget.sh).
B=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/workflow_plan_banner.sh" ] && { B="$c/amplifier-bundle/tools/workflow_plan_banner.sh"; break; }; done
if [ -n "$B" ]; then bash "$B"; else echo "WARNING: workflow plan banner helper not found (searched AMPLIHACK_HOME, REPO_PATH, cwd, ~/.copilot, ~/.amplihack) — continuing without the plan listing" >&2; fi
# FIX (#469/#311): Use env vars — safe against injection and word-splitting.
TASK_DESC="$TASK_DESCRIPTION"
printf 'Task: %s\n' "$TASK_DESC"
printf 'Repository: %s\n' "$REPO_PATH"
