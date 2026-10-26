#!/usr/bin/env bash
# Regression tests for issue #1676 — autodrive-build step-03-resolve-pr stopped
# a run with "phase 1 finished without a pull request" although an agent step
# had opened the PR on the run's own branch, at the run's HEAD
# (mistt-ui-platform #73, #80, #81).
#
# Production layout: REPO_PATH is the main checkout (on main) and phase 1's
# branch lives in a separate `git worktree`, named by worktree_setup, which
# step-02's default-workflow sub-recipe hands back to autodrive-build.
#
# Contract under test (step-03's own command, extracted from the recipe):
#   1. Acceptance: no PR in the preflight, none for REPO_PATH's branch, but an
#      open same-repository PR for the run's branch whose head is the
#      worktree's HEAD -> step-03 resolves it and phase 2 can start.
#   2. Refused (still "phase 1 finished without a pull request"): head is not
#      the worktree's HEAD, a fork PR, the worktree's branch is not
#      worktree_setup.branch_name, the worktree is on main, no worktree_setup,
#      gh fails, malformed JSON.
#   3. A helper planted in REPO_PATH is never sourced: only AMPLIHACK_HOME's.
#   4. A PR the preflight already knows is used as before; the new lookup does
#      not run.
#   5. (#1561) The worktree path and branch come only from the JSON in
#      RECIPE_VAR_worktree_setup. A stale RECIPE_VAR_worktree_setup__* or bare
#      WORKTREE_SETUP_* from a parent run is never read, and a path that is not
#      an already-resolved worktree root (a subdirectory, a symlink, a path
#      through a symlinked parent) skips the lookup with the #1676 INFO line.
#
# Every run uses `env -i`, a sandboxed HOME and stub `gh` / `amplihack`, so no
# real credentials or GitHub are reachable.
#
# Run: bash tests/issue_1676_build_resolves_agent_pr.sh
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD="$REPO_ROOT/amplifier-bundle/recipes/autodrive-build.yaml"
BASH_BIN="$(command -v bash)"
pass=0; fail=0
ok()  { pass=$((pass + 1)); echo "PASS: $1"; }
bad() { fail=$((fail + 1)); echo "FAIL: $1 -- $2"; }

# Resolved (pwd -P), so the run's own worktree passes step-03's resolved-path
# check even when TMPDIR goes through a symlink.
WORK="$(cd "$(mktemp -d)" && pwd -P)"; trap 'rm -rf "$WORK"' EXIT
HOME_D="$WORK/home"; mkdir -p "$HOME_D"
STUB="$WORK/stub"; mkdir -p "$STUB"
GH_LOG="$WORK/gh.log"; OUT="$WORK/out"; ERR="$WORK/err"

# step-03's command, exactly as the recipe runner would run it.
S03="$(awk '
  /^  - id: "step-03-resolve-pr"/ { s=1; next }
  s && /^  - id: / { exit }
  s && /^    command: \|/ { c=1; next }
  s && c { if ($0 ~ /^    [a-zA-Z_]+:/) exit; sub(/^      /, ""); print }' "$BUILD")"
[ -n "$S03" ] || { echo "FAIL: could not extract step-03-resolve-pr"; exit 1; }

# stub amplihack: just the two `orch helper` calls step-03 makes.
cat > "$STUB/amplihack" <<'AMP'
#!/usr/bin/env bash
[ "${1:-} ${2:-}" = "orch helper" ] || { echo "ampstub: unsupported: $*" >&2; exit 2; }
sub="${3:-}"; shift 3; field=""; def=""
while [ $# -gt 0 ]; do
  case "$1" in --require-field|--field) field="${2:-}"; shift 2 ;; --default) def="${2:-}"; shift 2 ;; *) shift ;; esac
done
in="$(cat)"
case "$sub" in
  extract-json) printf '%s' "$in" | jq -e --arg f "$field" 'has($f)' >/dev/null 2>&1 || exit 1; printf '%s\n' "$in" ;;
  extract-field) printf '%s' "$in" | jq -r --arg f "$field" --arg d "$def" '(.[$f] // $d) | tostring' ;;
