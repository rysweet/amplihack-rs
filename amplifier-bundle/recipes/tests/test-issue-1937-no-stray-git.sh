#!/usr/bin/env bash
# Regression for #1937: a recipe test that builds fixture repositories must
# keep every repository inside its own scratch directory.
#
# A pre-push or pre-commit hook leaves repository selectors such as GIT_DIR in
# the environment. Git honours them even for `git -C <fixture> init`, so an
# unprotected fixture writes a repository wherever the selector points; once
# that is /tmp, every later walk-up that starts under /tmp stops there.
#
# Each writer is run with TMPDIR set to a fresh directory and with GIT_DIR
# pointing at a `.git` in that directory's parent, which stands for /tmp. The
# test fails if a `.git` appears in the parent or in /tmp itself.
#
# Writers run to completion: test-checkpoint-scoped-format.sh and
# test-issue-1808-worktree-base.sh. test-issue-1671-hook-rejection-repair.sh
# takes about five minutes, so by default only the order of its isolation block
# and its first `git init` is checked; set AMPLIHACK_1937_FULL=1 to run it too.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1937-no-stray-git.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error (a precondition does not hold).

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ORIGINAL_TMPDIR="${TMPDIR:-/tmp}"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS: $1"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL: $1"; }

for tool in git bash jq awk; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "HARNESS-ERROR: ${tool} is required" >&2; exit 2; }
done

# The harness only reads the real /tmp; it never writes there.
if [[ -e /tmp/.git ]]; then
  echo "HARNESS-ERROR: /tmp/.git already exists; this test did not write it, and another process (a Codex sandbox, for one) may have. Remove it before running this test" >&2
  exit 2
fi

[[ "${ORIGINAL_TMPDIR}" == /* && -d "${ORIGINAL_TMPDIR}" ]] || { echo "HARNESS-ERROR: TMPDIR is not an existing absolute directory" >&2; exit 2; }
ARENA="$(mktemp -d "${ORIGINAL_TMPDIR%/}/issue-1937-arena.XXXXXX")" || exit 2
[[ -n "${ARENA}" && -d "${ARENA}" && -O "${ARENA}" ]] || { echo "HARNESS-ERROR: arena is not an owned directory" >&2; exit 2; }
trap 'rm -rf -- "${ARENA:?}"' EXIT

# Run one writer against a fresh parent/TMPDIR pair with a poisoned GIT_DIR.
run_writer() {
  local name="$1" script="$2" parent log
  parent="${ARENA}/${name}"
  mkdir -p "${parent}/tmp" || { fail "${name}: cannot create scratch parent"; return; }
  log="${parent}/output.log"
  env GIT_DIR="${parent}/.git" TMPDIR="${parent}/tmp" bash "${script}" >"${log}" 2>&1
  local rc=$?
  if [[ ${rc} -ne 0 ]]; then
    fail "${name}: writer exited ${rc} with an inherited GIT_DIR"
    sed -n '1,40p' "${log}" >&2
  else
    pass "${name}: writer succeeds with an inherited GIT_DIR"
  fi
  if [[ -e "${parent}/.git" ]]; then
    fail "${name}: created ${parent}/.git outside its TMPDIR"
  else
    pass "${name}: no .git appeared in TMPDIR's parent"
  fi
  if [[ -e /tmp/.git ]]; then
    fail "${name}: created /tmp/.git"
    echo "FAIL: /tmp/.git now exists and was left in place for inspection; another process may have written it" >&2
    exit 1
  fi
  pass "${name}: no .git appeared in /tmp"
}

# The isolation block must come before the first fixture `git init`.
check_isolation_order() {
  local name="$1" script="$2" unset_line init_line
  unset_line="$(awk '/^unset GIT_/ { print NR; exit }' "${script}")"
  init_line="$(awk '!/^[[:space:]]*#/ && /git .*init/ { print NR; exit }' "${script}")"
  if [[ -z "${unset_line}" ]]; then
    fail "${name}: no isolation of GIT_DIR before the fixture"
  elif [[ -n "${init_line}" && "${unset_line}" -ge "${init_line}" ]]; then
    fail "${name}: GIT_DIR is unset at line ${unset_line}, after the first git init at line ${init_line}"
  else
    pass "${name}: GIT_DIR is unset before the first git init"
  fi
}

# A case lookup rather than an associative array: macOS /bin/bash 3.2 has none
# (#1423).
writer_script() {
  case "$1" in
    checkpoint-scoped-format) echo "${SCRIPT_DIR}/test-checkpoint-scoped-format.sh" ;;
    issue-1808-worktree-base) echo "${SCRIPT_DIR}/test-issue-1808-worktree-base.sh" ;;
    issue-1671-hook-rejection-repair) echo "${SCRIPT_DIR}/test-issue-1671-hook-rejection-repair.sh" ;;
    *) return 1 ;;
  esac
}

for name in checkpoint-scoped-format issue-1808-worktree-base issue-1671-hook-rejection-repair; do
  script="$(writer_script "${name}")" || { echo "HARNESS-ERROR: unknown writer ${name}" >&2; exit 2; }
  [[ -f "${script}" ]] || { echo "HARNESS-ERROR: ${script} not found" >&2; exit 2; }
  check_isolation_order "${name}" "${script}"
  if [[ "${name}" != issue-1671-hook-rejection-repair || "${AMPLIHACK_1937_FULL:-0}" == 1 ]]; then
    run_writer "${name}" "${script}"
  fi
done

echo
echo "issue 1937 stray .git: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[[ ${FAIL_COUNT} -eq 0 ]]
