#!/usr/bin/env bash
# tdd_implementation_evidence_step.sh — the body of `implementation-terminal-evidence` in amplifier-bundle/recipes/workflow-tdd.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: the step runs it with
# `bash`, so it reads the same environment the recipe runner exported and
# writes the same stdout the step's `output:` captures. It is a file rather
# than an inline block so the recipe stays under its line budget; compressing
# the recipe instead is the one thing the brick rule forbids.

set -euo pipefail
# issue #962: full resolution ladder + visible no-discard degrade (see docs/reference/workflow-implementation-evidence.md)
HELPER="${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/workflow_implementation_evidence.sh"
[ -f "$HELPER" ] || HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_implementation_evidence.sh"
[ -f "$HELPER" ] || HELPER="$(pwd)/amplifier-bundle/tools/workflow_implementation_evidence.sh"
[ -f "$HELPER" ] || HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_implementation_evidence.sh"
[ -f "$HELPER" ] || HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_implementation_evidence.sh"
if [ -f "$HELPER" ]; then bash "$HELPER"; else echo "WARNING: implementation-evidence helper unresolvable (AMPLIHACK_HOME/REPO_PATH/cwd/~/.copilot/~/.amplihack); bookkeeping gate degrading VISIBLY to preserve step-08c-verified work (no-discard #962). searched: $HELPER cwd=$(pwd)" >&2; printf '%s\n' '{"implementation_completed":"true","terminal_no_op":"false","terminal_state":"EVIDENCE_TOOL_UNAVAILABLE","terminal_reason":"impl-evidence helper unresolvable; bookkeeping gate degraded (no-discard #962); step-08c-verified work preserved"}'; fi