esac
AMP
# stub gh: `pr list --state open` answers $GH_OPEN_JSON (or fails with
# GH_FAIL=1), except that a query for $GH_STALE_BRANCH answers
# $GH_STALE_JSON (#1561 rows); every other list is empty; `pr view N --json url` gives a URL;
# `pr view` with no number (REPO_PATH's branch) has no PR.
cat > "$STUB/gh" <<'GH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_LOG"
case "$*" in
  "pr list --head "*"--state open"*) [ "${GH_FAIL:-0}" = 1 ] && exit 1
    if [ -n "${GH_STALE_BRANCH:-}" ] && [ "$4" = "$GH_STALE_BRANCH" ]; then printf '%s\n' "${GH_STALE_JSON:-[]}"
    else printf '%s\n' "${GH_OPEN_JSON:-[]}"; fi ;;
  "pr list "*) echo "" ;;
  "pr view --json number"*) exit 1 ;;
  "pr view "[0-9]*" --json url"*) n="${3}"; echo "https://github.com/acme/widget/pull/${n}" ;;
  "pr view "[0-9]*" --json state"*) echo "OPEN" ;;
  *) exit 1 ;;
esac
GH
chmod 0755 "$STUB/amplihack" "$STUB/gh"

# A main checkout on main, and the run's branch in its own worktree.
REPO="$WORK/repo"; mkdir -p "$REPO"
git -C "$REPO" init -q -b main
git -C "$REPO" -c user.email=t@t -c user.name=t commit -q --allow-empty -m base
git -C "$REPO" remote add origin https://github.com/acme/widget.git
RUN_WT="$WORK/wt"; RUN_BRANCH="feat/issue-29-unify"
git -C "$REPO" worktree add -q -b "$RUN_BRANCH" "$RUN_WT"
git -C "$RUN_WT" -c user.email=t@t -c user.name=t commit -q --allow-empty -m work
HEAD_SHA="$(git -C "$RUN_WT" rev-parse HEAD)"
OTHER_SHA="$(git -C "$REPO" rev-parse HEAD)"

PREFLIGHT='{"should_run":"true","pr":"","pr_url":"","pr_state":"","branch":"main","state_dir":"","reason":""}'
pr_json() { printf '[{"number":%s,"headRefName":"%s","headRefOid":"%s","isCrossRepository":%s}]' "$1" "$2" "$3" "$4"; }

# worktree_setup as the runner hands it back: JSON in the base key (#1561).
# Both empty gives "", as for a sub-recipe that produced no worktree.
ws_json() {
  if [ -z "$1" ] && [ -z "$2" ]; then printf ''; return; fi
  jq -nc --arg p "$1" --arg b "$2" '{worktree_path: $p, branch_name: $b, created: true}'
}

# run <home> <wt-path> <wt-branch> [extra env...]
# Own values reach step-03 only as RECIPE_VAR_worktree_setup. Extra env comes
# after the defaults, so a row can override the base key or add stale names.
run() {
  local home="$1" wt="$2" br="$3"; shift 3
  : > "$GH_LOG"
  ( cd "$REPO" && env -i HOME="$HOME_D" PATH="$STUB:/usr/bin:/bin" GH_LOG="$GH_LOG" \
      AMPLIHACK_HOME="$home" REPO_PATH="$REPO" \
      BUILD_PREFLIGHT="${PF:-$PREFLIGHT}" RECIPE_VAR_build_preflight="${PF:-$PREFLIGHT}" \
      NO_MERGE=false RECIPE_VAR_no_merge=false \
      RECIPE_VAR_worktree_setup="$(ws_json "$wt" "$br")" \
      "$@" "$BASH_BIN" -c "$S03" ) > "$OUT" 2> "$ERR"
}
NO_PR="phase 1 finished without a pull request"

# 1. Acceptance.
run "$REPO_ROOT" "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" false)"; rc=$?
if [ "$rc" -eq 0 ] && jq -e '.pr == "73" and (.pr_url | endswith("/pull/73"))' < "$OUT" >/dev/null 2>&1 \
   && grep -q -- "--head $RUN_BRANCH --state open" "$GH_LOG"; then
  ok "acceptance: the agent-opened PR on the run's branch at its HEAD is resolved"
else
  bad "acceptance" "rc=$rc out=$(head -c 300 "$OUT") err=$(head -c 300 "$ERR")"
fi

# 2. Refusals.
refused() { # name, then run args
  local name="$1"; shift; run "$@"; local rc=$?
  if [ "$rc" -eq 1 ] && grep -qF "$NO_PR" "$ERR" && [ ! -s "$OUT" ]; then ok "$name"
  else bad "$name" "rc=$rc out=$(head -c 200 "$OUT") err=$(head -c 300 "$ERR")"; fi
}
refused "head that is not the worktree's HEAD is refused" \
  "$REPO_ROOT" "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$OTHER_SHA" false)"
