#!/usr/bin/env bash
# test-issue-1543-pr-head-worktree.sh — regression test for #1543.
#
# auto-drive-to-merge run with pr_number from a repository's MAIN checkout
# recorded the main checkout's HEAD as head_sha in every evidence step and ran
# qa-team on that tree instead of the PR head. autodrive-report then printed
# NOT_MERGED with an empty pr after the run really merged, because it read the
# PR only from build_result and the phase outputs only from bare env names.
#
# This builds throwaway, remote-less git repositories under $TMPDIR (a main
# checkout on `main` at commit A, a worktree on `feat/x` at commit B), puts a
# stub `gh` first on PATH, takes the REAL step bodies out of the recipes (not
# copies) and checks that:
#
#   * both loop preflights resolve the PR worktree and its head when pr_number
#     was supplied, and refuse — fail closed, should_run never true — when no
#     worktree has the PR branch checked out;
#   * step-02 of both loops hands that worktree to the loop as --repo and
#     repo_path, and pins AMPLIHACK_HOME to the launch tree when it is unset;
#   * the qa-team step, run where the preflight says, records head_sha=B and
#     runs the scenario runner inside the worktree;
#   * nothing changes when pr_number was not supplied;
#   * a relative repo_path is resolved once, from the directory the run
#     started in, not twice (sub/sub);
#   * through the REAL recipe-runner-rs and the REAL composed recipes
#     (auto-drive-to-merge, autodrive-build, both loops), with only
#     default-workflow, the loop driver and the merge gate stood in for:
#       - a fresh build with no pr_number sends BOTH loops to the same tree,
#         although the runner hands the crusty loop's pr_number back into
#         the parent when that phase returns (T20);
#       - a run whose build and crusty phases are skipped as already done
#         and whose gate merges reports MERGED with the PR number (T21).
#
# The merge_result read in autodrive-report, which the #1543 incident showed
# as NOT_MERGED after a real merge, was fixed by #1511 (PR #1563, cf1f3e71);
# this change does not touch it. T21 checks it end to end.
#
# The stub gh refuses everything but `pr view`; every case asserts that no
# merge was ever attempted. Tests never touch the real checkout.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1543-pr-head-worktree.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd -P)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
TOOL="${REPO_ROOT}/amplifier-bundle/tools/autodrive_pr_head.sh"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

for r in autodrive-merge-loop autodrive-crusty-loop autodrive-merge-evidence auto-drive-to-merge; do
  [[ -f "${RECIPES}/${r}.yaml" ]] || harness_error "missing ${RECIPES}/${r}.yaml"
done
command -v git >/dev/null 2>&1 || harness_error "git is required"
command -v jq >/dev/null 2>&1 || harness_error "jq is required (the gh stub honours --jq with it)"
command -v python3 >/dev/null 2>&1 || harness_error "python3 is required to validate JSON strictly"

# Same helper discovery as test-auto-drive-to-merge.sh: prefer a binary built
# from THIS tree, and require the --require-field pipeline the steps use.
supports_helper() {
  printf '{"a":"b"}' | "$1" orch helper extract-field --field a --default X >/dev/null 2>&1 \
    && printf '{"a":"b"}' | "$1" orch helper extract-json --require-field a >/dev/null 2>&1
}
REAL_AMPLIHACK=""
for cand in "${REPO_ROOT}/target/release/amplihack" "${REPO_ROOT}/target/debug/amplihack" \
            "$(command -v amplihack 2>/dev/null || true)"; do
  [[ -n "${cand}" && -x "${cand}" ]] || continue
  if supports_helper "${cand}"; then REAL_AMPLIHACK="${cand}"; break; fi
done
[[ -n "${REAL_AMPLIHACK}" ]] || {
  echo "HARNESS-ERROR: no 'amplihack' providing 'orch helper extract-json --require-field'." >&2
  echo "  Build it with: cargo build -p amplihack --bin amplihack" >&2; exit 2; }
export REAL_AMPLIHACK

extract_step_command() {
  local recipe="$1" step="$2"
  awk -v step="$step" '
    index($0, "id: \"" step "\"") { instep=1 }
    instep && $0 ~ /^    command: \|/ { incmd=1; next }
    incmd { if ($0 ~ /^    [a-zA-Z_]+:/ || $0 ~ /^  - id:/) { exit } sub(/^      /, ""); print }
  ' "${recipe}"
}
body_of() { # body_of <recipe> <step-id>
  local b
  b="$(extract_step_command "${RECIPES}/$1.yaml" "$2")"
  [[ -n "$b" ]] || harness_error "could not extract $1/$2"
  printf '%s' "$b"
}
MERGE_PREFLIGHT="$(body_of autodrive-merge-loop step-01-merge-loop-preflight)"
MERGE_LOOP="$(body_of autodrive-merge-loop step-02-merge-ready-loop)"
CRUSTY_PREFLIGHT="$(body_of autodrive-crusty-loop step-01-crusty-loop-preflight)"
CRUSTY_LOOP="$(body_of autodrive-crusty-loop step-02-crusty-loop)"
QA_STEP="$(body_of autodrive-merge-evidence step-02-qa-team-scenarios)"
REPORT="$(body_of auto-drive-to-merge autodrive-report)"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }
check_eq() { # check_eq <id> <desc> <want> <got>
  if [[ "$3" == "$4" ]]; then pass "$1" "$2"; else fail "$1" "$2: want '$3', got '$4'"; fi
}

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1543.XXXXXX")" || harness_error "mktemp failed"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT
STUB_BIN="$WORK/bin"; mkdir -p "$STUB_BIN" "$WORK/home"
export GH_LOG="$WORK/gh.log"; : > "$GH_LOG"

# Keep the fixtures independent of the developer's git config and hooks.
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null HOME="$WORK/home"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
unset GIT_DIR GIT_WORK_TREE AMPLIHACK_HOME MERGE_GATE_RESULT BUILD_RESULT CRUSTY_LOOP_RESULT
# This test can itself run inside an auto-drive round, whose environment
# carries that run's PR; none of it may reach the fixtures.
unset PR_NUMBER RECIPE_VAR_pr_number AUTODRIVE_PR_INPUT RECIPE_VAR_autodrive_pr_input EXISTING_BRANCH AUTODRIVE_STATE_DIR

