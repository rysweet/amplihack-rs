#!/usr/bin/env bash
# finalize_step_status.sh — the body of `finalizer-step-status` in
# amplifier-bundle/recipes/workflow-finalize.yaml.
#
# Extracted for the brick budget when that recipe gained the assessment-status
# step and the one retry of the assessment (#2819 R3). The body is unchanged:
# the step runs it with `bash`, so it reads the same environment the recipe
# runner exported and writes the same stdout the step's `output:` captures. It
# is a file rather than an inline block so the recipe stays under its line
# budget; compressing the recipe instead is the one thing the brick rule
# forbids.

set -euo pipefail
# Issue #969: typed {status, reporting_failure}, so the classifier tells an implementation
# failure from a reporting failure WITHOUT parsing the agent narrative; an empty
# narrative means the reporting step failed to produce its human-readable output.
command -v jq >/dev/null 2>&1 || { echo "ERROR: finalizer-step-status requires jq" >&2; exit 2; }
# An explicit framework root is authoritative, including missing assets.
if [ -n "${AMPLIHACK_HOME:-}" ]; then
  CONTEXT_HELPER="$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_context.sh"
else
  CONTEXT_HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_context.sh"
  [ -f "$CONTEXT_HELPER" ] || CONTEXT_HELPER="$(pwd)/amplifier-bundle/tools/workflow_context.sh"
  [ -f "$CONTEXT_HELPER" ] || CONTEXT_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_context.sh"
  [ -f "$CONTEXT_HELPER" ] || CONTEXT_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_context.sh"
fi
[ -f "$CONTEXT_HELPER" ] || { echo "ERROR: workflow_context.sh not found at selected framework root" >&2; exit 2; }
# shellcheck source=workflow_context.sh disable=SC1090
. "$CONTEXT_HELPER"
narrative="$(workflow_context_read agentic_finalizer_narrative || :)"
status="ok"
reporting_failure="false"
trimmed="$(printf '%s' "$narrative" | tr -d '[:space:]')"
if [ -z "$trimmed" ]; then
  status="failed"
  reporting_failure="true"
fi
jq -nc --arg status "$status" --arg reporting_failure "$reporting_failure" \
  '{status: $status, reporting_failure: $reporting_failure}'