refused "a fork PR is refused" \
  "$REPO_ROOT" "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" true)"
refused "a worktree whose branch is not worktree_setup.branch_name is refused" \
  "$REPO_ROOT" "$RUN_WT" "feat/something-else" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" false)"
refused "a worktree on main is refused" \
  "$REPO_ROOT" "$REPO" "" GH_OPEN_JSON="$(pr_json 73 main "$OTHER_SHA" false)"
refused "no worktree_setup keeps the old behaviour" \
  "$REPO_ROOT" "" "" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" false)"
refused "a gh failure is refused" \
  "$REPO_ROOT" "$RUN_WT" "$RUN_BRANCH" GH_FAIL=1
refused "malformed JSON is refused" \
  "$REPO_ROOT" "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON='{"number":73}'
refused "a non-numeric PR number is refused" \
  "$REPO_ROOT" "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON="[{\"number\":\"73;id\",\"headRefName\":\"$RUN_BRANCH\",\"headRefOid\":\"$HEAD_SHA\",\"isCrossRepository\":false}]"

# 3. A helper planted in REPO_PATH (the PR's own tree) is never sourced.
PLANT="$WORK/plant"; mkdir -p "$PLANT/amplifier-bundle/tools"
mkdir -p "$REPO/amplifier-bundle/tools"
printf 'autodrive_open_pr_for_run_branch() { echo 999; }\n' > "$REPO/amplifier-bundle/tools/autodrive_run_pr.sh"
refused "a helper planted in REPO_PATH is not sourced (relative AMPLIHACK_HOME)" \
  "amplifier-bundle/.." "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" false)"
refused "no helper under AMPLIHACK_HOME means no lookup" \
  "$PLANT" "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" false)"
rm -rf "$REPO/amplifier-bundle"

# 4. A PR the preflight already knows is used; the new lookup does not run.
PF='{"should_run":"true","pr":"41","pr_url":"","pr_state":"","branch":"main","state_dir":"","reason":""}' \
  run "$REPO_ROOT" "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" false)"; rc=$?
if [ "$rc" -eq 0 ] && jq -e '.pr == "41"' < "$OUT" >/dev/null 2>&1 && ! grep -q -- '--state open' "$GH_LOG"; then
  ok "a preflight PR is used and the run-branch lookup is skipped"
else
  bad "preflight PR" "rc=$rc out=$(head -c 200 "$OUT") gh=$(tr '\n' '|' < "$GH_LOG")"
fi

# 5. #1561: worktree_setup is read only from its base key.
# A second repository stands in for a parent run's checkout. It has its own
# branch, HEAD and open PR (#88), so a step that used any stale name would
# resolve #88 and touch this tree.
STALE="$(mktemp -d "$WORK/stale.XXXXXX")"
STALE_BRANCH="feat/issue-1-parent-run"
git -C "$STALE" init -q -b "$STALE_BRANCH"
git -C "$STALE" -c user.email=t@t -c user.name=t commit -q --allow-empty -m parent
git -C "$STALE" remote add origin https://github.com/acme/widget.git
STALE_SHA="$(git -C "$STALE" rev-parse HEAD)"
STALE_PR="$(pr_json 88 "$STALE_BRANCH" "$STALE_SHA" false)"
snap() { # files, refs, HEAD and status of a tree, for a before/after compare
  { (cd "$1" && find . -path ./.git/logs -prune -o -print | LC_ALL=C sort)
    git -C "$1" for-each-ref --format='%(refname) %(objectname)'
    git -C "$1" rev-parse HEAD; git -C "$1" status --porcelain --ignored; } 2>&1
}
STALE_BEFORE="$(snap "$STALE")"
stale_untouched() { [ "$(snap "$STALE")" = "$STALE_BEFORE" ]; }
stale_not_queried() { ! grep -qF -- "$STALE_BRANCH" "$GH_LOG"; }
SKIP_INFO="INFO: #1676: worktree_setup.worktree_path is not a worktree root; skipping the run-branch lookup [autodrive-build/step-03]"
STALE_ENV=(
  RECIPE_VAR_worktree_setup__worktree_path="$STALE"   # deliberately stale (#1561)
  WORKTREE_SETUP_WORKTREE_PATH="$STALE"               # deliberately stale (#1561)
  RECIPE_VAR_worktree_setup__branch_name="$STALE_BRANCH" # deliberately stale (#1561)
  WORKTREE_SETUP_BRANCH_NAME="$STALE_BRANCH"          # deliberately stale (#1561)
  GH_STALE_BRANCH="$STALE_BRANCH" GH_STALE_JSON="$STALE_PR"
)

