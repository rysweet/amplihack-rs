#!/usr/bin/env bash
# Tests for Issue #684: step-03-create-issue must detect git remote host type
# and branch into GitHub, Azure DevOps, or local-tracking paths.
#
# TDD: These tests are written BEFORE the implementation. They define the
# required contract for multi-host support in the issue creation step.
#
# These tests validate:
#   1. step-03 command contains remote host type detection logic.
#   2. step-03 has distinct paths for GitHub, AzDO, and unknown remotes.
#   3. step-03b extracts issue numbers from AzDO work item URLs.
#   4. step-16 handles non-GitHub remotes (skips or adapts PR creation).
#   5. No GitHub-specific error messages leak to non-GitHub remote paths.
#   6. Runtime: detection function classifies remote URLs correctly.
#   7. YAML remains parseable.
#
# Run: bash tests/issue_684_azdo_remote_detection.sh
# Expected before fix: FAIL. Expected after fix: PASS.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREP_FILE="$REPO_ROOT/amplifier-bundle/recipes/workflow-prep.yaml"
PUBLISH_FILE="$REPO_ROOT/amplifier-bundle/recipes/workflow-publish.yaml"
FINALIZE_FILE="$REPO_ROOT/amplifier-bundle/recipes/workflow-finalize.yaml"

fail=0
pass=0

assert() {
    local desc="$1"
    local cond="$2"
    if eval "$cond"; then
        echo "PASS: $desc"
        pass=$((pass+1))
    else
        echo "FAIL: $desc"
        echo "      condition: $cond"
        fail=$((fail+1))
    fi
}

echo "=== Issue #684 TDD tests: Azure DevOps remote detection ==="
echo "Prep file:     $PREP_FILE"
echo "Publish file:  $PUBLISH_FILE"
echo "Finalize file: $FINALIZE_FILE"
echo

# --- Test 1: recipe files exist ----------------------------------------------
assert "workflow-prep.yaml exists" "[ -f '$PREP_FILE' ]"
assert "workflow-publish.yaml exists" "[ -f '$PUBLISH_FILE' ]"
assert "workflow-finalize.yaml exists" "[ -f '$FINALIZE_FILE' ]"

# --- Test 2: step-03 contains remote host type detection ---------------------
assert "step-03 detects REMOTE_HOST_TYPE" \
    "grep -q 'REMOTE_HOST_TYPE' '$PREP_FILE'"

assert "step-03 checks for github.com" \
    "grep -q 'github.com' '$PREP_FILE'"

assert "step-03 checks for dev.azure.com" \
    "grep -q 'dev.azure.com' '$PREP_FILE'"

assert "step-03 checks for visualstudio.com" \
    "grep -q 'visualstudio.com' '$PREP_FILE'"

# --- Test 3: step-03 has Azure DevOps work item path -------------------------
assert "step-03 has 'az boards' for AzDO work items" \
    "grep -q 'az boards' '$PREP_FILE'"

assert "step-03 supports AB# work item references" \
    "grep -q 'AB#' '$PREP_FILE'"

# --- Test 4: step-03 has local tracking fallback -----------------------------
assert "step-03 has local tracking fallback" \
    "grep -Eq 'local.tracking|LOCAL_ISSUE|local_issue' '$PREP_FILE'"

# --- Test 5: no GitHub-specific error messages for non-GitHub remotes --------
assert "step-03 does not contain 'gh auth login' error text" \
    "! grep -q 'gh auth login' '$PREP_FILE'"

assert "step-03 does not contain 'none of the git remotes' error text" \
    "! grep -q 'none of the git remotes' '$PREP_FILE'"

# --- Test 6: step-03b handles AzDO URLs --------------------------------------
# Use awk+grep directly (avoids quoting issues with $-vars in YAML command blocks)
assert "step-03b handles _workitems/edit/ URLs" \
    "awk '/id: .step-03b-extract-issue-number/,/output:/' '$PREP_FILE' | grep -q '_workitems/edit/'"

assert "step-03b handles AB# references" \
    "awk '/id: .step-03b-extract-issue-number/,/output:/' '$PREP_FILE' | grep -q 'AB#'"

assert "step-03b still handles GitHub issues/ URLs (regression)" \
    "awk '/id: .step-03b-extract-issue-number/,/output:/' '$PREP_FILE' | grep -q 'issues/'"

assert "step-03b still handles GitHub pull/ URLs (regression)" \
    "awk '/id: .step-03b-extract-issue-number/,/output:/' '$PREP_FILE' | grep -q 'pull/'"

# --- Test 7: step-16 handles non-GitHub remotes -----------------------------
assert "step-16 contains remote host type detection or github.com check" \
    "grep -Eq 'REMOTE_HOST_TYPE|remote_host_type' '$PUBLISH_FILE' || \
     awk '/id: \"step-16-create-draft-pr\"/{f=1} f' '$PUBLISH_FILE' | head -30 | grep -q 'github.com'"

# --- Test 8: step-21 guards PR_URL before gh commands -----------------------
assert "step-21 references PR_URL variable" \
    "awk '/id: .step-21-pr-ready/,/output:/' '$FINALIZE_FILE' | grep -q 'PR_URL'"