# --- stubs -------------------------------------------------------------------
# gh: answers only `pr view` from STUB_* env, honouring --json/--jq; logs every
# call; refuses (97) anything else, so an attempted merge cannot succeed.
cat > "$STUB_BIN/gh" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${GH_LOG:-/dev/null}"
if [ "${1:-}" != "pr" ] || { [ "${2:-}" != "view" ] && [ "${2:-}" != "list" ]; }; then
  echo "stub gh: refusing '$*'" >&2; exit 97
fi
[ "${STUB_GH_FAIL:-0}" = "1" ] && { echo "stub gh: lookup failed" >&2; exit 1; }
SUB="$2"; shift 2
JQ=""
while [ $# -gt 0 ]; do
  case "$1" in --jq) JQ="${2:-}"; shift 2 ;; *) shift ;; esac
done
OBJ="$(jq -cn --arg oid "${STUB_HEAD_OID:-}" --arg ref "${STUB_HEAD_REF:-}" \
  --arg state "${STUB_STATE:-OPEN}" --arg num "${STUB_PR:-7}" \
  '{number:($num|tonumber),state:$state,mergedAt:null,headRefOid:$oid,headRefName:$ref,baseRefName:"main",url:("https://example.invalid/pull/"+$num)}')"
# `pr list`: the PR exists only once STUB_PR_EXISTS names an existing file
# (the stand-in default-workflow creates it), as after a fresh build.
if [ "$SUB" = list ]; then
  if [ -n "${STUB_PR_EXISTS:-}" ] && [ -e "${STUB_PR_EXISTS}" ]; then OBJ="[$OBJ]"; else OBJ="[]"; fi
fi
if [ -n "$JQ" ]; then printf '%s' "$OBJ" | jq -r "$JQ"; else printf '%s\n' "$OBJ"; fi
STUB
# cargo: the rust-cli qa-team runner; records where it ran.
cat > "$STUB_BIN/cargo" <<'STUB'
#!/usr/bin/env bash
pwd -P > "${CARGO_PWD_MARKER:-/dev/null}"
echo "test result: ok. 1 passed; 0 failed"
STUB
cat > "$STUB_BIN/amplihack" <<'STUB'
#!/usr/bin/env bash
exec "$REAL_AMPLIHACK" "$@"
STUB
chmod +x "$STUB_BIN/gh" "$STUB_BIN/cargo" "$STUB_BIN/amplihack"
export PATH="$STUB_BIN:$PATH"

# A stand-in amplihack home whose autodrive_loop.sh records what step-02 gave it.
FAKE_HOME="$WORK/fakehome"
mkdir -p "$FAKE_HOME/amplifier-bundle/tools"
cat > "$FAKE_HOME/amplifier-bundle/tools/autodrive_loop.sh" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$@" > "${LOOP_ARGV_FILE:-/dev/null}"
printf '%s\n' "${AMPLIHACK_HOME:-<unset>}" > "${LOOP_HOME_FILE:-/dev/null}"
echo '{"loop_result":"DONE"}'
STUB
# #1520: merge step-02 requires the crusty re-review tool next to the loop
# helper. The stub loop never runs it; it only has to exist.
cat > "$FAKE_HOME/amplifier-bundle/tools/autodrive_crusty_rereview.sh" <<'STUB'
#!/usr/bin/env bash
echo '{"before_round_result":"NO_REREVIEW"}'
STUB
# #1570: loop step-02 reads task_description through the shipped
# recipe_context_value.sh, found on the same tool ladder as the loop helper.
cp "$REPO_ROOT/amplifier-bundle/tools/recipe_context_value.sh" "$FAKE_HOME/amplifier-bundle/tools/" \
  || harness_error "recipe_context_value.sh missing from the bundle"
printf '{"task_description":"drive PR 7"}' > "$WORK/context.json"
export AMPLIHACK_CONTEXT_FILE="$WORK/context.json"
# install_fake_tools <dir> — the stub loop helper, re-review tool and the
# context reader.
install_fake_tools() {
  mkdir -p "$1"
  cp "$FAKE_HOME/amplifier-bundle/tools/autodrive_loop.sh" \
     "$FAKE_HOME/amplifier-bundle/tools/autodrive_crusty_rereview.sh" \
     "$FAKE_HOME/amplifier-bundle/tools/recipe_context_value.sh" "$1/"
}

# --- fixture -----------------------------------------------------------------
# make_fixture <name> [worktree-dir-name] -> sets MAIN WT SHA_A SHA_B
MAIN="" WT="" SHA_A="" SHA_B=""
make_fixture() {
  local base="$WORK/$1" wtname="${2:-wt}"
  mkdir -p "$base"
  MAIN="$base/main"; WT="$base/$wtname"
  git init -q -b main "$MAIN" || harness_error "git init failed"
  printf '[package]\nname = "fixture"\n' > "$MAIN/Cargo.toml"
  printf 'a\n' > "$MAIN/file"
  git -C "$MAIN" add -A && git -C "$MAIN" commit -q -m A || harness_error "commit A failed"
  SHA_A="$(git -C "$MAIN" rev-parse HEAD)"
  git -C "$MAIN" branch feat/x
  git -C "$MAIN" worktree add -q "$WT" feat/x || harness_error "worktree add failed"
  printf 'b\n' > "$WT/file"
  git -C "$WT" commit -q -am B || harness_error "commit B failed"
  SHA_B="$(git -C "$WT" rev-parse HEAD)"
  [ "$SHA_A" != "$SHA_B" ] || harness_error "fixture main and PR heads must differ"
  WT="$(cd "$WT" && pwd -P)"
  export STUB_HEAD_OID="$SHA_B" STUB_HEAD_REF="feat/x" STUB_STATE=OPEN STUB_PR=7 STUB_GH_FAIL=0
}

# field <json> <name> — via the same helper pipeline the recipes use.
field() {
  printf '%s' "$1" | "$REAL_AMPLIHACK" orch helper extract-json --require-field "$2" 2>/dev/null \
    | "$REAL_AMPLIHACK" orch helper extract-field --field "$2" --default '' 2>/dev/null
}
real() { (cd "$1" 2>/dev/null && pwd -P) || printf '%s' "$1"; }