# 5-control: the base JSON alone finds the agent PR. Without this a broken
# step-03 would pass every refusal below.
run "$REPO_ROOT" "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" false)"; rc=$?
if [ "$rc" -eq 0 ] && jq -e '.pr == "73"' < "$OUT" >/dev/null 2>&1 \
   && grep -q -- "--head $RUN_BRANCH --state open" "$GH_LOG"; then
  ok "1561-control: worktree_setup JSON in the base key finds the agent PR"
else
  bad "1561-control" "rc=$rc out=$(head -c 300 "$OUT") err=$(head -c 300 "$ERR")"
fi

# 5a: own JSON plus conflicting stale nested and bare names.
run "$REPO_ROOT" "$RUN_WT" "$RUN_BRANCH" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" false)" \
  "${STALE_ENV[@]}"; rc=$?
if [ "$rc" -eq 0 ] && jq -e '.pr == "73"' < "$OUT" >/dev/null 2>&1 \
   && grep -q -- "--head $RUN_BRANCH --state open" "$GH_LOG" && stale_not_queried && stale_untouched; then
  ok "1561-a: own worktree_setup JSON wins over stale __worktree_path/__branch_name and WORKTREE_SETUP_*"
else
  bad "1561-a" "rc=$rc out=$(head -c 200 "$OUT") gh=$(tr '\n' '|' < "$GH_LOG") err=$(head -c 300 "$ERR")"
fi

# 5b: an empty base key plus the stale names: no lookup, no message, stop.
run "$REPO_ROOT" "" "" RECIPE_VAR_worktree_setup="" "${STALE_ENV[@]}"; rc=$?
if [ "$rc" -eq 1 ] && grep -qF "$NO_PR" "$ERR" && [ ! -s "$OUT" ] \
   && stale_not_queried && stale_untouched; then
  ok "1561-b: worktree_setup=\"\" never falls back to stale nested or bare names"
else
  bad "1561-b" "rc=$rc out=$(head -c 200 "$OUT") gh=$(tr '\n' '|' < "$GH_LOG") err=$(head -c 300 "$ERR")"
fi

# skipped <name> <wt-path> <wt-branch> [extra env...]: the path check refuses
# the path, so the lookup is skipped with the INFO line and the run stops.
skipped() {
  local name="$1"; shift; run "$REPO_ROOT" "$@"; local rc=$?
  if [ "$rc" -eq 1 ] && grep -qF "$NO_PR" "$ERR" && grep -qF "$SKIP_INFO" "$ERR" && [ ! -s "$OUT" ] \
     && ! grep -q -- '--state open' "$GH_LOG" && stale_untouched; then ok "$name"
  else bad "$name" "rc=$rc out=$(head -c 200 "$OUT") gh=$(tr '\n' '|' < "$GH_LOG") err=$(head -c 400 "$ERR")"; fi
}

# 5c1: a subdirectory of the run's worktree is not its root.
mkdir -p "$RUN_WT/sub"
skipped "1561-c1: a worktree_path that is a subdirectory is refused by the top-level check" \
  "$RUN_WT/sub" "$RUN_BRANCH" GH_OPEN_JSON="$(pr_json 73 "$RUN_BRANCH" "$HEAD_SHA" false)"
rmdir "$RUN_WT/sub"

# 5c2: a symlink to another repository's root, and a path through a
# symlinked parent. rev-parse --show-toplevel and realpath both follow the
# link, so only the resolved-path check refuses these.
ln -s "$STALE" "$WORK/link-to-other-repo"
skipped "1561-c2: a worktree_path that is a symlink to another repository's root is refused" \
  "$WORK/link-to-other-repo" "$STALE_BRANCH" GH_OPEN_JSON="$STALE_PR"
ln -s "$WORK" "$WORK/linked-parent"
skipped "1561-c2-parent: a worktree_path through a symlinked parent directory is refused" \
  "$WORK/linked-parent/$(basename "$STALE")" "$STALE_BRANCH" GH_OPEN_JSON="$STALE_PR"

echo "issue #1676 build resolves agent-opened PR: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
