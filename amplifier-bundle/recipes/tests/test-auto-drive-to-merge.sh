#!/usr/bin/env bash
# Static recipe parsing, shell portability, and fail-safe parser contracts.
# Agent recovery and real platform actions require separate runtime evidence.
set -uo pipefail

# This test runs inside auto-drive's own qa-team step, whose recipe runner
# exports every step output as AUTODRIVE_*/upper-case and RECIPE_VAR_* names.
# The step bodies once read the bare AUTODRIVE_TOOLS_DIR first and
# RECIPE_VAR_autodrive_tools_dir second,
# so an inherited AUTODRIVE_TOOLS_DIR overrode the per-case tools dir and
# MERGEREADY-no-round-tool found the real tool (PR #1588 round-1 QA, #1607).
# They now read RECIPE_VAR_ only (#1561), and every case sets what it needs.
# By default the test still starts from none of the caller's run context, so
# a developer's shell cannot change its result. AUTODRIVE_TEST_KEEP_INHERITED=1
# skips this removal only (test-only; the #1551 merge guard below and the gh
# stubs stay), which proves the step bodies ignore the inherited names.
if [ "${AUTODRIVE_TEST_KEEP_INHERITED:-}" != "1" ]; then
  while IFS= read -r _inherited; do
    case "${_inherited}" in AUTODRIVE_*|RECIPE_VAR_*) unset "${_inherited}" ;; esac
  done < <(compgen -e)
  unset _inherited
fi
# The same goes for the #1551 gh merge guard. An auto-drive run exports
# BASH_ENV=<guard>/bash-env and AMPLIHACK_MERGE_GUARD=1 to every bash in it,
# this one included, and bash-env has already run before this line: it put
# the guard first on PATH and defined a `gh` function. Left in place, every
# gate this test starts sources it too, and the shim refuses the stub merge
# (PR #1644 crusty round 11: GATE-merge-79 and 1551-gate-merges-under-guard
# failed inside auto-drive). The 1551 cases put the guard back on their own
# command (GUARD_1551). auto_drive_contract_shell_test_passes runs this under
# the guard's environment so a regression here fails `cargo test`.
unset BASH_ENV ENV AMPLIHACK_MERGE_GUARD AMPLIHACK_MERGE_GUARD_USER_BASH_ENV
unset -f gh 2>/dev/null || true
_path=""
while IFS= read -r -d: _entry; do
  case "${_entry}" in */gh-merge-guard|*/gh-merge-guard/) ;; *) _path="${_path:+${_path}:}${_entry}" ;; esac
done <<< "${PATH}:"
PATH="${_path}"; export PATH
unset _path _entry

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
SKILL="${REPO_ROOT}/amplifier-bundle/skills/auto-drive-to-merge/SKILL.md"

AUTODRIVE_RECIPES=(
  auto-drive-to-merge autodrive-build autodrive-crusty-round
  autodrive-crusty-loop autodrive-merge-evidence autodrive-merge-round
  autodrive-merge-loop autodrive-finalize
)
AUTODRIVE_TOOLS=(autodrive_budget.sh autodrive_progress.sh autodrive_crusty_rereview.sh autodrive_done_override.sh autodrive_escalate.sh autodrive_loop.sh autodrive_merge_message.sh autodrive_merge.sh autodrive_round_word.sh
  autodrive_platform_facts.sh autodrive_pr_head.sh autodrive_qa_evidence.sh autodrive_round_evidence.sh autodrive_state.sh
  autodrive_trust.sh autodrive_unmerged.sh autodrive_worktree_guard.sh)
# merge-round step-00-tools-dir finds the tools once and every later step of
# the round reads the directory from its output. A scalar output reaches bash
# as RECIPE_VAR_<output> and as the upper-case name (recipe-runner 0.3.8,
# context.rs shell_env_vars); the step bodies run here get the first form. A
# case that needs another directory, or none, sets it on its own command.
export RECIPE_VAR_autodrive_tools_dir="${TOOLS}"
STATE_HELPER="${TOOLS}/autodrive_state.sh"
# The range and qa evidence checks are new in #1517 too. A missing file fails
# the sections that use it (5k, 6f to 6h) and every other section still runs.
TRUST_HELPER="${TOOLS}/autodrive_trust.sh"