assert "step-21 checks PR_URL before gh pr ready" \
    "awk '/id: .step-21-pr-ready/,/output:/' '$FINALIZE_FILE' | grep -Eq '\\[ -z .PR_URL|\\[ -n .PR_URL|-z \".PR_URL|-n \".PR_URL|PR_URL.*=~|\\[\\[.*PR_URL'"

# --- Test 9: Runtime remote detection function tests -------------------------
# These test the expected shell logic for classifying remote URLs.
# The implementation should use something like:
#   REMOTE_URL=$(git remote get-url origin 2>/dev/null || echo "")
#   if [[ "$REMOTE_URL" =~ github\.com ]]; then REMOTE_HOST_TYPE="github"
#   elif [[ "$REMOTE_URL" =~ dev\.azure\.com|visualstudio\.com|ssh\.dev\.azure\.com ]]; then REMOTE_HOST_TYPE="azdo"
#   else REMOTE_HOST_TYPE="unknown"; fi

# Simulate the detection logic for various URL formats
detect_host_type() {
    local url="$1"
    if [[ "$url" =~ github\.com ]]; then
        echo "github"
    elif [[ "$url" =~ dev\.azure\.com|visualstudio\.com|ssh\.dev\.azure\.com ]]; then
        echo "azdo"
    else
        echo "unknown"
    fi
}

# GitHub URLs
assert "detect: https://github.com/org/repo.git → github" \
    "[ '$(detect_host_type 'https://github.com/org/repo.git')' = 'github' ]"

assert "detect: git@github.com:org/repo.git → github" \
    "[ '$(detect_host_type 'git@github.com:org/repo.git')' = 'github' ]"

assert "detect: ssh://git@github.com/org/repo.git → github" \
    "[ '$(detect_host_type 'ssh://git@github.com/org/repo.git')' = 'github' ]"

# Azure DevOps URLs
assert "detect: https://dev.azure.com/org/project/_git/repo → azdo" \
    "[ '$(detect_host_type 'https://dev.azure.com/org/project/_git/repo')' = 'azdo' ]"

assert "detect: https://org@dev.azure.com/org/project/_git/repo → azdo" \
    "[ '$(detect_host_type 'https://org@dev.azure.com/org/project/_git/repo')' = 'azdo' ]"

assert "detect: https://org.visualstudio.com/project/_git/repo → azdo" \
    "[ '$(detect_host_type 'https://org.visualstudio.com/project/_git/repo')' = 'azdo' ]"

assert "detect: git@ssh.dev.azure.com:v3/org/project/repo → azdo" \
    "[ '$(detect_host_type 'git@ssh.dev.azure.com:v3/org/project/repo')' = 'azdo' ]"

assert "detect: ssh://git@ssh.dev.azure.com/v3/org/project/repo → azdo" \
    "[ '$(detect_host_type 'ssh://git@ssh.dev.azure.com/v3/org/project/repo')' = 'azdo' ]"

# Unknown/other remotes
assert "detect: https://gitlab.com/org/repo.git → unknown" \
    "[ '$(detect_host_type 'https://gitlab.com/org/repo.git')' = 'unknown' ]"

assert "detect: https://bitbucket.org/org/repo.git → unknown" \
    "[ '$(detect_host_type 'https://bitbucket.org/org/repo.git')' = 'unknown' ]"

assert "detect: empty URL → unknown" \
    "[ '$(detect_host_type '')' = 'unknown' ]"

# --- Test 10: AzDO work item ID extraction from various formats ---------------
extract_azdo_id() {
    local input="$1"
    local extracted=""
    # Try _workitems/edit/NNN
    extracted=$(printf '%s' "$input" | grep -oE '_workitems/edit/[0-9]+' | grep -oE '[0-9]+' | head -1)
    if [ -z "$extracted" ]; then
        # Try AB#NNN
        extracted=$(printf '%s' "$input" | grep -oE 'AB#[0-9]+' | grep -oE '[0-9]+' | head -1)
    fi
    printf '%s' "$extracted"
}

assert "extract AzDO: _workitems/edit/12345 → 12345" \
    "[ '$(extract_azdo_id 'https://dev.azure.com/org/project/_workitems/edit/12345')' = '12345' ]"

assert "extract AzDO: AB#6789 → 6789" \
    "[ '$(extract_azdo_id 'AB#6789')' = '6789' ]"

assert "extract AzDO: 'Created AB#42 in project' → 42" \
    "[ '$(extract_azdo_id 'Created AB#42 in project')' = '42' ]"

# --- Test 11: YAML parses cleanly -------------------------------------------
if command -v python3 >/dev/null 2>&1; then
    if python3 -c "import yaml" 2>/dev/null; then
        assert "workflow-prep.yaml parses with yaml.safe_load" \
            "python3 -c 'import yaml; yaml.safe_load(open(\"$PREP_FILE\"))'"
        assert "workflow-publish.yaml parses with yaml.safe_load" \
            "python3 -c 'import yaml; yaml.safe_load(open(\"$PUBLISH_FILE\"))'"
        assert "workflow-finalize.yaml parses with yaml.safe_load" \
            "python3 -c 'import yaml; yaml.safe_load(open(\"$FINALIZE_FILE\"))'"
    else
        echo "SKIP: PyYAML not available"
    fi
else
    echo "SKIP: python3 not available"
fi

# --- Summary ----------------------------------------------------------------
echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
