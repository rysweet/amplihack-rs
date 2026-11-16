#!/usr/bin/env bash
# finalize_workflow_complete_step.sh — the body of `workflow-complete` in amplifier-bundle/recipes/workflow-finalize.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: it reads the same
# `RECIPE_VAR_*` variables the recipe runner exports and writes the same
# stdout the step's `output:` captures. It is a file rather than an inline
# block so the recipe stays under the 400-line limit; compressing the recipe
# instead is the one thing the brick rule forbids.

set -euo pipefail
# Reports workflow_result, terminal_outcome, terminal_state, terminal_success,
# required_next_action, hollow_success_detected, evidence_used, reporting_failure,
# finalizer_schema_version, finalizer_confidence, finalizer_output_valid,
# terminal_failure, and the terminal vocabulary: MERGED, CLOSED_OBSOLETE,
# NO_DIFF_SUCCESS, FOLLOWUP_CREATED, EXISTING_OPEN_PR, SUPERSEDED, IMPLEMENTED_VERIFIED,
# ALLOW_NO_OP, BLOCKED_CI, FAILED_IMPLEMENTATION, FAILED_REPORTING,
# FAILED_INVALID_EVIDENCE, FAILED_MISSING_TOOLING, FAILED_PR_METADATA_UNAVAILABLE,
# FAILED_DIRTY_WORKTREE, FAILED_MEANINGFUL_DIFF, HOLLOW_SUCCESS, INCOMPLETE.
FINALIZER_HELPER="${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/workflow_agentic_finalization.sh"; [ -f "$FINALIZER_HELPER" ] || FINALIZER_HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_agentic_finalization.sh"; [ -f "$FINALIZER_HELPER" ] || FINALIZER_HELPER="$(pwd)/amplifier-bundle/tools/workflow_agentic_finalization.sh"; [ -f "$FINALIZER_HELPER" ] || FINALIZER_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_agentic_finalization.sh"; [ -f "$FINALIZER_HELPER" ] || FINALIZER_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_agentic_finalization.sh"; [ -f "$FINALIZER_HELPER" ] || { echo "ERROR: agentic finalization helper not found at any known location (AMPLIHACK_HOME, REPO_PATH, worktree, cwd, ~/.copilot, ~/.amplihack): $FINALIZER_HELPER; cwd=$(pwd)" >&2; exit 2; }
bash "$FINALIZER_HELPER" complete
