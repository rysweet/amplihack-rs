#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #1517: the auto-drive merge
# round reads the merge-ready skill's files instead of invoking a skill that
# refuses agents (`disable-model-invocation: true`), and a missing gadugi-test
# stops auto-drive by name before the build (PR #1520 review).
#
# Runs, as black boxes:
#   - the SHIPPED amplifier-bundle/tools/autodrive_merge_ready_files.sh against
#     installs laid out in a temporary directory;
#   - the SHIPPED bash of merge round step-00-merge-ready-files and of
#     auto-drive-to-merge's autodrive-prerequisites, read from the recipes with
#     the extractor beside this file, with and without gadugi-test on PATH.
#
# Launched by the gadugi `execute` action with no arguments; prints one PASS or
# FAIL line per case and ALL_CASES_PASSED when every case holds.
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
mkdir -p "$WORK/home" "$WORK/plain" "$WORK/gadugi-bin" "$WORK/no-gadugi-bin"
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
OUT="$(cd "$WORK/plain" && env -i HOME="$WORK/plain" PATH="$SYS_PATH" REPO_PATH="$WORK/plain" \
  bash "$RESOLVER" 2>"$WORK/err3")"; RC=$?
if [ "$RC" -ne 0 ] && [ -z "$OUT" ] && grep -qF 'ERROR: merge-ready-skill-files-not-found: searched' "$WORK/err3"; then
  pass merge_ready_missing_install_named "no merge-ready install fails with a named error, never a blocker"
else
  fl merge_ready_missing_install_named "rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err3")"
fi

# 4. The merge round reads the files; it never invokes the skill.
ROUND="$ROUND_RECIPE"
if ! grep -qE 'Skill\(skill="?merge-ready' "$ROUND" && grep -qF '{{merge_ready_files.skill_md}}' "$ROUND" \
   && grep -qF '{{merge_ready_files.template}}' "$ROUND"; then
  pass merge_round_reads_skill_files "the merge round reads SKILL.md and the template; it has no Skill(merge-ready) call"
else
  fl merge_round_reads_skill_files "autodrive-merge-round.yaml invokes merge-ready or does not cite its files"
fi

# 5. The real step-00 body: JSON with gadugi-test on PATH, a named stop without.
# The step bodies are read with recipe-step-command.sh, which the Rust suite
# compares with serde_yaml for every step a harness reads (HARNESS_STEPS).
RECIPE="$ROUND_RECIPE"
S00="$(bash "$SCRIPT_DIR/recipe-step-command.sh" "$RECIPE" step-00-merge-ready-files)" || S00=""
run_s00() { # run_s00 <gadugi-bin-dir>
  env -i HOME="$WORK/home" PATH="$1:$SYS_PATH" AMPLIHACK_HOME="$WORK/ah" REPO_PATH="$WORK/plain" \
    AUTODRIVE_TOOLS_DIR="$REPO_ROOT/amplifier-bundle/tools" bash -c "$S00"
}
OUT="$(run_s00 "$WORK/gadugi-bin" 2>"$WORK/err5")"; RC=$?
if [ -n "$S00" ] && [ "$RC" -eq 0 ] && [ "$(printf '%s' "$OUT" | jq -r .skill_md 2>/dev/null)" = "$WANT/SKILL.md" ]; then
  pass merge_round_step00_resolves "the shipped step-00 body emits the merge-ready file paths"
else
  fl merge_round_step00_resolves "rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err5")"
fi
OUT="$(run_s00 "$WORK/no-gadugi-bin" 2>"$WORK/err6")"; RC=$?
if [ "$RC" -ne 0 ] && [ -z "$OUT" ] && grep -q '^ERROR: gadugi-test-not-installed:' "$WORK/err6"; then
  pass merge_round_step00_stops_without_gadugi "step-00 still stops by name if gadugi-test goes away during a run"
else
  fl merge_round_step00_stops_without_gadugi "rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err6")"
fi

# 6. auto-drive-to-merge checks gadugi-test FIRST, before the build.
FIRST="$(grep -m1 -E '^  - id: ' "$TOP_RECIPE" | sed -E 's/^  - id: "?([^"]*)"?.*/\1/')"
RECIPE="$TOP_RECIPE"
PRQ="$(bash "$SCRIPT_DIR/recipe-step-command.sh" "$RECIPE" autodrive-prerequisites)" || PRQ=""
OUT="$(env -i HOME="$WORK/home" PATH="$WORK/no-gadugi-bin:$SYS_PATH" bash -c "$PRQ" 2>"$WORK/err7")"; RC=$?
if [ "$FIRST" = "autodrive-prerequisites" ] && [ -n "$PRQ" ] && [ "$RC" -ne 0 ] && [ -z "$OUT" ] \
   && grep -q '^ERROR: gadugi-test-not-installed:' "$WORK/err7" \
   && grep -qF 'npm install -g github:rysweet/gadugi-agentic-test#6c120657798995b1b53399a5acf3693d418a2d8b' "$WORK/err7"; then
  pass gadugi_missing_stops_before_build "a missing gadugi-test stops auto-drive at its first step, before the build, with the install command"
else
  fl gadugi_missing_stops_before_build "first=$FIRST rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err7")"
fi
OUT="$(env -i HOME="$WORK/home" PATH="$WORK/gadugi-bin:$SYS_PATH" bash -c "$PRQ" 2>"$WORK/err8")"; RC=$?
if [ "$RC" -eq 0 ] && [ "$OUT" = '{"gadugi_test":"found"}' ]; then
  pass gadugi_present_goes_on "with gadugi-test on PATH auto-drive goes on to the build"
else
  fl gadugi_present_goes_on "rc=$RC out=$OUT err=$(tr '\n' ' ' < "$WORK/err8")"
fi

if [ "$fail" -eq 0 ]; then
  echo "merge-ready read from its files; gadugi-test checked before the build"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
