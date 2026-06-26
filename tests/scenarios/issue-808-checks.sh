#!/usr/bin/env bash
# Aggregated QA checks for issue #808 (default-workflow finalization cleanup).
# Resolves its own repository root so it runs correctly regardless of cwd.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
export AMPLIHACK_HOME="$ROOT"

echo "== [1/4] issue #808 deterministic cleanup + sibling-safety regression suite =="
cargo test -p amplihack --test issue_808_finalization_cleanup

echo "== [2/4] runtime-artifact helper contracts preserved =="
bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh

echo "== [3/4] PR-always-opens static guard =="
bash amplifier-bundle/recipes/tests/test-pr-always-opens.sh

echo "== [4/4] static guard validation scope =="
bash amplifier-bundle/recipes/tests/test-static-guard-validation-scope.sh

echo "ALL ISSUE #808 QA CHECKS PASSED"
