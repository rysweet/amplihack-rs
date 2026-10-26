#!/usr/bin/env bash
# Regression tests for issue #1680 — a fresh auto-drive run (no pr_number, no
# existing_branch) kept its whole state under a key made from the base
# checkout's branch (…_mistt-ui-platform_main), so two fresh runs in one
# repository at once shared verdict files.
#
# Contract under test (autodrive-build's own step-01 and step-03 commands,
# extracted from the recipe):
#   1. Acceptance: a fresh run's build_result.state_dir is the directory a
#      later pr_number=N run's preflight resolves to, and phase 1's "build"
#      mark is written there, not in the base-branch directory.
#   2. Acceptance: two fresh runs that open different PRs get different state
#      directories.
#   3. A PR the preflight already knew keeps its directory.
#   4. An explicit autodrive_state_dir is kept.
#
# Every run uses `env -i`, a sandboxed HOME and state root, and stub `gh` /
# `amplihack`, so no real credentials, state or GitHub are reachable.
#
# Run: bash tests/issue_1680_fresh_run_state_key.sh
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD="$REPO_ROOT/amplifier-bundle/recipes/autodrive-build.yaml"
BASH_BIN="$(command -v bash)"
pass=0; fail=0
ok()  { pass=$((pass + 1)); echo "PASS: $1"; }
bad() { fail=$((fail + 1)); echo "FAIL: $1 -- $2"; }

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
HOME_D="$WORK/home"; mkdir -p "$HOME_D"
STATE_ROOT="$WORK/state"
STUB="$WORK/stub"; mkdir -p "$STUB"
OUT="$WORK/out"; ERR="$WORK/err"

step_cmd() {
  awk -v id="$1" '
    $0 == "  - id: \"" id "\"" { s=1; next }
    s && /^  - id: / { exit }
    s && /^    command: \|/ { c=1; next }
    s && c { if ($0 ~ /^    [a-zA-Z_]+:/) exit; sub(/^      /, ""); print }' "$BUILD"
}
S01="$(step_cmd step-01-build-preflight)"
S03="$(step_cmd step-03-resolve-pr)"
[ -n "$S01" ] && [ -n "$S03" ] || { echo "FAIL: could not extract step-01 / step-03"; exit 1; }

# stub amplihack: the `orch helper` calls the steps make.
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
# stub gh: `pr list --head` answers $GH_PR, which only step-03 runs set, so
# the preflight sees no PR (a fresh run) and step-03 finds the one phase 1
# opened. Numbered views answer OPEN and a URL.
cat > "$STUB/gh" <<'GH'
#!/usr/bin/env bash
case "$*" in
  "pr list --head "*) echo "${GH_PR:-}" ;;
  "pr view --json number"*) exit 1 ;;
  "pr view "[0-9]*" --json url"*) echo "https://github.com/acme/widget/pull/$3" ;;
  "pr view "[0-9]*" --json state"*) echo '{"state":"OPEN","mergedAt":null}' ;;
  *) exit 1 ;;
esac
GH
chmod 0755 "$STUB/amplihack" "$STUB/gh"

REPO="$WORK/repo"; mkdir -p "$REPO"
git -C "$REPO" init -q -b main
git -C "$REPO" -c user.email=t@t -c user.name=t commit -q --allow-empty -m base
git -C "$REPO" remote add origin https://github.com/acme/widget.git

# run <cmd> [extra env...]: one step, from REPO_PATH, sandboxed.
run() {
  local cmd="$1"; shift
  ( cd "$REPO" && env -i HOME="$HOME_D" PATH="$STUB:/usr/bin:/bin" \
      AMPLIHACK_HOME="$REPO_ROOT" AMPLIHACK_STATE_DIR="$STATE_ROOT" REPO_PATH="$REPO" \
      NO_MERGE=false RECIPE_VAR_no_merge=false \
      "$@" "$BASH_BIN" -c "$cmd" ) > "$OUT" 2> "$ERR"
}
field() { jq -r --arg f "$1" '.[$f]' < "$OUT"; }

# preflight <extra env...> -> prints the preflight JSON
preflight() { run "$S01" "$@" && cat "$OUT"; }
# fresh_run <pr> -> prints build_result.state_dir
fresh_run() {
  local pf
  pf="$(preflight)" || { echo "preflight failed: $(head -c 300 "$ERR")" >&2; return 1; }
  run "$S03" BUILD_PREFLIGHT="$pf" RECIPE_VAR_build_preflight="$pf" GH_PR="$1" \
      RECIPE_VAR_worktree_setup__worktree_path="" || { echo "step-03 failed: $(head -c 300 "$ERR")" >&2; return 1; }
  field state_dir
}

# 1. Acceptance: fresh run -> the later PR run's directory.
BASE_DIR="$(preflight | jq -r .state_dir)"
D73="$(fresh_run 73)"
LATER="$(preflight PR_NUMBER=73 RECIPE_VAR_pr_number=73 | jq -r .state_dir)"
if [ -n "$D73" ] && [ "$D73" = "$LATER" ] && [ "$D73" != "$BASE_DIR" ] && grep -q '^build' "$D73/phases.tsv" 2>/dev/null \
   && [ ! -e "$BASE_DIR/phases.tsv" ]; then
  ok "acceptance: a fresh run's state_dir is the one a later pr_number=73 run resolves to"
else
  bad "acceptance: fresh run keyed by PR" "base=$BASE_DIR d73=$D73 later=$LATER err=$(head -c 300 "$ERR")"
fi
case "$BASE_DIR" in *_main) ok "the preflight still keys a PR-less run by REPO_PATH's branch until the PR is known" ;;
  *) bad "preflight key" "base=$BASE_DIR" ;; esac

# 2. Acceptance: two fresh runs -> two directories.
D74="$(fresh_run 74)"
if [ -n "$D74" ] && [ "$D74" != "$D73" ]; then ok "acceptance: two fresh runs in one repository get different state directories"
else bad "two fresh runs" "d73=$D73 d74=$D74"; fi

# 3. A preflight-known PR keeps its directory.
PF="$(preflight PR_NUMBER=73 RECIPE_VAR_pr_number=73)"
run "$S03" BUILD_PREFLIGHT="$PF" RECIPE_VAR_build_preflight="$PF"
if [ "$(field state_dir)" = "$LATER" ] && ! grep -q '#1680' "$ERR"; then ok "a PR run keeps the preflight's directory"
else bad "PR run" "out=$(head -c 200 "$OUT") err=$(head -c 200 "$ERR")"; fi

# 4. An explicit autodrive_state_dir is kept.
EXPLICIT="$WORK/explicit-state"
PF="$(preflight AUTODRIVE_STATE_DIR="$EXPLICIT")"
run "$S03" BUILD_PREFLIGHT="$PF" RECIPE_VAR_build_preflight="$PF" GH_PR=75 \
    AUTODRIVE_STATE_DIR="$EXPLICIT" RECIPE_VAR_autodrive_state_dir="$EXPLICIT"
if [ "$(field state_dir)" = "$EXPLICIT" ] && [ "$(field pr)" = "75" ]; then ok "an explicit autodrive_state_dir is kept"
else bad "explicit dir" "out=$(head -c 200 "$OUT") err=$(head -c 200 "$ERR")"; fi

echo "issue #1680 fresh-run state key: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
