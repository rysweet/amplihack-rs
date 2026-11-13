#!/usr/bin/env bash
# Self-checking scenario for issue #1517: the auto-drive merge round reads the
# merge-ready skill's files instead of invoking a skill that refuses agents
# (`disable-model-invocation: true`). Step-00 of the merge round resolves those
# files with or without gadugi-test on PATH, in a Node and in a Rust CLI
# repository. A missing tool never stops auto-drive here; recovering from a
# missing tool is left to the QA agent (PR #1520 review).
#
# Runs, as black boxes:
#   - the SHIPPED amplifier-bundle/tools/autodrive_merge_ready_files.sh against
#     installs laid out in a temporary directory;
#   - the SHIPPED bash of merge round step-00-merge-ready-files, read from the
#     recipe with the extractor beside this file, with and without gadugi-test
#     on PATH, in a Node and a Rust CLI repository.
#
# CI runs this script directly. The gadugi `execute` action in
# tests/gadugi/scenarios/issue-1517-merge-ready-files.yaml launches it with no
# arguments. Prints one PASS or FAIL line per case and ALL_CASES_PASSED when
# every case holds.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESOLVER="$REPO_ROOT/amplifier-bundle/tools/autodrive_merge_ready_files.sh"
ROUND_RECIPE="$REPO_ROOT/amplifier-bundle/recipes/autodrive-merge-round.yaml"
TOP_RECIPE="$REPO_ROOT/amplifier-bundle/recipes/auto-drive-to-merge.yaml"
for f in "$RESOLVER" "$ROUND_RECIPE" "$TOP_RECIPE"; do
  [ -f "$f" ] || { echo "FAIL: missing $f"; echo "SCENARIO_FAILED"; exit 1; }
done
command -v jq >/dev/null 2>&1 || { echo "FAIL: jq is required"; echo "SCENARIO_FAILED"; exit 1; }
SYS_PATH="$(dirname "$(command -v jq)"):$(dirname "$(command -v git)"):/usr/bin:/bin"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
WORK="$(cd "$WORK" && pwd -P)"
mkdir -p "$WORK/home" "$WORK/plain" "$WORK/node" "$WORK/rust" "$WORK/gadugi-bin" "$WORK/no-gadugi-bin"
: > "$WORK/node/package.json"; : > "$WORK/rust/Cargo.toml"
printf '#!/bin/sh\necho "gadugi-test must not run here" >&2\nexit 99\n' > "$WORK/gadugi-bin/gadugi-test"
chmod +x "$WORK/gadugi-bin/gadugi-test"

fail=0
pass() { echo "PASS: $1 — $2"; }
fl() { echo "FAIL: $1 — $2"; fail=1; }

install_skill() { # install_skill <root> [no-template]
  local d="$1/amplifier-bundle/skills/merge-ready"
  mkdir -p "$d"
  printf -- '---\nname: merge-ready\ndisable-model-invocation: true\n---\n# merge-ready\n' > "$d/SKILL.md"
  [ "${2:-}" = "no-template" ] || printf '# PR description template\n' > "$d/pr-description-template.md"
}

# 1. The resolver finds SKILL.md and its template in one install.
install_skill "$WORK/ah"
OUT="$(cd "$WORK/plain" && env -i HOME="$WORK/home" PATH="$SYS_PATH" AMPLIHACK_HOME="$WORK/ah" REPO_PATH="$WORK/plain" \
  bash "$RESOLVER" 2>"$WORK/err1")"; RC=$?
WANT="$WORK/ah/amplifier-bundle/skills/merge-ready"
if [ "$RC" -eq 0 ] && [ "$(printf '%s' "$OUT" | jq -r .skill_md)" = "$WANT/SKILL.md" ] \
   && [ "$(printf '%s' "$OUT" | jq -r .template)" = "$WANT/pr-description-template.md" ] \
   && [ "$(printf '%s' "$OUT" | jq -r .skill_md_sha)" = "$(git hash-object --no-filters "$WANT/SKILL.md")" ]; then
  pass merge_ready_files_resolved "the skill's SKILL.md and template are found in one install, with the SKILL.md hash"
else
  fl merge_ready_files_resolved "rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err1")"
fi