OUT="" ERR="" RC=0
# run_body <body> [VAR=value]... — run a step body from the main checkout.
run_body() {
  local body="$1"; shift
  OUT="$(cd "$MAIN" && env "$@" bash -c "$body" 2>"$WORK/stderr")"; RC=$?
  ERR="$(cat "$WORK/stderr")"
}
# preflight <merge|crusty> [VAR=value]...
preflight() {
  local which="$1" body; shift
  if [[ "$which" == merge ]]; then body="$MERGE_PREFLIGHT"; else body="$CRUSTY_PREFLIGHT"; fi
  rm -rf "$WORK/state-$which"
  run_body "$body" REPO_PATH="$MAIN" PR_NUMBER=7 AUTODRIVE_PR_INPUT=7 EXISTING_BRANCH=feat/x \
    AUTODRIVE_STATE_DIR="$WORK/state-$which" AMPLIHACK_HOME="$REPO_ROOT" \
    AMPLIHACK_STATE_DIR="$WORK/amplihack-state" "$@"
}

echo "=== #1543: auto-drive evidence measures the PR head ==="

echo "== T1: started from the main checkout, the preflights resolve the PR worktree =="
make_fixture t1
for loop in merge crusty; do
  preflight "$loop"
  check_eq "T1-$loop-rc" "$loop preflight succeeds" 0 "$RC"
  check_eq "T1-$loop-repo" "$loop preflight repo is the PR worktree" "$WT" "$(real "$(field "$OUT" repo)")"
  check_eq "T1-$loop-sha" "$loop preflight head_sha is the PR head (B, not main's A)" "$SHA_B" "$(field "$OUT" head_sha)"
  check_eq "T1-$loop-run" "$loop preflight still runs the phase" true "$(field "$OUT" should_run)"
done

echo "== T1-QA: qa-team runs in the resolved worktree and records the PR head =="
preflight merge
RESOLVED="$(field "$OUT" repo)"
if [[ -z "$RESOLVED" ]]; then
  fail T1-QA "the merge preflight emitted no repo; the evidence steps would measure the main checkout"
else
  rm -f "$WORK/qa.json" "$WORK/cargo.pwd"
  # #1520: the step runs the launch tree's autodrive_qa_evidence.sh.
  run_body "$QA_STEP" REPO_PATH="$RESOLVED" AMPLIHACK_HOME="$REPO_ROOT" AUTODRIVE_QA_EVIDENCE="$WORK/qa.json" \
    CARGO_PWD_MARKER="$WORK/cargo.pwd" AUTODRIVE_ROUND_LABEL=round-1
  check_eq T1-QA-sha "qa evidence head_sha is the PR head" "$SHA_B" "$(field "$OUT" head_sha)"
  check_eq T1-QA-status "qa-team ran (stub runner passed)" PASS "$(field "$OUT" qa_status)"
  check_eq T1-QA-pwd "the scenario runner ran inside the PR worktree" "$WT" "$(cat "$WORK/cargo.pwd" 2>/dev/null)"
  check_eq T1-QA-file "the evidence file binds to the PR head" "$SHA_B" "$(field "$(cat "$WORK/qa.json" 2>/dev/null)" head_sha)"
fi

echo "== T1-LOOP: step-02 hands the PR worktree to the loop =="
for loop in merge crusty; do
  preflight "$loop"
  PF="$OUT"
  if [[ "$loop" == merge ]]; then body="$MERGE_LOOP"; var=MERGE_LOOP_PREFLIGHT; else body="$CRUSTY_LOOP"; var=CRUSTY_LOOP_PREFLIGHT; fi
  : > "$WORK/argv"; : > "$WORK/home-seen"
  run_body "$body" REPO_PATH="$MAIN" "$var=$PF" AMPLIHACK_HOME="$FAKE_HOME" \
    LOOP_ARGV_FILE="$WORK/argv" LOOP_HOME_FILE="$WORK/home-seen" PR_URL= TASK_DESCRIPTION=t
  check_eq "T1-LOOP-$loop-rc" "$loop step-02 ran the loop" 0 "$RC"
  got_repo="$(awk 'p{print; exit} $0=="--repo"{p=1}' "$WORK/argv")"
  check_eq "T1-LOOP-$loop-repo" "$loop step-02 --repo is the PR worktree" "$WT" "$(real "$got_repo")"
  got_ctx="$(grep -m1 '^repo_path=' "$WORK/argv" | cut -d= -f2-)"
  check_eq "T1-LOOP-$loop-ctx" "$loop step-02 repo_path context is the PR worktree" "$WT" "$(real "$got_ctx")"
  check_eq "T11-$loop-keep" "$loop step-02 never overrides a set AMPLIHACK_HOME" "$FAKE_HOME" "$(cat "$WORK/home-seen")"
done

echo "== T15: preflight read through RECIPE_VAR_ only — step-02 still hands the PR worktree to the loop =="
# #1563 reads step outputs as ${X:-${RECIPE_VAR_x:-}}. If repo were read from
# the bare name only, an empty bare value would send the loop back to the
# launch checkout — the #1543 bug. Both loops must take repo from the fallback.
for loop in merge crusty; do
  preflight "$loop"
  PF="$OUT"
  if [[ "$loop" == merge ]]; then body="$MERGE_LOOP"; var=MERGE_LOOP_PREFLIGHT; rv=RECIPE_VAR_merge_loop_preflight
  else body="$CRUSTY_LOOP"; var=CRUSTY_LOOP_PREFLIGHT; rv=RECIPE_VAR_crusty_loop_preflight; fi
  : > "$WORK/argv"
  run_body "$body" REPO_PATH="$MAIN" "$var=" "$rv=$PF" AMPLIHACK_HOME="$FAKE_HOME" \
    LOOP_ARGV_FILE="$WORK/argv" LOOP_HOME_FILE=/dev/null PR_URL= TASK_DESCRIPTION=t
  check_eq "T15-$loop-rc" "$loop step-02 ran the loop from $rv alone" 0 "$RC"
  got_repo="$(awk 'p{print; exit} $0=="--repo"{p=1}' "$WORK/argv")"
  check_eq "T15-$loop-repo" "$loop step-02 --repo is the PR worktree, not REPO_PATH" "$WT" "$(real "$got_repo")"

  : > "$WORK/argv"
  run_body "$body" REPO_PATH="$MAIN" "$var=" "$rv=" AMPLIHACK_HOME="$FAKE_HOME" \
    LOOP_ARGV_FILE="$WORK/argv" LOOP_HOME_FILE=/dev/null PR_URL= TASK_DESCRIPTION=t
  if [[ "$RC" -ne 0 ]] && [[ "$ERR" == *"no state_dir"* ]] && [[ ! -s "$WORK/argv" ]]; then
    pass "T15-$loop-none" "$loop step-02 with no preflight refuses (rc=$RC) and never runs the loop"
  else
    fail "T15-$loop-none" "$loop step-02 with no preflight: rc=$RC, loop argv: $(tr '\n' ' ' < "$WORK/argv"), stderr: $ERR"
  fi
