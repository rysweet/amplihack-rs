#!/usr/bin/env bash
# Self-locating harness that runs the SHIPPED `step-03b-extract-issue-number`
# bash body from amplifier-bundle/recipes/workflow-prep.yaml against a given
# issue_creation payload. Used by the gadugi-test scenario for issues
# #815/#804 so the scenario exercises the real recipe logic (not a copy).
#
# Usage: run-step-03b.sh <issue_creation> [task_description]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RECIPE="$SCRIPT_DIR/../../amplifier-bundle/recipes/workflow-prep.yaml"
[ -f "$RECIPE" ] || { echo "ERROR: recipe not found: $RECIPE" >&2; exit 2; }

ISSUE_CREATION_INPUT="${1:?usage: run-step-03b.sh <issue_creation> [task_description]}"
TASK_DESCRIPTION_INPUT="${2:-}"

# Extract the step-03b command body verbatim from the YAML.
STEP_BODY="$(
  RECIPE_PATH="$RECIPE" python3 - <<'PY'
import os, yaml
with open(os.environ["RECIPE_PATH"]) as fh:
    recipe = yaml.safe_load(fh)
for step in recipe["steps"]:
    if step.get("id") == "step-03b-extract-issue-number":
        print(step["command"], end="")
        break
else:
    raise SystemExit("step-03b-extract-issue-number not found")
PY
)"

ISSUE_CREATION="$ISSUE_CREATION_INPUT" TASK_DESCRIPTION="$TASK_DESCRIPTION_INPUT" \
  bash -c "$STEP_BODY"
