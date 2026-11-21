#!/usr/bin/env bash
# smart_recursion_guard_step.sh — the body of `derive-recursion-guard` in amplifier-bundle/recipes/smart-execute-routing.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: the step runs it with
# `bash`, so it reads the same environment the recipe runner exported and
# writes the same stdout the step's `output:` captures. It is a file rather
# than an inline block so the recipe stays under its line budget; compressing
# the recipe instead is the one thing the brick rule forbids.

# FIX (#469/#311): Use env var — safe against injection and word-splitting.
SESSION_JSON="${RECIPE_VAR_session_info:-}"
# Issue #1062 (finding D1): read `status` through the tested
# extract-json|extract-field pipeline; unknown -> BLOCKED:unknown fail-safe.
STATUS=$(printf '%s' "$SESSION_JSON" \
  | amplihack orch helper extract-json \
  | amplihack orch helper extract-field --field status --default unknown)
if [ "$STATUS" = "ok" ] || [ "$STATUS" = "no_tree_script" ]; then
  echo "ALLOWED"
elif [ "$STATUS" = "registration_failed" ]; then
  echo "NOTE: Session tree registration was skipped — depth and capacity limits inactive." >&2
  echo "BLOCKED:registration_failed"
else
  # Unknown status — default to blocked
  echo "BLOCKED:unknown"
fi