done

echo "== T11: with AMPLIHACK_HOME unset, step-02 pins it to the launch tree =="
install_fake_tools "$MAIN/amplifier-bundle/tools"
preflight merge
PF="$OUT"
: > "$WORK/home-seen"
run_body "$MERGE_LOOP" REPO_PATH="$MAIN" MERGE_LOOP_PREFLIGHT="$PF" \
  LOOP_ARGV_FILE="$WORK/argv" LOOP_HOME_FILE="$WORK/home-seen" PR_URL= TASK_DESCRIPTION=t
check_eq T11 "the loop sees AMPLIHACK_HOME = the launch checkout, not PR-authored tools" \
  "$(real "$MAIN")" "$(real "$(cat "$WORK/home-seen")")"
rm -rf "$MAIN/amplifier-bundle"

echo "== T11b: tools from ~/.amplihack, PR worktree ships its own tools — AMPLIHACK_HOME pins to ~/.amplihack =="
# The common case outside amplihack-rs: the launch checkout has no bundle, so
# the loop helper comes from ~/.amplihack. A PR that adds amplifier-bundle/tools
# must not become the rounds' tool root.
install_fake_tools "$HOME/.amplihack/amplifier-bundle/tools"
install_fake_tools "$WT/amplifier-bundle/tools"
for loop in merge crusty; do
  preflight "$loop"
  PF="$OUT"
  if [[ "$loop" == merge ]]; then body="$MERGE_LOOP"; var=MERGE_LOOP_PREFLIGHT; else body="$CRUSTY_LOOP"; var=CRUSTY_LOOP_PREFLIGHT; fi
  : > "$WORK/home-seen"
  run_body "$body" REPO_PATH="$MAIN" "$var=$PF" \
    LOOP_ARGV_FILE="$WORK/argv" LOOP_HOME_FILE="$WORK/home-seen" PR_URL= TASK_DESCRIPTION=t
  check_eq "T11b-$loop" "$loop step-02 pins AMPLIHACK_HOME to ~/.amplihack, never the PR worktree" \
    "$(real "$HOME/.amplihack")" "$(real "$(cat "$WORK/home-seen")")"
done
rm -rf "$HOME/.amplihack" "$WT/amplifier-bundle"

echo "== T1-RO: resolution is read-only =="
snap() { { git -C "$MAIN" for-each-ref; git -C "$MAIN" worktree list --porcelain; git -C "$MAIN" status --porcelain; git -C "$WT" status --porcelain; } 2>&1; }
BEFORE="$(snap)"
if [[ -x "$TOOL" || -f "$TOOL" ]]; then
  (cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" >/dev/null 2>&1)
fi
preflight merge; preflight crusty
check_eq T1-RO "refs, worktrees and status are unchanged by resolution" "$BEFORE" "$(snap)"

echo "== T1b: no existing_branch — the worktree is found through headRefName =="
for loop in merge crusty; do
  preflight "$loop" EXISTING_BRANCH=
  check_eq "T1b-$loop" "$loop preflight repo via headRefName" "$WT" "$(real "$(field "$OUT" repo)")"
done

echo "== T7: an option-looking existing_branch is compared literally and refused =="
preflight merge EXISTING_BRANCH=--force
check_eq T7-rc "a --force branch that is not headRefName refuses" 1 "$RC"
check_eq T7-out "nothing on stdout for step-02 to act on" "" "$OUT"
case "$ERR" in
  *"branch-mismatch: --branch '--force'"*) pass T7-msg "stderr names the literal branch" ;;
  *) fail T7-msg "expected a branch-mismatch line naming '--force'; got: $ERR" ;;
esac
check_eq T7-ro "no worktree was created or changed" "$BEFORE" "$(snap)"

echo "== T16: existing_branch differs from headRefName — refuse, never pick its worktree =="
git -C "$MAIN" branch feat/x-2 "$SHA_A"
git -C "$MAIN" worktree add -q "$WORK/t1/wt-x2" feat/x-2 || harness_error "worktree add feat/x-2 failed"
(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x-2 --repo "$MAIN" >"$WORK/tool.out" 2>"$WORK/tool.err"); TRC=$?
check_eq T16-rc "a branch other than headRefName exits 5" 5 "$TRC"
check_eq T16-out "stdout is empty on refusal" "" "$(cat "$WORK/tool.out")"
grep -q "branch-mismatch: --branch 'feat/x-2' is not PR #7's head branch 'feat/x'" "$WORK/tool.err" \
  && pass T16-msg "stderr names both branches" \
  || fail T16-msg "unexpected stderr: $(cat "$WORK/tool.err")"
for loop in merge crusty; do
  preflight "$loop" EXISTING_BRANCH=feat/x-2
  check_eq "T16-$loop-rc" "$loop preflight refuses" 1 "$RC"
  case "$OUT" in *wt-x2*) fail "T16-$loop-out" "the feat/x-2 worktree reached stdout: $OUT" ;;
    *) pass "T16-$loop-out" "the wrong-branch worktree is never handed to the loop" ;; esac
done
git -C "$MAIN" worktree remove --force "$WORK/t1/wt-x2" && git -C "$MAIN" branch -D -q feat/x-2 \
  || harness_error "cannot remove the feat/x-2 fixture"
BEFORE="$(snap)"

echo "== T17: PR_NUMBER empty — the operator's pr_number is used, not a guess =="
: > "$GH_LOG.t17"; GH_LOG_SAVE="$GH_LOG"; export GH_LOG="$GH_LOG.t17"
for loop in merge crusty; do
  preflight "$loop" PR_NUMBER=
  check_eq "T17-$loop-rc" "$loop preflight succeeds" 0 "$RC"
  check_eq "T17-$loop-pr" "$loop preflight pr is AUTODRIVE_PR_INPUT" 7 "$(field "$OUT" pr)"
  check_eq "T17-$loop-repo" "$loop preflight repo is the PR worktree" "$WT" "$(real "$(field "$OUT" repo)")"
done
if grep -q -- '--json number' "$GH_LOG"; then
  fail T17-gh "the launch checkout's PR was guessed although pr_number was supplied"
else
  pass T17-gh "no gh pr view guess from the launch checkout"
fi
cat "$GH_LOG" >> "$GH_LOG_SAVE"; export GH_LOG="$GH_LOG_SAVE"

