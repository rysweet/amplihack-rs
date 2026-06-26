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
# run the resulting bash. Python keeps multi-line payloads byte-exact.
SCRIPT_BODY="$(
  RECIPE_PATH="$RECIPE" V1="$V1_FILE" V2="$V2_FILE" V3="$V3_FILE" \
  THRESHOLD_VAL="$THRESHOLD" CYCLE_VAL="$CYCLE" OUTPUT_DIR_VAL="$OUTPUT_DIR" python3 - <<'PY'
import os, yaml
with open(os.environ["RECIPE_PATH"]) as fh:
    recipe = yaml.safe_load(fh)
for step in recipe["steps"]:
    if step.get("id") == "merge-validations":
        cmd = step["command"]
        break
else:
    raise SystemExit("merge-validations step not found")

def read(path):
    with open(path) as fh:
        return fh.read()

subs = {
    "validation_agent_1": read(os.environ["V1"]),
    "validation_agent_2": read(os.environ["V2"]),
    "validation_agent_3": read(os.environ["V3"]),
    "validation_threshold": os.environ["THRESHOLD_VAL"],
    "cycle_number": os.environ["CYCLE_VAL"],
    "output_dir": os.environ["OUTPUT_DIR_VAL"],
}
for key, val in subs.items():
    cmd = cmd.replace("{{" + key + "}}", val)
print(cmd, end="")
PY
)"

bash -c "$SCRIPT_BODY"
