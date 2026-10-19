#!/usr/bin/env bash
# `load_pr_fields` and `load_azdo_pr_fields` must REPLACE their array, not
# append to it (issue #1423 follow-up).
#
# Both functions used to call the bash 4.0 array-read builtin, which replaces
# the array's whole contents on every call. #1423 replaced it with a
# `while IFS= read` loop, because macOS ships bash 3.2 and that builtin arrived
# in bash 4.0. A read-loop APPENDS. The replacements therefore set an explicit
# `ARR=()` ahead of the loop to restore the builtin's semantics.
#
# That reset is one short, easily-deleted statement whose absence is invisible
# on a first call and wrong on every call after it: PR_FIELDS[0] would still
# hold the FIRST pull request's URL while the caller believed it held the
# second's. The publish path calls these loaders once per candidate pull
# request, so a stale index is a wrong PR URL written into a workflow result.
# The static lint (tests/issue_1423_bash4_constructs.sh) cannot see this — it
# checks for bash-4 syntax, and a missing reset is valid bash. Hence a test
# that runs the functions.
#
# The helper is sourced in isolation through the WORKFLOW_PUBLISH_PR_LIB_ONLY
# seam, so this needs no git remote and no publish machinery.
#
# Usage: bash amplifier-bundle/recipes/tests/test-pr-field-arrays-reload.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="$HERE/../../tools/workflow_publish_pr.sh"

[ -f "$HELPER" ] || { echo "harness error: $HELPER not found" >&2; exit 2; }
command -v jq >/dev/null 2>&1 || { echo "harness error: jq is required" >&2; exit 2; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

# shellcheck disable=SC1090
WORKFLOW_PUBLISH_PR_LIB_ONLY=1 . "$HELPER"

echo "issue #1423 follow-up: PR field arrays reload instead of accumulating"
echo ""

# --- GitHub loader -----------------------------------------------------------
load_pr_fields '{"url":"https://example.test/pr/1","number":1,"state":"OPEN","mergedAt":null}'
first_len=${#PR_FIELDS[@]}
load_pr_fields '{"url":"https://example.test/pr/2","number":2,"state":"MERGED","mergedAt":"2026-09-23"}'

if [ "${#PR_FIELDS[@]}" -eq "$first_len" ]; then
  pass "load_pr_fields replaces PR_FIELDS (length stayed $first_len across two calls)"
else
  fail "load_pr_fields APPENDED: length went $first_len -> ${#PR_FIELDS[@]}; the ARR=() reset is missing"
fi

if [ "${PR_FIELDS[0]:-}" = "https://example.test/pr/2" ]; then
  pass "PR_FIELDS[0] is the second call's URL"
else
  fail "PR_FIELDS[0] is '${PR_FIELDS[0]:-}', expected the second call's URL — indices are stale"
fi

if [ "${PR_URL_RESULT:-}" = "https://example.test/pr/2" ]; then
  pass "PR_URL_RESULT tracks the second call"
else
  fail "PR_URL_RESULT is '${PR_URL_RESULT:-}', expected https://example.test/pr/2"
fi

# A third call proves the reset is unconditional, not a one-off.
load_pr_fields '{"url":"https://example.test/pr/3","number":3,"state":"OPEN","mergedAt":null}'
if [ "${#PR_FIELDS[@]}" -eq "$first_len" ] && [ "${PR_FIELDS[0]:-}" = "https://example.test/pr/3" ]; then
  pass "a third call still replaces rather than accumulates"
else
  fail "third call drifted: length ${#PR_FIELDS[@]}, PR_FIELDS[0]='${PR_FIELDS[0]:-}'"
fi

# An empty jq field must still occupy its slot, or every later index shifts.
load_pr_fields '{"url":"","number":4,"state":"OPEN","mergedAt":null}'
if [ "${#PR_FIELDS[@]}" -eq "$first_len" ]; then
  pass "an empty leading field keeps its slot (indices do not shift)"
else
  fail "empty field collapsed: length ${#PR_FIELDS[@]}, expected $first_len"
fi

# --- Azure DevOps loader -----------------------------------------------------
load_azdo_pr_fields '{"url":"https://azdo.test/pr/1","pullRequestId":1,"status":"active","sourceRefName":"refs/heads/a","targetRefName":"refs/heads/main"}'
azdo_first_len=${#AZDO_PR_FIELDS[@]}
load_azdo_pr_fields '{"url":"https://azdo.test/pr/2","pullRequestId":2,"status":"completed","sourceRefName":"refs/heads/b","targetRefName":"refs/heads/main"}'

if [ "${#AZDO_PR_FIELDS[@]}" -eq "$azdo_first_len" ]; then
  pass "load_azdo_pr_fields replaces AZDO_PR_FIELDS (length stayed $azdo_first_len)"
else
  fail "load_azdo_pr_fields APPENDED: length went $azdo_first_len -> ${#AZDO_PR_FIELDS[@]}"
fi

if [ "${AZDO_PR_FIELDS[0]:-}" = "https://azdo.test/pr/2" ]; then
  pass "AZDO_PR_FIELDS[0] is the second call's URL"
else
  fail "AZDO_PR_FIELDS[0] is '${AZDO_PR_FIELDS[0]:-}', expected the second call's URL"
fi

# --- The arrays must stay global, or the callers below see nothing -----------
# Length, not content: the call directly above loaded an empty url on purpose,
# so PR_FIELDS[0] is legitimately "" here. What is being checked is that the
# arrays survive the function at all — were either declared `local`, the caller
# would see a zero-length array no matter what was loaded.
if [ "${#PR_FIELDS[@]}" -gt 0 ] && [ "${#AZDO_PR_FIELDS[@]}" -gt 0 ]; then
  pass "both arrays are visible to the caller after sourcing"
else
  fail "an array was declared local to its function; callers would read nothing (PR_FIELDS=${#PR_FIELDS[@]}, AZDO_PR_FIELDS=${#AZDO_PR_FIELDS[@]})"
fi

echo ""
if [ "$fails" -ne 0 ]; then
  echo "FAIL: $fails check(s) failed."
  echo ""
  echo "These loaders replaced the bash 4.0 array-read builtin with a read-loop so"
  echo "they work on the bash 3.2 macOS ships (issue #1423). That builtin replaced"
  echo "the array; a read-loop appends. Each loop needs an explicit 'ARR=()' before"
  echo "it. Restore the reset."
  exit 1
fi
echo "PASS: PR field arrays reload cleanly on every call"