echo "== T17b: a non-numeric PR_NUMBER falls back to the operator's pr_number =="
for loop in merge crusty; do
  preflight "$loop" PR_NUMBER=n/a
  check_eq "T17b-$loop-rc" "$loop preflight succeeds" 0 "$RC"
  check_eq "T17b-$loop-pr" "$loop preflight pr is AUTODRIVE_PR_INPUT" 7 "$(field "$OUT" pr)"
done

echo "== T18: a non-numeric pr_number with PR_NUMBER empty — refuse with the input named =="
for loop in merge crusty; do
  preflight "$loop" PR_NUMBER= AUTODRIVE_PR_INPUT=abc
  check_eq "T18-$loop-rc" "$loop preflight refuses" 1 "$RC"
  case "$ERR" in *"pr_number 'abc' is not a pull request number"*) pass "T18-$loop-msg" "stderr names the bad input" ;;
    *) fail "T18-$loop-msg" "unexpected stderr: $ERR" ;; esac
done

echo "== T3: pr_number not supplied — unchanged behaviour, no head lookup =="
: > "$GH_LOG.t3"; GH_LOG_SAVE="$GH_LOG"; export GH_LOG="$GH_LOG.t3"
for loop in merge crusty; do
  preflight "$loop" AUTODRIVE_PR_INPUT=
  check_eq "T3-$loop-rc" "$loop preflight succeeds" 0 "$RC"
  check_eq "T3-$loop-repo" "$loop preflight repo is REPO_PATH" "$(real "$MAIN")" "$(real "$(field "$OUT" repo)")"
done
if grep -q headRefOid "$GH_LOG"; then
  fail T3-gh "gh was asked for headRefOid although pr_number was not supplied: $(grep headRefOid "$GH_LOG" | head -1)"
else
  pass T3-gh "no head resolution without pr_number"
fi
cat "$GH_LOG" >> "$GH_LOG_SAVE"; export GH_LOG="$GH_LOG_SAVE"

echo "== T2: no worktree has the PR branch — refuse before the loop =="
make_fixture t2
git -C "$MAIN" worktree remove --force "$WT" || harness_error "worktree remove failed"
for loop in merge crusty; do
  preflight "$loop"
  if [[ "$RC" -ne 0 ]]; then pass "T2-$loop-rc" "$loop preflight exits non-zero (rc=$RC)"; else fail "T2-$loop-rc" "$loop preflight succeeded without a PR worktree"; fi
  if [[ "$ERR" == *"feat/x"* && "$ERR" == *refusing* ]]; then
    pass "T2-$loop-msg" "stderr names the branch and says it is refusing"
  else
    fail "T2-$loop-msg" "stderr must name feat/x and say 'refusing'; got: ${ERR:0:400}"
  fi
  if [[ "$(field "$OUT" should_run)" == true ]]; then fail "T2-$loop-run" "should_run is true"; else pass "T2-$loop-run" "should_run is not true"; fi
done

echo "== T2b: the PR branch is checked out in REPO_PATH itself =="
git -C "$MAIN" checkout -q feat/x || harness_error "checkout feat/x failed"
for loop in merge crusty; do
  preflight "$loop"
  check_eq "T2b-$loop-rc" "$loop preflight succeeds" 0 "$RC"
  check_eq "T2b-$loop-repo" "$loop preflight repo is the main checkout" "$(real "$MAIN")" "$(real "$(field "$OUT" repo)")"
  check_eq "T2b-$loop-sha" "$loop head_sha is the PR head" "$SHA_B" "$(field "$OUT" head_sha)"
done

echo "== T6: a worktree path with a space and a double quote round-trips =="
make_fixture t6 'wt "q" dir'
TOOL_OUT="$(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" 2>/dev/null)"
if printf '%s' "$TOOL_OUT" | python3 -c 'import json,sys; json.loads(sys.stdin.read())' 2>/dev/null; then
  pass T6-tool-json "the tool's JSON line parses"
else
  fail T6-tool-json "the tool's output is not valid JSON: ${TOOL_OUT:0:200}"
fi
check_eq T6-tool-wt "tool worktree round-trips" "$WT" "$(field "$TOOL_OUT" worktree)"
preflight merge
check_eq T6-repo "preflight repo round-trips" "$WT" "$(field "$OUT" repo)"
check_eq T6-run "should_run is the real value, not forged by the path" true "$(field "$OUT" should_run)"

echo "== tool contract: output, exit codes =="
make_fixture tool
TOOL_OUT="$(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" 2>"$WORK/tool.err")"; TRC=$?
check_eq TOOL-rc "exit 0 when the worktree is found" 0 "$TRC"
check_eq TOOL-pr "pr" 7 "$(field "$TOOL_OUT" pr)"
check_eq TOOL-sha "head_sha is headRefOid" "$SHA_B" "$(field "$TOOL_OUT" head_sha)"
check_eq TOOL-ref "head_ref" feat/x "$(field "$TOOL_OUT" head_ref)"
check_eq TOOL-wt "worktree" "$WT" "$(field "$TOOL_OUT" worktree)"
check_eq TOOL-wthead "worktree_head" "$SHA_B" "$(field "$TOOL_OUT" worktree_head)"
check_eq TOOL-match "head_matches" true "$(field "$TOOL_OUT" head_matches)"
check_eq TOOL-reason "reason" ok "$(field "$TOOL_OUT" reason)"
check_eq TOOL-oneline "stdout is exactly one line" 1 "$(printf '%s\n' "$TOOL_OUT" | grep -c .)"

echo "== T13: worktree behind the PR head — warn, do not refuse =="
NEWER="$(printf '%040d' 1 | tr 0 c)"
TOOL_OUT="$(cd "$WORK" && STUB_HEAD_OID="$NEWER" bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" 2>"$WORK/tool.err")"; TRC=$?
check_eq T13-rc "a stale worktree is not a refusal" 0 "$TRC"
check_eq T13-match "head_matches is false" false "$(field "$TOOL_OUT" head_matches)"
check_eq T13-wthead "worktree_head is the worktree's real HEAD" "$SHA_B" "$(field "$TOOL_OUT" worktree_head)"
if grep -q WARNING "$WORK/tool.err" && grep -q "$NEWER" "$WORK/tool.err" && grep -q "$SHA_B" "$WORK/tool.err"; then
  pass T13-warn "stderr WARNING prints both SHAs"
else
  fail T13-warn "expected a WARNING with both SHAs on stderr; got: $(head -c 400 "$WORK/tool.err")"
