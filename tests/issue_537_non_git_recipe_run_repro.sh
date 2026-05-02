#!/usr/bin/env bash
# Reproduction for issue #537: `amplihack recipe run` currently assumes the
# caller is inside a Git repository before routing can decide whether Git is
# actually needed.
#
# Run: bash tests/issue_537_non_git_recipe_run_repro.sh
# Expected before fix: captures a non-zero run from a non-git tempdir.
# Expected after fix: command succeeds, proving the raw Git-context failure no
# longer reproduces.

set -u

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMPDIR_REPRO="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_REPRO"' EXIT

ln -s "$REPO_ROOT/amplifier-bundle" "$TMPDIR_REPRO/amplifier-bundle"

echo "=== Issue #537 non-git recipe-run reproduction ==="
echo "repo root: $REPO_ROOT"
echo "non-git cwd: $TMPDIR_REPRO"
echo

(
  cd "$TMPDIR_REPRO" || exit 99
  AMPLIHACK_HOME="$REPO_ROOT" \
    amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
      -c task_description="hello" \
      -c repo_path=. \
      >"$TMPDIR_REPRO/stdout.txt" \
      2>"$TMPDIR_REPRO/stderr.txt"
)
rc=$?

echo "exit_code=$rc"
echo "--- stdout ---"
sed -n '1,160p' "$TMPDIR_REPRO/stdout.txt"
echo "--- stderr ---"
sed -n '1,200p' "$TMPDIR_REPRO/stderr.txt"

if [ "$rc" -eq 0 ]; then
  echo "No longer reproduces: non-git smart-orchestrator run succeeded."
  exit 0
fi

if grep -Eiq 'not a git repository|git repo|exit(ed)? with 128|exit status: 128|fatal:' \
  "$TMPDIR_REPRO/stdout.txt" "$TMPDIR_REPRO/stderr.txt"; then
  echo "Reproduced issue #537: non-git cwd triggered a Git-context failure."
  exit 0
fi

echo "Command failed, but not with the expected Git-context symptom." >&2
exit 1