for r in "${AUTODRIVE_RECIPES[@]}"; do
  [[ -f "${RECIPES}/${r}.yaml" ]] || { echo "HARNESS-ERROR: missing ${RECIPES}/${r}.yaml" >&2; exit 2; }
done
for t in autodrive_loop.sh autodrive_state.sh; do
  [[ -f "${TOOLS}/${t}" ]] || { echo "HARNESS-ERROR: missing ${TOOLS}/${t}" >&2; exit 2; }
done
command -v git >/dev/null 2>&1 || { echo "HARNESS-ERROR: git is required to hash crusty round records" >&2; exit 2; }
REAL_GIT="$(command -v git)"
# Every git repository this test creates is isolated from the host: no global
# or system config (so no template hooks, hooksPath or credential helper), and
# a fixed identity passed through the environment.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
[[ -f "${SKILL}" ]] || { echo "HARNESS-ERROR: missing ${SKILL}" >&2; exit 2; }

# The verdict pipeline runs through `orch helper`. Prefer a binary built from
# THIS tree, in target/ or under CARGO_TARGET_DIR, over an older installed one
# that may be first on PATH.
# The gates read verdicts with `extract-json --require-field` (issue #1337,
# PR #1347). A binary without that flag would silently exercise a different,
# fail-open pipeline, so it is part of what "supports the helper" means.
supports_helper() {
  printf '{"a":"b"}' | "$1" orch helper extract-field --field a --default X >/dev/null 2>&1 \
    && printf '{"a":"b"}' | "$1" orch helper extract-json --require-field a >/dev/null 2>&1
}
REAL_AMPLIHACK=""
for cand in "${REPO_ROOT}/target/release/amplihack" "${REPO_ROOT}/target/debug/amplihack" \
            "${CARGO_TARGET_DIR:+${CARGO_TARGET_DIR}/release/amplihack}" \
            "${CARGO_TARGET_DIR:+${CARGO_TARGET_DIR}/debug/amplihack}" \
            "$(command -v amplihack 2>/dev/null || true)"; do
  [[ -n "${cand}" && -x "${cand}" ]] || continue
  if supports_helper "${cand}"; then REAL_AMPLIHACK="${cand}"; break; fi
done
[[ -n "${REAL_AMPLIHACK}" ]] || {
  echo "HARNESS-ERROR: no 'amplihack' providing 'orch helper extract-json --require-field'." >&2
  echo "  Build it with: cargo build -p amplihack --bin amplihack" >&2; exit 2; }
export REAL_AMPLIHACK
echo "amplihack binary: ${REAL_AMPLIHACK}"

for recipe in "${AUTODRIVE_RECIPES[@]}"; do
  "$REAL_AMPLIHACK" recipe show "$RECIPES/$recipe.yaml" >/dev/null || exit 1
done
for tool in "$TOOLS"/autodrive_*.sh; do
  bash -n "$tool" || exit 1
  # GNU-only `chmod MODE -- PATH`; the portable `chmod -- MODE PATH` (#2257) passes.
  if sed '/^[[:space:]]*#/d' "$tool" | grep -E '/proc/|chmod[[:space:]]+[^-[:space:];][^[:space:];]*[[:space:]]+--([[:space:]]|$)' >/dev/null; then
    echo "FAIL: platform-specific prerequisite in $tool" >&2; exit 1
  fi
done
for retired in "$TOOLS/autodrive_merge_gate.sh" "$TOOLS/gh-merge-guard/gh" "$TOOLS/autodrive_merge_ready_files.sh"; do
  [ ! -e "$retired" ] || { echo "FAIL: retired dependency $retired" >&2; exit 1; }
done
# Crusty still fails safe, and the shared evaluator retains its exact tokens.
[ "$(printf '%s' 'unreadable' | "$REAL_AMPLIHACK" orch helper normalise-loop-verdict)" = STUCK ] || exit 1
printf '%s\n' 'PASS: agentic recipes parse, local shell is portable, and malformed loop verdict blocks.'