# 2. A SKILL.md without its template is refused; the template is never taken
#    from another install.
install_skill "$WORK/ah2" no-template
install_skill "$WORK/home/.amplihack"
OUT="$(cd "$WORK/plain" && env -i HOME="$WORK/home" PATH="$SYS_PATH" AMPLIHACK_HOME="$WORK/ah2" REPO_PATH="$WORK/plain" \
  bash "$RESOLVER" 2>"$WORK/err2")"; RC=$?
if [ "$RC" -ne 0 ] && [ -z "$OUT" ] && grep -qF 'ERROR: merge-ready-template-not-found:' "$WORK/err2"; then
  pass merge_ready_template_never_mixed "a SKILL.md without its template fails by name instead of mixing installs"
else
  fl merge_ready_template_never_mixed "rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err2")"
fi

# 3. No install at all fails by name, listing what was searched.
OUT="$(cd "$WORK/plain" && env -i HOME="$WORK/plain" PATH="$SYS_PATH" AMPLIHACK_HOME="$WORK/plain" REPO_PATH="$WORK/plain" \
  bash "$RESOLVER" 2>"$WORK/err3")"; RC=$?
if [ "$RC" -ne 0 ] && [ -z "$OUT" ] && grep -qF 'ERROR: merge-ready-skill-files-not-found: searched' "$WORK/err3"; then
  pass merge_ready_missing_install_named "no merge-ready install fails with a named error, never a blocker"
else
  fl merge_ready_missing_install_named "rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err3")"
fi

# 4. The real step-00 body prints the merge-ready file paths as JSON with
#    gadugi-test on PATH, and without it in a Node and a Rust CLI repository.
# The step bodies are read with recipe-step-command.sh, which the Rust suite
# compares with serde_yaml for every step a harness reads (HARNESS_STEPS).
RECIPE="$ROUND_RECIPE"
S00="$(bash "$SCRIPT_DIR/recipe-step-command.sh" "$RECIPE" step-00-merge-ready-files)" || S00=""
run_s00() { # run_s00 <gadugi-bin-dir> [repo]
  env -i HOME="$WORK/home" PATH="$1:$SYS_PATH" AMPLIHACK_HOME="$WORK/ah" REPO_PATH="${2:-$WORK/plain}" \
    RECIPE_VAR_autodrive_tools_dir="$REPO_ROOT/amplifier-bundle/tools" bash -c "$S00"
}
OUT="$(run_s00 "$WORK/gadugi-bin" 2>"$WORK/err5")"; RC=$?
if [ -n "$S00" ] && [ "$RC" -eq 0 ] && [ "$(printf '%s' "$OUT" | jq -r .skill_md 2>/dev/null)" = "$WANT/SKILL.md" ]; then
  pass merge_round_step00_resolves "the shipped step-00 body emits the merge-ready file paths"
else
  fl merge_round_step00_resolves "rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err5")"
fi
OUT="$(run_s00 "$WORK/no-gadugi-bin" "$WORK/node" 2>"$WORK/err6")"; RC=$?
if [ "$RC" -eq 0 ] && [ "$(printf '%s' "$OUT" | jq -r .skill_md)" = "$WANT/SKILL.md" ]; then
  pass merge_round_step00_leaves_tool_recovery_to_agent "trusted criteria lookup does not make required QA tooling optional; the QA agent diagnoses missing tools"
else
  fl merge_round_step00_leaves_tool_recovery_to_agent "rc=$RC out=$OUT"
fi
OUT="$(run_s00 "$WORK/no-gadugi-bin" "$WORK/rust" 2>"$WORK/err6b")"; RC=$?
if [ "$RC" -eq 0 ] && [ "$(printf '%s' "$OUT" | jq -r .skill_md 2>/dev/null)" = "$WANT/SKILL.md" ]; then
  pass merge_round_step00_rust_needs_no_gadugi "step-00 needs no gadugi-test in a Rust CLI repository, as qa-team's Rust CLI line says"
else
  fl merge_round_step00_rust_needs_no_gadugi "rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err6b")"
fi

# Prerequisite recovery is agentic and is validated separately with controlled
# Rust records. This harness never starts an agent or a hosting write.
if [ "$fail" -eq 0 ]; then
  echo "merge-ready read from its files; step-00 resolves them with or without gadugi-test"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