fi

echo "== T12: two worktrees on the PR branch — refuse as ambiguous =="
make_fixture t12
git -C "$MAIN" worktree add -q -f "$WORK/t12/wt2" feat/x || harness_error "second worktree add failed"
(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" >"$WORK/tool.out" 2>"$WORK/tool.err"); TRC=$?
check_eq T12-rc "ambiguous worktrees exit 3" 3 "$TRC"
check_eq T12-stdout "stdout is empty on failure" "" "$(cat "$WORK/tool.out")"
if grep -q "$WT" "$WORK/tool.err" && grep -q "wt2" "$WORK/tool.err"; then
  pass T12-msg "stderr lists every matching worktree"
else
  fail T12-msg "stderr must list both worktrees; got: $(head -c 400 "$WORK/tool.err")"
fi
preflight merge
if [[ "$RC" -ne 0 && "$(field "$OUT" should_run)" != true ]]; then pass T12-preflight "preflight refuses on ambiguity"; else fail T12-preflight "preflight did not refuse (rc=$RC)"; fi

echo "== T2-tool: no worktree — exit 3 =="
make_fixture t2tool
git -C "$MAIN" worktree remove --force "$WT" || harness_error "worktree remove failed"
(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" >"$WORK/tool.out" 2>"$WORK/tool.err"); TRC=$?
check_eq T2-tool-rc "no PR worktree exits 3" 3 "$TRC"
check_eq T2-tool-stdout "stdout is empty on failure" "" "$(cat "$WORK/tool.out")"

echo "== T8: a worktree record that resolves to another repository is skipped =="
make_fixture t8
OTHER="$WORK/t8/other"
git init -q -b feat/x "$OTHER" && git -C "$OTHER" commit -q --allow-empty -m other || harness_error "other repo failed"
WT_ADMIN="$(git -C "$WT" rev-parse --absolute-git-dir)"
printf '%s/.git\n' "$OTHER" > "$WT_ADMIN/gitdir"
if git -C "$MAIN" worktree list --porcelain | grep -qx "worktree $OTHER"; then
  (cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" >"$WORK/tool.out" 2>"$WORK/tool.err"); TRC=$?
  check_eq T8-rc "a foreign repository's directory is never the PR worktree (exit 3)" 3 "$TRC"
  if grep -q "$OTHER" "$WORK/tool.out"; then fail T8-out "the tool emitted the foreign path"; else pass T8-out "the foreign path is not emitted"; fi
else
  harness_error "T8: could not make the worktree list point at another repository"
fi

echo "== T10: a non-hex headRefOid — exit 4 and refuse =="
make_fixture t10
(cd "$WORK" && STUB_HEAD_OID="not-a-sha" bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" >"$WORK/tool.out" 2>"$WORK/tool.err"); TRC=$?
check_eq T10-rc "non-hex headRefOid exits 4" 4 "$TRC"
preflight merge STUB_HEAD_OID=not-a-sha
if [[ "$RC" -ne 0 && "$(field "$OUT" should_run)" != true ]]; then pass T10-preflight "preflight refuses"; else fail T10-preflight "preflight did not refuse (rc=$RC)"; fi

echo "== T14: gh pr view fails — exit 4 and refuse, no gh output echoed =="
(cd "$WORK" && STUB_GH_FAIL=1 bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" >"$WORK/tool.out" 2>"$WORK/tool.err"); TRC=$?
check_eq T14-rc "a failed lookup exits 4" 4 "$TRC"
if grep -q "lookup failed" "$WORK/tool.err"; then fail T14-leak "gh's own stderr was echoed"; else pass T14-leak "gh's output is summarised, not echoed"; fi
for loop in merge crusty; do
  preflight "$loop" STUB_GH_FAIL=1
  if [[ "$RC" -ne 0 ]]; then pass "T14-$loop" "$loop preflight refuses"; else fail "T14-$loop" "$loop preflight succeeded on a failed lookup"; fi
done

echo "== T19: usage errors exit 2 =="
for args in "--pr abc" "--pr 0" "--pr" ""; do
  # shellcheck disable=SC2086
  (cd "$WORK" && bash "$TOOL" $args --repo "$MAIN" >/dev/null 2>&1); TRC=$?
  check_eq "T19[${args:-<none>}]" "bad --pr is a usage error" 2 "$TRC"
done

echo "== T4: build and crusty skipped as already done, the merge gate merged =="
report() { # report [VAR=value]... -> sets OUT
  OUT="$(env -u BUILD_RESULT -u CRUSTY_LOOP_RESULT -u MERGE_GATE_RESULT -u PR_NUMBER \
    -u RECIPE_VAR_build_result -u RECIPE_VAR_crusty_loop_result -u RECIPE_VAR_merge_gate_result \
    -u RECIPE_VAR_pr_number "$@" bash -c "$REPORT" 2>"$WORK/report.err")"; RC=$?
}
unique_keys() { python3 -c '
import json,sys
def hook(pairs):
    keys=[k for k,_ in pairs]
    if len(keys)!=len(set(keys)): raise SystemExit(1)
    return dict(pairs)
json.loads(sys.stdin.read(), object_pairs_hook=hook)' 2>/dev/null; }
report PR_NUMBER=27 RECIPE_VAR_merge_gate_result='{"merge_result":"MERGED","pr":"27"}'
check_eq T4-outcome "outcome is MERGED" MERGED "$(field "$OUT" outcome)"
check_eq T4-merge "merge_result is MERGED" MERGED "$(field "$OUT" merge_result)"
check_eq T4-pr "pr is the PR number" 27 "$(field "$OUT" pr)"
if grep -q "MERGED — PR #27" "$WORK/report.err"; then pass T4-line "the summary line names the merge and the PR"; else fail T4-line "summary line: $(cat "$WORK/report.err")"; fi

echo "== T5: no pr_number — the PR comes from the merge gate =="
report RECIPE_VAR_merge_gate_result='{"merge_result":"MERGED","pr":"27"}'
check_eq T5-pr "pr from merge_gate_result" 27 "$(field "$OUT" pr)"
check_eq T5-outcome "outcome is MERGED" MERGED "$(field "$OUT" outcome)"

echo "== T5b: PR precedence and RECIPE_VAR_ fallbacks =="
report RECIPE_VAR_build_result='{"pr":"31"}' RECIPE_VAR_merge_gate_result='{"merge_result":"NOT_MERGED"}'
check_eq T5b-build "pr falls back to build_result" 31 "$(field "$OUT" pr)"
report RECIPE_VAR_pr_number=27 BUILD_RESULT='{"pr":"31"}' MERGE_GATE_RESULT='{"merge_result":"ALREADY_MERGED","pr":"29"}'
check_eq T5b-prvar "RECIPE_VAR_pr_number wins over gate and build" 27 "$(field "$OUT" pr)"
check_eq T5b-bare "bare MERGE_GATE_RESULT is still read" MERGED "$(field "$OUT" outcome)"
report RECIPE_VAR_merge_gate_result='{"merge_result":"MERGED","pr":"27"}' RECIPE_VAR_crusty_loop_result='{"loop_result":"DONE"}'
check_eq T5b-crusty "crusty result read from RECIPE_VAR_" DONE "$(field "$OUT" crusty_loop)"
report
check_eq T5b-empty "nothing ran: NOT_MERGED" NOT_MERGED "$(field "$OUT" outcome)"

echo "== T9: injected values cannot forge the report =="
report RECIPE_VAR_pr_number='27","outcome":"MERGED' RECIPE_VAR_merge_gate_result='{"merge_result":"NOT_MERGED"}'
if printf '%s' "$OUT" | unique_keys; then pass T9-json "report is valid JSON with unique keys"; else fail T9-json "report JSON is invalid or has duplicate keys: $OUT"; fi
check_eq T9-outcome "outcome is not forged" NOT_MERGED "$(field "$OUT" outcome)"
case "$(field "$OUT" pr)" in *[!0-9]*) fail T9-pr "pr is not digits-only: $(field "$OUT" pr)" ;; *) pass T9-pr "pr is digits-only" ;; esac
report MERGE_GATE_RESULT='{"merge_result":"MERGED\",\"pr\":\"1","pr":"27"}'
if printf '%s' "$OUT" | unique_keys; then pass T9b-json "report is valid JSON with unique keys"; else fail T9b-json "invalid report JSON: $OUT"; fi
check_eq T9b-outcome "a malformed merge_result is NOT_MERGED" NOT_MERGED "$(field "$OUT" outcome)"

echo "== T22: a relative repo_path is resolved once, from the launch directory =="
make_fixture t22
for loop in merge crusty; do
  if [[ "$loop" == merge ]]; then body="$MERGE_PREFLIGHT"; else body="$CRUSTY_PREFLIGHT"; fi
  rm -rf "$WORK/state-t22"
  OUT="$(cd "$WORK/t22" && env REPO_PATH=main PR_NUMBER=7 AUTODRIVE_PR_INPUT=7 EXISTING_BRANCH=feat/x \
    AUTODRIVE_STATE_DIR="$WORK/state-t22" AMPLIHACK_HOME="$REPO_ROOT" AMPLIHACK_STATE_DIR="$WORK/amplihack-state" \
    bash -c "$body" 2>"$WORK/stderr")"; RC=$?; ERR="$(cat "$WORK/stderr")"
  check_eq "T22-$loop-rc" "$loop preflight with repo_path=main succeeds" 0 "$RC"
  check_eq "T22-$loop-repo" "$loop preflight repo is the PR worktree" "$WT" "$(real "$(field "$OUT" repo)")"
  OUT="$(cd "$WORK/t22" && env REPO_PATH=missing PR_NUMBER=7 AUTODRIVE_PR_INPUT=7 EXISTING_BRANCH=feat/x \
    AUTODRIVE_STATE_DIR="$WORK/state-t22" AMPLIHACK_HOME="$REPO_ROOT" AMPLIHACK_STATE_DIR="$WORK/amplihack-state" \
    bash -c "$body" 2>"$WORK/stderr")"; RC=$?; ERR="$(cat "$WORK/stderr")"
  if [[ "$RC" -ne 0 && "$ERR" == *"cannot cd to REPO_PATH='missing'"* && "$(field "$OUT" should_run)" != true ]]; then
    pass "T22-$loop-missing" "$loop preflight refuses a repo_path it cannot enter"
  else
    fail "T22-$loop-missing" "$loop preflight with repo_path=missing: rc=$RC, out=$OUT, stderr: ${ERR:0:300}"
  fi
done

# --- the composed recipe through the real runner -----------------------------
# Everything above runs step bodies with the environment supplied by hand. The
# defect crusty found on PR #1588 lives between the steps: the runner copies a
# sub-recipe's whole context back into the parent when it returns, so the
# parent's `pr_number` after the crusty loop is build_result.pr. Only a run of
# the composed recipe through the runner exercises that handover.
RUNNER="${RECIPE_RUNNER_RS_PATH:-$(command -v recipe-runner-rs 2>/dev/null || true)}"
[[ -n "$RUNNER" && -x "$RUNNER" ]] \
  || harness_error "recipe-runner-rs not found (set RECIPE_RUNNER_RS_PATH, or run scripts/install-pinned-recipe-runner.sh); T20/T21 cannot run without it"

# e2e_setup <name> — a stand-in amplihack home: the real tools and the real
# auto-drive recipes, with the loop driver, the crusty re-review, the merge
# gate and default-workflow replaced by recorders. Sets E2E_HOME E2E_LOG.
E2E_HOME="" E2E_LOG=""
e2e_setup() {
  E2E_HOME="$WORK/e2e-$1"; E2E_LOG="$E2E_HOME/log"
  mkdir -p "$E2E_HOME/amplifier-bundle/tools" "$E2E_HOME/amplifier-bundle/recipes" "$E2E_LOG"
  cp "$REPO_ROOT"/amplifier-bundle/tools/*.sh "$E2E_HOME/amplifier-bundle/tools/" || harness_error "copy tools"
  for r in auto-drive-to-merge autodrive-build autodrive-crusty-loop autodrive-merge-loop; do
    cp "$RECIPES/$r.yaml" "$E2E_HOME/amplifier-bundle/recipes/" || harness_error "copy $r"
  done
  cat > "$E2E_HOME/amplifier-bundle/tools/autodrive_loop.sh" <<'STUB'
#!/usr/bin/env bash
name=""; prev=""
for a in "$@"; do [ "$prev" = "--loop-name" ] && name="$a"; prev="$a"; done
log="$(cd "$(dirname "$0")/../.." && pwd -P)/log"
printf '%s\n' "$@" > "$log/loop-$name.argv"
echo '{"loop_result":"DONE"}'
STUB
  cat > "$E2E_HOME/amplifier-bundle/tools/autodrive_crusty_rereview.sh" <<'STUB'
#!/usr/bin/env bash
echo '{"before_round_result":"NO_REREVIEW"}'
STUB
  cat > "$E2E_HOME/amplifier-bundle/tools/autodrive_merge_gate.sh" <<'STUB'
#!/usr/bin/env bash
log="$(cd "$(dirname "$0")/../.." && pwd -P)/log"
printf '%s\n' "$@" > "$log/gate.argv"
pr=""; prev=""
for a in "$@"; do [ "$prev" = "--pr" ] && pr="$a"; prev="$a"; done
printf '{"merge_result":"MERGED","pr":"%s"}\n' "$pr"
STUB
  cat > "$E2E_HOME/amplifier-bundle/recipes/default-workflow.yaml" <<STUB
name: "default-workflow"
description: "stand-in for #1543 T20: publishes the PR by creating the marker the gh stub reads"
version: "1.0.0"
steps:
  - id: "stub-build"
    type: "bash"
    command: |
      : > "$E2E_LOG/built"
      echo built
STUB
}
# e2e_run [-c key=value]... — run the composed recipe from the main checkout.
E2E_OUT="" E2E_RC=0
e2e_run() {
  E2E_OUT="$(cd "$MAIN" && env -u AMPLIHACK_CONTEXT_FILE AMPLIHACK_HOME="$E2E_HOME" \
    STUB_PR_EXISTS="$E2E_LOG/built" "$RUNNER" "$E2E_HOME/amplifier-bundle/recipes/auto-drive-to-merge.yaml" \
    -R "$E2E_HOME/amplifier-bundle/recipes" -C "$MAIN" --no-auto-stage --output-format json \
    -c repo_path="$MAIN" -c task_description="drive PR 7" "$@" 2>&1)"; E2E_RC=$?
}
argv_after() { awk -v k="$2" 'p{print; exit} $0==k{p=1}' "$1" 2>/dev/null; }
# summary <field> — the autodrive-report output the runner returned for the run.
summary() {
  printf '%s' "$E2E_OUT" | python3 -c '
import json, sys
d = json.loads(sys.stdin.read())
out = [r["output"] for r in d.get("step_results", []) if r.get("step_id") == "autodrive-report"]
print(json.loads(out[0]).get(sys.argv[1], "") if out else "<no autodrive-report>")' "$1" 2>/dev/null \
    || printf '<runner output is not JSON>'
}

echo "== T20: fresh build, no pr_number — both loops measure the same tree =="
make_fixture t20
e2e_setup t20
e2e_run -c autodrive_state_dir="$WORK/state-t20"
check_eq T20-rc "the composed recipe runs to the end" 0 "$E2E_RC"
[[ "$E2E_RC" -eq 0 ]] || echo "${E2E_OUT: -3000}" >&2
if [[ -e "$E2E_LOG/built" ]]; then pass T20-built "phase 1 ran default-workflow (fresh build)"; else fail T20-built "default-workflow did not run"; fi
CRUSTY_REPO="$(argv_after "$E2E_LOG/loop-crusty.argv" --repo)"
MERGE_REPO="$(argv_after "$E2E_LOG/loop-merge-ready.argv" --repo)"
check_eq T20-crusty "the crusty loop ran in REPO_PATH" "$(real "$MAIN")" "$(real "$CRUSTY_REPO")"
check_eq T20-merge "the merge loop ran in REPO_PATH, not a resolved PR worktree" "$(real "$MAIN")" "$(real "$MERGE_REPO")"
check_eq T20-same "both loops got the same repo" "$(real "$CRUSTY_REPO")" "$(real "$MERGE_REPO")"
check_eq T20-pr "the merge loop drove the PR phase 1 built" 7 "$(grep -m1 '^pr_number=' "$E2E_LOG/loop-merge-ready.argv" 2>/dev/null | cut -d= -f2-)"
check_eq T20-outcome "the run reports MERGED" MERGED "$(summary outcome)"
check_eq T20-report-pr "the run reports PR #7" 7 "$(summary pr)"

echo "== T21: build and crusty skipped as already done, the gate merges — the run reports MERGED =="
make_fixture t21
e2e_setup t21
( . "$E2E_HOME/amplifier-bundle/tools/autodrive_state.sh" \
  && autodrive_private_dir "$WORK/state-t21" && autodrive_mark_phase_done "$WORK/state-t21" crusty-loop ) \
  || harness_error "cannot mark the crusty loop done in the T21 state directory"
e2e_run -c pr_number=7 -c existing_branch=feat/x -c autodrive_state_dir="$WORK/state-t21"
check_eq T21-rc "the composed recipe runs to the end" 0 "$E2E_RC"
[[ "$E2E_RC" -eq 0 ]] || echo "${E2E_OUT: -3000}" >&2
if [[ -e "$E2E_LOG/built" ]]; then fail T21-build "default-workflow ran although PR #7 is open"; else pass T21-build "phase 1 was skipped: PR #7 is already open"; fi
if [[ -e "$E2E_LOG/loop-crusty.argv" ]]; then fail T21-crusty "the crusty loop ran although it was recorded done"; else pass T21-crusty "phase 2 was skipped as already done"; fi
check_eq T21-merge-repo "the merge loop ran in the PR worktree" "$WT" "$(real "$(argv_after "$E2E_LOG/loop-merge-ready.argv" --repo)")"
check_eq T21-gate-pr "the gate was asked about PR #7" 7 "$(argv_after "$E2E_LOG/gate.argv" --pr)"
check_eq T21-outcome "autodrive-report outcome is MERGED" MERGED "$(summary outcome)"
check_eq T21-merge-result "autodrive-report merge_result is MERGED" MERGED "$(summary merge_result)"
check_eq T21-report-pr "autodrive-report pr is 7" 7 "$(summary pr)"
( . "$E2E_HOME/amplifier-bundle/tools/autodrive_state.sh" && autodrive_phase_done "$WORK/state-t21" merged ) \
  && pass T21-recorded "the merge loop recorded the merge" || fail T21-recorded "the merged phase was not recorded"

echo "== no merge was ever attempted =="
if grep -Eq '(^| )merge( |$)' "$GH_LOG" "$GH_LOG".* 2>/dev/null; then
  fail NO-MERGE "gh saw a merge: $(grep -E '(^| )merge( |$)' "$GH_LOG" | head -1)"
else
  pass NO-MERGE "gh was never asked to merge"
fi

echo
echo "Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[ "$FAIL_COUNT" -eq 0 ] || exit 1
exit 0
