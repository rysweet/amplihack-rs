#!/usr/bin/env bash
# Scenario helper for issue #1326.
#
# `cargo test -- <filter>` exits 0 when the filter matches NOTHING, so an
# exit-code assertion alone lets a renamed test silently stop being covered.
# Require a non-zero pass count, matching the convention in
# tests/scenarios/issue-1266-launch-target-resolution.yaml.
#
# usage: issue-1326-cargo-step.sh <cargo-test-args...>
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
out="$(cargo test "$@" 2>&1)"
line="$(printf '%s\n' "$out" | grep -E '^test result' | tail -1)"
if printf '%s' "$line" | grep -qE 'ok\. [1-9][0-9]* passed'; then
  echo "PASS ($line)"
  exit 0
fi
printf '%s\n' "$out" | tail -25
echo "FAIL: expected at least one passing test; got: ${line:-<no test result line>}"
exit 1
