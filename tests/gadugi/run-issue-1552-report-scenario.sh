#!/usr/bin/env bash
# Execute the exact shipped investigation-report step through recipe-runner-rs
# via the focused Rust regression, and require that runner-backed case to run.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
: "${RECIPE_RUNNER_QA_BINARY:?set RECIPE_RUNNER_QA_BINARY to the absolute path of recipe-runner-rs}"
case "$RECIPE_RUNNER_QA_BINARY" in
  /*) ;;
  *) echo "FAIL: RECIPE_RUNNER_QA_BINARY must be an absolute path" >&2; exit 1 ;;
esac
if [[ ! -f "$RECIPE_RUNNER_QA_BINARY" || ! -x "$RECIPE_RUNNER_QA_BINARY" ]]; then
  echo "FAIL: RECIPE_RUNNER_QA_BINARY must name an executable file at an absolute path" >&2
  exit 1
fi

export AMPLIHACK_1552_REQUIRE_RECIPE_RUNNER=1
export CARGO_BUILD_JOBS=4
: "${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR to a checkout-specific build directory}"
cd "$REPO_ROOT"
cargo test --locked -p amplihack-cli --test issue_1552_investigation_report_written \
  recipe_runner_exports_context_and_executes_shipped_report_step -- --exact --nocapture
echo "ISSUE_1552_RECIPE_RUNNER_SCENARIO_PASSED"
