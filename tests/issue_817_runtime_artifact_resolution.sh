#!/usr/bin/env bash
# Behavior reproduction for issue #817 used by the QA scenario
# (tests/scenarios/issue-817-runtime-artifact-resolution.yaml).
#
# It extracts the *real* RUNTIME_ARTIFACT_HELPER resolution chain from
# workflow-tdd.yaml and executes it in a controlled environment, asserting:
#   1. With AMPLIHACK_HOME unset and a target repo that has no amplifier-bundle/,
#      the helper resolves from the installed ~/.amplihack bundle.
#   2. An explicit AMPLIHACK_HOME bundle takes precedence over the fallback.
#
# This mirrors crates/amplihack-cli/tests/issue_817_runtime_artifact_resolution.rs
# but runs instantly (no cargo build) so it is usable from the agentic-test
# QA harness.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RECIPE="$REPO_ROOT/amplifier-bundle/recipes/workflow-tdd.yaml"

[ -f "$RECIPE" ] || { echo "FAIL: cannot find $RECIPE" >&2; exit 1; }

# Extract the contiguous resolution assignments (the only RUNTIME_ARTIFACT_HELPER=
# lines in workflow-tdd.yaml belong to this single chain).
CHAIN="$(grep 'RUNTIME_ARTIFACT_HELPER=' "$RECIPE" | sed 's/^[[:space:]]*//')"
[ -n "$CHAIN" ] || { echo "FAIL: no resolution chain found in workflow-tdd.yaml" >&2; exit 1; }
case "$CHAIN" in
  *".amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"*) : ;;
  *) echo "FAIL: chain missing ~/.amplihack fallback candidate" >&2; exit 1 ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

run_chain() {
  # Args: REPO_PATH HOME [AMPLIHACK_HOME]
  local repo="$1" home="$2" ahome="${3:-}"
  local script
  script=$(printf 'set -uo pipefail\n%s\nprintf "%%s" "$RUNTIME_ARTIFACT_HELPER"\n' "$CHAIN")
  if [ -n "$ahome" ]; then
    env -u WORKFLOW_RUNTIME_ARTIFACT_HELPER AMPLIHACK_HOME="$ahome" REPO_PATH="$repo" HOME="$home" \
      bash -c "cd '$repo' && $script"
  else
    env -u WORKFLOW_RUNTIME_ARTIFACT_HELPER -u AMPLIHACK_HOME REPO_PATH="$repo" HOME="$home" \
      bash -c "cd '$repo' && $script"
  fi
}

# --- Case 1: fallback to installed ~/.amplihack bundle ---
REPO1="$WORK/target-repo"          # deliberately has no amplifier-bundle/
HOME1="$WORK/home"
INSTALLED="$HOME1/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
mkdir -p "$REPO1" "$(dirname "$INSTALLED")"
printf '#!/usr/bin/env bash\npreflight_known_workflow_runtime_artifacts() { :; }\n' > "$INSTALLED"

RESOLVED1="$(run_chain "$REPO1" "$HOME1")"
if [ "$RESOLVED1" != "$INSTALLED" ]; then
  echo "FAIL: expected fallback to '$INSTALLED' but resolved '$RESOLVED1'" >&2
  exit 1
fi
[ -f "$RESOLVED1" ] || { echo "FAIL: resolved helper is not a file: $RESOLVED1" >&2; exit 1; }

# --- Case 2: explicit AMPLIHACK_HOME wins over the installed fallback ---
REPO2="$WORK/target-repo2"
HOME2="$WORK/home2"
EXPLICIT_HOME="$WORK/explicit-home"
EXPLICIT="$EXPLICIT_HOME/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
INSTALLED2="$HOME2/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
mkdir -p "$REPO2" "$(dirname "$EXPLICIT")" "$(dirname "$INSTALLED2")"
printf '# explicit\n' > "$EXPLICIT"
printf '# installed\n' > "$INSTALLED2"

RESOLVED2="$(run_chain "$REPO2" "$HOME2" "$EXPLICIT_HOME")"
if [ "$RESOLVED2" != "$EXPLICIT" ]; then
  echo "FAIL: explicit AMPLIHACK_HOME must win; expected '$EXPLICIT' but resolved '$RESOLVED2'" >&2
  exit 1
fi

echo "PASS: issue-817 runtime artifact resolution behaves correctly"
