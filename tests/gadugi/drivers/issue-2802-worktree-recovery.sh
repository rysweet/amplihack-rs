#!/usr/bin/env bash
# CI-only entry point shared with the Gadugi scenario. All fixtures and
# assertions live in Rust; this driver performs no recovery or Git mutation.
set -euo pipefail
umask 077

: "${ISSUE_2802_OPENAI_API_KEY:?INCOMPLETE #2802: real-agent credentials unavailable}"
: "${AMPLIHACK_TEST_BINARY:?INCOMPLETE #2802: supply the CI-installed amplihack binary}"
command -v codex >/dev/null || { echo 'INCOMPLETE #2802: real Codex adapter unavailable' >&2; exit 2; }
command -v jq >/dev/null || { echo 'INCOMPLETE #2802: jq unavailable' >&2; exit 2; }

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd -P)"
private_root="$(mktemp -d "${RUNNER_TEMP:?}/issue-2802.XXXXXX")"
trap 'rm -rf -- "$private_root"' EXIT
mkdir -m 700 "$private_root/tmp"
export TMPDIR="$private_root/tmp"

cd "$repo"
set +e
cargo test --locked -p amplihack-cli --test cli_integration \
  issue_2802_worktree_recovery::live_ -- --ignored --nocapture --test-threads=1 \
  2>&1 | tee "$private_root/result.log"
status=${PIPESTATUS[0]}
set -e

if [ "$status" -ne 0 ]; then
  echo "FAILED_OR_INCOMPLETE #2802: real-agent behavioral suite exited $status" >&2
  exit "$status"
fi
printf 'ALL_CASES_PASSED #2802: real-agent private worktree recovery\n'
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    printf '### Issue #2802 release evidence\n\n'
    printf -- '- Run: %s/%s/actions/runs/%s\n' "$GITHUB_SERVER_URL" "$GITHUB_REPOSITORY" "$GITHUB_RUN_ID"
    printf -- '- Head: `%s`\n- Job: `%s`\n' "$GITHUB_SHA" "$GITHUB_JOB"
    printf -- '- Step: Outside-in - private worktree recovery (issue #2802)\n'
    printf -- '- Conclusion: passed; all selected real-agent behavioral assertions passed.\n'
  } >> "$GITHUB_STEP_SUMMARY"
fi
