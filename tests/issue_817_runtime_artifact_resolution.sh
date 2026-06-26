#!/usr/bin/env bash
# Behavior reproduction for issue #817 used by the QA scenario
# (tests/scenarios/issue-817-runtime-artifact-resolution.yaml).
#
# It extracts the *real* RUNTIME_ARTIFACT_HELPER resolution chains from each
# lifecycle recipe and executes them in a controlled environment, asserting:
#   1. For every lifecycle recipe, with AMPLIHACK_HOME unset and a target repo
#      that has no amplifier-bundle/, the helper resolves from the installed
#      ~/.amplihack bundle.
#   2. An explicit AMPLIHACK_HOME bundle takes precedence over the fallback.
#   3. The ~/.copilot bundle takes precedence over the ~/.amplihack bundle
#      (the documented order: ... -> ~/.copilot -> ~/.amplihack).
#   4. When no candidate exists anywhere, the recipe guard fails loudly with
#      exit 2 instead of silently sourcing a missing path.
#
# This mirrors crates/amplihack-cli/tests/issue_817_runtime_artifact_resolution.rs
# but runs instantly (no cargo build) so it is usable from the agentic-test
# QA harness.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RECIPES_DIR="$REPO_ROOT/amplifier-bundle/recipes"

# Every lifecycle recipe that resolves workflow_runtime_artifacts.sh.
RECIPES=(
  workflow-tdd.yaml
  workflow-finalize.yaml
  workflow-publish.yaml
  workflow-refactor-review.yaml
  workflow-pr-review.yaml
)

# Extract the RUNTIME_ARTIFACT_HELPER= resolution assignments from a recipe.
# A recipe may contain more than one identical resolution site; they all encode
# the same precedence, so concatenating them and reading the final value is
# sufficient to validate the resolution behavior.
extract_chain() {
  local recipe="$1"
  local chain
  chain="$(grep 'RUNTIME_ARTIFACT_HELPER=' "$RECIPES_DIR/$recipe" | sed 's/^[[:space:]]*//')"
  [ -n "$chain" ] || { echo "FAIL: no resolution chain found in $recipe" >&2; exit 1; }
  case "$chain" in
    *".amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"*) : ;;
    *) echo "FAIL: $recipe chain missing ~/.amplihack fallback candidate" >&2; exit 1 ;;
  esac
  printf '%s' "$chain"
}

# Extract the failure guard line ([ -f ... ] || { echo ...; exit 2; }) for a recipe.
extract_guard() {
  local recipe="$1"
  grep -m1 'workflow runtime artifact helper not found' "$RECIPES_DIR/$recipe" | sed 's/^[[:space:]]*//'
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Run a resolution snippet (chain plus optional extra lines) under a controlled
# environment and print the resolved RUNTIME_ARTIFACT_HELPER. Args:
#   $1 snippet  $2 repo  $3 home  $4 AMPLIHACK_HOME (optional)
run_snippet() {
  local snippet="$1" repo="$2" home="$3" ahome="${4:-}"
  local script
  script=$(printf 'set -uo pipefail\n%s\nprintf "%%s" "$RUNTIME_ARTIFACT_HELPER"\n' "$snippet")
  # stderr is suppressed: some chains probe `git rev-parse` in the throwaway
  # target dir (which is intentionally not a git repo) and that benign warning
  # would otherwise clutter the QA output. Resolution uses stdout + exit code.
  if [ -n "$ahome" ]; then
    env -u WORKFLOW_RUNTIME_ARTIFACT_HELPER AMPLIHACK_HOME="$ahome" REPO_PATH="$repo" HOME="$home" \
      bash -c "cd '$repo' && $script" 2>/dev/null
  else
    env -u WORKFLOW_RUNTIME_ARTIFACT_HELPER -u AMPLIHACK_HOME REPO_PATH="$repo" HOME="$home" \
      bash -c "cd '$repo' && $script" 2>/dev/null
  fi
}

# --- Case 1: every recipe falls back to the installed ~/.amplihack bundle ---
for recipe in "${RECIPES[@]}"; do
  chain="$(extract_chain "$recipe")"
  repo="$WORK/$recipe-repo"            # deliberately has no amplifier-bundle/
  home="$WORK/$recipe-home"
  installed="$home/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
  mkdir -p "$repo" "$(dirname "$installed")"
  printf '#!/usr/bin/env bash\npreflight_known_workflow_runtime_artifacts() { :; }\n' > "$installed"

  resolved="$(run_snippet "$chain" "$repo" "$home")"
  if [ "$resolved" != "$installed" ]; then
    echo "FAIL: $recipe expected fallback to '$installed' but resolved '$resolved'" >&2
    exit 1
  fi
  [ -f "$resolved" ] || { echo "FAIL: $recipe resolved helper is not a file: $resolved" >&2; exit 1; }
done

TDD_CHAIN="$(extract_chain "workflow-tdd.yaml")"

# --- Case 2: explicit AMPLIHACK_HOME wins over the installed fallback ---
repo="$WORK/case2-repo"
home="$WORK/case2-home"
explicit_home="$WORK/case2-explicit"
explicit="$explicit_home/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
installed="$home/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
mkdir -p "$repo" "$(dirname "$explicit")" "$(dirname "$installed")"
printf '# explicit\n' > "$explicit"
printf '# installed\n' > "$installed"
resolved="$(run_snippet "$TDD_CHAIN" "$repo" "$home" "$explicit_home")"
if [ "$resolved" != "$explicit" ]; then
  echo "FAIL: explicit AMPLIHACK_HOME must win; expected '$explicit' but resolved '$resolved'" >&2
  exit 1
fi

# --- Case 3: ~/.copilot wins over ~/.amplihack when both exist ---
repo="$WORK/case3-repo"
home="$WORK/case3-home"
copilot="$home/.copilot/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
amplihack="$home/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
mkdir -p "$repo" "$(dirname "$copilot")" "$(dirname "$amplihack")"
printf '# copilot\n' > "$copilot"
printf '# amplihack\n' > "$amplihack"
resolved="$(run_snippet "$TDD_CHAIN" "$repo" "$home")"
if [ "$resolved" != "$copilot" ]; then
  echo "FAIL: ~/.copilot must win over ~/.amplihack; expected '$copilot' but resolved '$resolved'" >&2
  exit 1
fi

# --- Case 4: no candidate anywhere -> recipe guard exits 2 ---
GUARD="$(extract_guard "workflow-tdd.yaml")"
[ -n "$GUARD" ] || { echo "FAIL: could not extract tdd failure guard" >&2; exit 1; }
repo="$WORK/case4-repo"   # no bundle
home="$WORK/case4-home"   # no ~/.copilot or ~/.amplihack bundle
mkdir -p "$repo" "$home"
set +e
run_snippet "$(printf '%s\n%s' "$TDD_CHAIN" "$GUARD")" "$repo" "$home" >/dev/null 2>&1
rc=$?
set -e
if [ "$rc" -ne 2 ]; then
  echo "FAIL: missing helper everywhere must exit 2, got $rc" >&2
  exit 1
fi

echo "PASS: issue-817 runtime artifact resolution behaves correctly across all lifecycle recipes"
