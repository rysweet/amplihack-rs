#!/usr/bin/env bash
# Self-locating harness that runs the SHIPPED `merge-validations` bash body from
# amplifier-bundle/recipes/quality-audit-cycle.yaml against three validator
# payloads. Used by the gadugi-test scenario for issue #820 so the scenario
# exercises the REAL recipe logic (not a copy).
#
# Usage:
#   run-merge-validations.sh <v1_file> <v2_file> <v3_file> [threshold] [cycle] [output_dir]
#
# Writes the merged JSON to stdout, the step's diagnostics to stderr, and exits
# with the step's exit code.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RECIPE="$SCRIPT_DIR/../../amplifier-bundle/recipes/quality-audit-cycle.yaml"
[ -f "$RECIPE" ] || { echo "ERROR: recipe not found: $RECIPE" >&2; exit 2; }

V1_FILE="${1:?usage: run-merge-validations.sh <v1_file> <v2_file> <v3_file> [threshold] [cycle] [output_dir]}"
V2_FILE="${2:?need v2_file}"
V3_FILE="${3:?need v3_file}"
THRESHOLD="${4:-2}"
CYCLE="${5:-1}"
OUTPUT_DIR="${6:-$(mktemp -d)}"

# The merge step calls `amplihack orch helper extract-json`. Make sure that
# binary is resolvable even when this runs outside an activated shell (CI),
# preferring an already-on-PATH amplihack, then a locally built one.
if ! command -v amplihack >/dev/null 2>&1; then
  for _cand in \
    "$SCRIPT_DIR/../../target/debug/amplihack" \
    "$SCRIPT_DIR/../../target/release/amplihack" \
    "${CARGO_TARGET_DIR:-}/debug/amplihack" \
    "${CARGO_TARGET_DIR:-}/release/amplihack" \
    "$HOME/.local/bin/amplihack" \
    "$HOME/.cargo/bin/amplihack"; do
    if [ -n "$_cand" ] && [ -x "$_cand" ]; then
      PATH="$(dirname "$_cand"):$PATH"
      export PATH
      break
    fi
  done
fi

# Substitute the {{...}} placeholders exactly as recipe-runner-rs would, then
# run the resulting bash. The body is read with recipe-step-command.sh and the
# placeholders are replaced in bash, not python3: auto-drive runs this scenario
# with the real gadugi-test on any host, and a PyYAML import made it fail
# wherever PyYAML was not installed (PR #1520 review).
#
# replace_all splits on the literal placeholder, so a payload is never read as
# a pattern and its `&`, `\` and `*` stay byte-exact on every bash from 3.2 up.
# Each payload keeps its trailing newlines (the trailing `x` stops $(...) from
# stripping them), as the file read did before.
replace_all() { # replace_all <text> <placeholder> <value>
  local rest="$1" out=""
  while :; do
    case "$rest" in
      *"$2"*) out="${out}${rest%%"$2"*}$3"; rest="${rest#*"$2"}" ;;
      *) break ;;
    esac
  done
  printf '%s' "${out}${rest}"
}
read_payload() { cat -- "$1" && printf x; }

SCRIPT_BODY="$(bash "$SCRIPT_DIR/recipe-step-command.sh" "$RECIPE" merge-validations)"
for key in validation_agent_1 validation_agent_2 validation_agent_3 validation_threshold cycle_number output_dir; do
  case "$key" in
    validation_agent_1) val="$(read_payload "$V1_FILE")"; val="${val%x}" ;;
    validation_agent_2) val="$(read_payload "$V2_FILE")"; val="${val%x}" ;;
    validation_agent_3) val="$(read_payload "$V3_FILE")"; val="${val%x}" ;;
    validation_threshold) val="$THRESHOLD" ;;
    cycle_number) val="$CYCLE" ;;
    output_dir) val="$OUTPUT_DIR" ;;
  esac
  SCRIPT_BODY="$(replace_all "$SCRIPT_BODY" "{{${key}}}" "$val"; printf x)"
  SCRIPT_BODY="${SCRIPT_BODY%x}"
done

bash -c "$SCRIPT_BODY"
