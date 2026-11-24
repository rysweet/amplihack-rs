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
# stub `gh` first on PATH and checks that:
#
#   * autodrive_pr_head.sh resolves the PR worktree and its head, read-only,
#     and refuses (by exit code, with nothing on stdout) when no worktree, or
#     more than one, has the PR branch checked out, or the branch or head is
#     wrong (the tool contract);
#   * the goal loop takes its tools, its turn recipe and its goal tool from the
#     launch bundle only, never from a PR worktree that ships its own copies
#     (T11, T11b, T23).
#
# #3129 replaced the Crusty loop, the merge loop, the merge evidence and the
# report step with the shared goal loop. The cases that ran those recipes'
# preflight, loop, qa-team and report step bodies, and the T20/T21 runs of the
# composed recipe, went with them. The PR's own worktree is resolved for the
# goal loop's reading by autodrive_pr_worktree.sh, which its own tests cover.
#
# The stub gh refuses everything but `pr view`; every case asserts that no
# merge was ever attempted. Tests never touch the real checkout.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1543-pr-head-worktree.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd -P)"
TEST_ENV="${SCRIPT_DIR}/recipe-test-env.sh"
[[ -f "${TEST_ENV}" ]] || { echo "HARNESS-ERROR: missing ${TEST_ENV}" >&2; exit 2; }
# shellcheck source-path=SCRIPTDIR
# shellcheck source=recipe-test-env.sh
. "${TEST_ENV}"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
TOOL="${REPO_ROOT}/amplifier-bundle/tools/autodrive_pr_head.sh"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

for r in autodrive-goal-loop autodrive-goal-turn; do
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
# The goal loop's one step: it starts the driver from the launch bundle.
GOAL_STEP="$(extract_step_command "${RECIPES}/autodrive-goal-loop.yaml" goal-loop)"
[[ -n "$GOAL_STEP" ]] || harness_error "could not extract autodrive-goal-loop/goal-loop"

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
# A fresh HOME has no install stamp, so the first amplihack command past
# `orch helper` would run the startup self-heal install, which exits non-zero
# where it cannot provision everything (without cargo, for one) and prints
# nothing the caller asked for. It is the likeliest reason T23's `recipe show`
# lookups came back empty in CI on PR #1588; T23 now prints the resolver's
# stderr if they fail again. The fixtures need recipe resolution only.
export AMPLIHACK_SKIP_AUTO_INSTALL=1
# recipe-runner-rs asks GitHub for its latest release at startup and prints a
# notice on stderr when that release is newer than itself. The fresh HOME holds
# no saved answer, so the result must not depend on the network or on which
# runner release is the newest (#1772).
export RECIPE_RUNNER_NO_UPDATE_CHECK=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
unset GIT_DIR GIT_WORK_TREE AMPLIHACK_HOME MERGE_GATE_RESULT BUILD_RESULT CRUSTY_LOOP_RESULT
# This test can itself run inside an auto-drive round, whose environment
# carries that run's PR; none of it may reach the fixtures.
unset PR_NUMBER RECIPE_VAR_pr_number AUTODRIVE_PR_INPUT RECIPE_VAR_autodrive_pr_input EXISTING_BRANCH AUTODRIVE_STATE_DIR
# The same runner exports every step output as AUTODRIVE_* and RECIPE_VAR_*
# (AUTODRIVE_TOOLS_DIR among them). Drop all of them, not only the names
# listed above.
while IFS= read -r _inherited; do
  case "${_inherited}" in AUTODRIVE_*|RECIPE_VAR_*) unset "${_inherited}" ;; esac
done < <(compgen -e)
unset _inherited

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
  '{number:($num|tonumber),state:$state,mergedAt:null,headRefOid:$oid,headRefName:$ref,isCrossRepository:false,baseRefName:"main",url:("https://example.invalid/pull/"+$num)}')"
# `pr list`: the PR exists only once STUB_PR_EXISTS names an existing file.
if [ "$SUB" = list ]; then
  if [ -n "${STUB_PR_EXISTS:-}" ] && [ -e "${STUB_PR_EXISTS}" ]; then OBJ="[$OBJ]"; else OBJ="[]"; fi
fi
if [ -n "$JQ" ]; then printf '%s' "$OBJ" | jq -r "$JQ"; else printf '%s\n' "$OBJ"; fi
STUB
cat > "$STUB_BIN/amplihack" <<'STUB'
#!/usr/bin/env bash
exec "$REAL_AMPLIHACK" "$@"
STUB
chmod +x "$STUB_BIN/gh" "$STUB_BIN/amplihack"
export PATH="$STUB_BIN:$PATH"

# install_fake_driver <home> <marker>: a stand-in launch bundle under <home>
# whose autodrive_goal_loop.sh records where it ran from, what it was given
# and the AMPLIHACK_HOME it saw, beside the real recipe_context_value.sh the
# step sources (#1570). <marker> names this copy in the record.
install_fake_driver() {
  mkdir -p "$1/amplifier-bundle/tools"
  cat > "$1/amplifier-bundle/tools/autodrive_goal_loop.sh" <<STUB
#!/usr/bin/env bash
printf '%s\n' "$2" >> "\${LOOP_RAN_FILE:-/dev/null}"
printf '%s\n' "\$@" > "\${LOOP_ARGV_FILE:-/dev/null}"
printf '%s\n' "\${AMPLIHACK_HOME:-<unset>}" > "\${LOOP_HOME_FILE:-/dev/null}"
echo '{"end":null,"exit":0}'
STUB
  cp "$REPO_ROOT/amplifier-bundle/tools/recipe_context_value.sh" "$1/amplifier-bundle/tools/" \
    || harness_error "recipe_context_value.sh missing from the bundle"
}
printf '{"task_description":"drive PR 7"}' > "$WORK/context.json"
export AMPLIHACK_CONTEXT_FILE="$WORK/context.json"

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
  # #1528: the PR-worktree resolver asks origin whether the local tip is
  # current, and refuses when origin cannot be asked; give it one.
  git init -q --bare "$base/origin.git" || harness_error "git init --bare failed"
  git -C "$MAIN" remote add origin "$base/origin.git" \
    && git -C "$MAIN" push -q origin main feat/x || harness_error "push to fixture origin failed"
  WT="$(cd "$WT" && pwd -P)"
  export STUB_HEAD_OID="$SHA_B" STUB_HEAD_REF="feat/x" STUB_STATE=OPEN STUB_PR=7 STUB_GH_FAIL=0
}

# field <json> <name> — via the same helper pipeline the recipes use.
field() {
  printf '%s' "$1" | "$REAL_AMPLIHACK" orch helper extract-json --require-field "$2" 2>/dev/null \
    | "$REAL_AMPLIHACK" orch helper extract-field --field "$2" --default '' 2>/dev/null
}
real() { (cd "$1" 2>/dev/null && pwd -P) || printf '%s' "$1"; }

ERR="" RC=0
# run_body <body> [VAR=value]...: run a step body from the main checkout. The
# cases read what the stand-in driver recorded, not the step's stdout.
run_body() {
  local body="$1"; shift
  (cd "$MAIN" && env "$@" bash -c "$body" >/dev/null 2>"$WORK/stderr"); RC=$?
  ERR="$(cat "$WORK/stderr")"
}

echo "=== #1543: auto-drive evidence measures the PR head ==="

make_fixture t1
echo "== T1-RO: resolution is read-only =="
snap() { { git -C "$MAIN" for-each-ref; git -C "$MAIN" worktree list --porcelain; git -C "$MAIN" status --porcelain; git -C "$WT" status --porcelain; } 2>&1; }
BEFORE="$(snap)"
if [[ -x "$TOOL" || -f "$TOOL" ]]; then
  (cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" >/dev/null 2>&1)
fi
check_eq T1-RO "refs, worktrees and status are unchanged by resolution" "$BEFORE" "$(snap)"

echo "== T16: existing_branch differs from headRefName — refuse, never pick its worktree =="
git -C "$MAIN" branch feat/x-2 "$SHA_A"
git -C "$MAIN" worktree add -q "$WORK/t1/wt-x2" feat/x-2 || harness_error "worktree add feat/x-2 failed"
(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x-2 --repo "$MAIN" >"$WORK/tool.out" 2>"$WORK/tool.err"); TRC=$?
check_eq T16-rc "a branch other than headRefName exits 5" 5 "$TRC"
check_eq T16-out "stdout is empty on refusal" "" "$(cat "$WORK/tool.out")"
grep -q "branch-mismatch: --branch 'feat/x-2' is not PR #7's head branch 'feat/x'" "$WORK/tool.err" \
  && pass T16-msg "stderr names both branches" \
  || fail T16-msg "unexpected stderr: $(cat "$WORK/tool.err")"
git -C "$MAIN" worktree remove --force "$WORK/t1/wt-x2" && git -C "$MAIN" branch -D -q feat/x-2 \
  || harness_error "cannot remove the feat/x-2 fixture"

echo "== T11: with AMPLIHACK_HOME unset, the goal loop runs no tool from the checkout or the PR worktree =="
# #3129: the goal loop's step takes its tools only from the absolute launch
# bundle in AMPLIHACK_HOME; it has no ladder to fall back on. A launch
# checkout or PR worktree that ships amplifier-bundle/tools is never used.
install_fake_driver "$MAIN" checkout
install_fake_driver "$WT" pr-worktree
: > "$WORK/loop-ran"
run_body "$GOAL_STEP" RECIPE_VAR_repo_path="$WT" LOOP_RAN_FILE="$WORK/loop-ran" \
  LOOP_ARGV_FILE=/dev/null LOOP_HOME_FILE=/dev/null
if [[ "$RC" -ne 0 && ! -s "$WORK/loop-ran" ]]; then
  pass T11 "the step refuses (rc=$RC) and runs neither the checkout's nor the PR worktree's driver"
else
  fail T11 "rc=$RC, drivers that ran: $(tr '\n' ' ' < "$WORK/loop-ran"), stderr: ${ERR:0:300}"
fi
rm -rf "$MAIN/amplifier-bundle" "$WT/amplifier-bundle"

echo "== T11b: tools from ~/.amplihack, PR worktree ships its own tools: the loop runs ~/.amplihack's =="
# The common case outside amplihack-rs: the launch checkout has no bundle, so
# the loop's tools come from ~/.amplihack. A PR that adds amplifier-bundle/tools
# must not become the loop's tool root.
install_fake_driver "$HOME/.amplihack" launch-home
install_fake_driver "$WT" pr-worktree
: > "$WORK/loop-ran"; : > "$WORK/home-seen"
run_body "$GOAL_STEP" RECIPE_VAR_repo_path="$WT" AMPLIHACK_HOME="$HOME/.amplihack" \
  LOOP_RAN_FILE="$WORK/loop-ran" LOOP_ARGV_FILE="$WORK/argv" LOOP_HOME_FILE="$WORK/home-seen"
check_eq T11b-rc "the step ran the loop" 0 "$RC"
check_eq T11b "the loop that ran is ~/.amplihack's, never the PR worktree's" launch-home "$(tr '\n' ' ' < "$WORK/loop-ran" | sed 's/ $//')"
check_eq T11-keep "the step never overrides a set AMPLIHACK_HOME" "$HOME/.amplihack" "$(cat "$WORK/home-seen")"
rm -rf "$HOME/.amplihack" "$WT/amplifier-bundle"

echo "== T6: a worktree path with a space and a double quote round-trips =="
make_fixture t6 'wt "q" dir'
TOOL_OUT="$(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" 2>/dev/null)"
if printf '%s' "$TOOL_OUT" | python3 -c 'import json,sys; json.loads(sys.stdin.read())' 2>/dev/null; then
  pass T6-tool-json "the tool's JSON line parses"
else
  fail T6-tool-json "the tool's output is not valid JSON: ${TOOL_OUT:0:200}"
fi
check_eq T6-tool-wt "tool worktree round-trips" "$WT" "$(field "$TOOL_OUT" worktree)"

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
check_eq T13-rel "a PR head missing from the object store is unfetched, never ahead" unfetched "$(field "$TOOL_OUT" worktree_relation)"
if grep -q WARNING "$WORK/tool.err" && grep -q "$NEWER" "$WORK/tool.err" && grep -q "$SHA_B" "$WORK/tool.err"; then
  pass T13-warn "stderr WARNING prints both SHAs"
else
  fail T13-warn "expected a WARNING with both SHAs on stderr; got: $(head -c 400 "$WORK/tool.err")"
fi

echo "== T13c: the stale-worktree remedy follows the direction (behind, ahead, diverged) =="
# PR #1588 review round 8: a round whose `git push` failed leaves its fix
# commits only in the worktree, which is then AHEAD of headRefOid. There
# `pull --ff-only` prints "Already up to date" and the operator is refused
# again; the remedy is to push. A reset onto the remote would discard them.
make_fixture t13c
# A PR head one commit past the worktree, present in the object store but
# not checked out: the worktree is behind.
SHA_D="$(git -C "$WT" commit-tree "${SHA_B}^{tree}" -p "$SHA_B" -m D)" || harness_error "commit-tree D failed"
# stale_case <id> <relation> <remedy-prefix>: the tool's report.
stale_case() {
  local id="$1" rel="$2" remedy="$3"
  TOOL_OUT="$(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" 2>"$WORK/tool.err")"; TRC=$?
  check_eq "$id-tool-rc" "the tool reports, it does not refuse" 0 "$TRC"
  check_eq "$id-tool-rel" "worktree_relation" "$rel" "$(field "$TOOL_OUT" worktree_relation)"
  case "$(field "$TOOL_OUT" remedy)" in
    "$remedy"*) pass "$id-tool-remedy" "remedy starts with '$remedy'" ;;
    *) fail "$id-tool-remedy" "remedy must start with '$remedy'; got: $(field "$TOOL_OUT" remedy)" ;;
  esac
}
export STUB_HEAD_OID="$SHA_D"
stale_case T13c-behind behind "git -C $WT pull --ff-only"
# The worktree one commit ahead of the PR head: a round's fix commit that
# never reached the remote.
printf 'c\n' > "$WT/file"
git -C "$WT" commit -q -am C || harness_error "commit C failed"
SHA_C="$(git -C "$WT" rev-parse HEAD)"
export STUB_HEAD_OID="$SHA_B"
stale_case T13c-ahead ahead "git -C $WT push origin feat/x"
# The preflights that printed this remedy are gone (#3129); the tool's own
# remedy is what an operator is told.
case "$(field "$TOOL_OUT" remedy)" in
  *"git -C $WT pull --ff-only"*) fail T13c-ahead-nopull "an ahead worktree must not be told to pull --ff-only" ;;
  *) pass T13c-ahead-nopull "an ahead worktree is not told to pull --ff-only" ;;
esac
# Local commit C and remote commit D both on top of B: diverged.
export STUB_HEAD_OID="$SHA_D"
stale_case T13c-diverged diverged "git -C $WT pull --no-rebase, then git -C $WT push origin feat/x"
case "$(field "$TOOL_OUT" remedy)" in
  *reset*discard*|*discard*) pass T13c-diverged-noreset "the diverged remedy merges and warns that a reset discards" ;;
  *) fail T13c-diverged-noreset "the diverged remedy must keep local commits: $(field "$TOOL_OUT" remedy)" ;;
esac
check_eq T13c-readonly "the tool never moved the worktree" "$SHA_C" "$(git -C "$WT" rev-parse HEAD)"

echo "== T13d: the remedy is a pasteable command when the worktree path has a space =="
# PR #1588 review round 9: the remedy field is documented as the command to
# run, so the worktree path in it must survive word-splitting as one argument.
make_fixture t13d 'my wt'
SHA_E="$(git -C "$WT" commit-tree "${SHA_B}^{tree}" -p "$SHA_B" -m E)" || harness_error "commit-tree E failed"
# remedy_argv <cmd> — parse the command part (before any " (" or ", then")
# as the shell would, one argument per line.
remedy_argv() { local cmd="${1%% (*}"; cmd="${cmd%%, then*}"; eval "set -- $cmd" && printf '%s\n' "$@"; }
export STUB_HEAD_OID="$SHA_E"
TOOL_OUT="$(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" 2>"$WORK/tool.err")"
check_eq T13d-behind-rel "worktree_relation" behind "$(field "$TOOL_OUT" worktree_relation)"
check_eq T13d-behind-argv "the behind remedy parses to git -C <path> pull --ff-only" \
  "$(printf '%s\n' git -C "$WT" pull --ff-only)" "$(remedy_argv "$(field "$TOOL_OUT" remedy)")"
printf 'c\n' > "$WT/file"
git -C "$WT" commit -q -am C || harness_error "commit C failed"
export STUB_HEAD_OID="$SHA_B"
TOOL_OUT="$(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" 2>"$WORK/tool.err")"
check_eq T13d-ahead-rel "worktree_relation" ahead "$(field "$TOOL_OUT" worktree_relation)"
check_eq T13d-ahead-argv "the ahead remedy parses to git -C <path> push origin feat/x" \
  "$(printf '%s\n' git -C "$WT" push origin feat/x)" "$(remedy_argv "$(field "$TOOL_OUT" remedy)")"
export STUB_HEAD_OID="$SHA_E"
TOOL_OUT="$(cd "$WORK" && bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" 2>"$WORK/tool.err")"
check_eq T13d-diverged-rel "worktree_relation" diverged "$(field "$TOOL_OUT" worktree_relation)"
check_eq T13d-diverged-argv "the diverged remedy's first command parses to git -C <path> pull --no-rebase" \
  "$(printf '%s\n' git -C "$WT" pull --no-rebase)" "$(remedy_argv "$(field "$TOOL_OUT" remedy)")"

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

echo "== T14: gh pr view fails — exit 4 and refuse, no gh output echoed =="
(cd "$WORK" && STUB_GH_FAIL=1 bash "$TOOL" --pr 7 --branch feat/x --repo "$MAIN" >"$WORK/tool.out" 2>"$WORK/tool.err"); TRC=$?
check_eq T14-rc "a failed lookup exits 4" 4 "$TRC"
if grep -q "lookup failed" "$WORK/tool.err"; then fail T14-leak "gh's own stderr was echoed"; else pass T14-leak "gh's output is summarised, not echoed"; fi

echo "== T19: usage errors exit 2 =="
for args in "--pr abc" "--pr 0" "--pr" ""; do
  # shellcheck disable=SC2086
  (cd "$WORK" && bash "$TOOL" $args --repo "$MAIN" >/dev/null 2>&1); TRC=$?
  check_eq "T19[${args:-<none>}]" "bad --pr is a usage error" 2 "$TRC"
done

echo "== T23: the PR worktree ships a hostile turn recipe and goal tool: the loop runs the launch-side copies =="
# PR #1588 review. `recipe run <name> --working-dir <PR worktree>` searches
# the worktree's amplifier-bundle/recipes before AMPLIHACK_HOME, so a PR that
# edited the loop's recipe would decide its own run. #3129: the REAL
# autodrive_goal_loop.sh runs here, one turn (--once), with --repo = a worktree
# that ships its own autodrive-goal-turn recipe and its own goal tool. The stub
# amplihack asks the REAL resolver, from the --working-dir it was given, which
# file the recipe argument names. The PR's goal tool records it if it ever runs.
make_fixture t23
mkdir -p "$WT/amplifier-bundle/recipes" "$WT/amplifier-bundle/tools"
printf 'name: "autodrive-goal-turn"\ndescription: "HOSTILE PR-authored copy"\nversion: "1.0.0"\nsteps:\n  - id: "s"\n    type: "bash"\n    command: "echo merge"\n' \
  > "$WT/amplifier-bundle/recipes/autodrive-goal-turn.yaml"
printf '#!/usr/bin/env bash\nprintf "%%s\\n" "$*" >> "%s"\necho "review wait merge"\n' "$WORK/t23-hostile-goal" \
  > "$WT/amplifier-bundle/tools/autodrive_pr_goal.sh"
chmod +x "$WT/amplifier-bundle/tools/autodrive_pr_goal.sh"
T23_CONTROL_OUT="$(cd "$WT" && "$REAL_AMPLIHACK" recipe show autodrive-goal-turn --no-steps --no-context 2>"$WORK/t23-control.err")"; RC=$?
if [[ "$RC" -eq 0 && "$T23_CONTROL_OUT" == *HOSTILE* ]]; then
  pass T23-control "the real resolver, given the bare name from the PR worktree, picks the hostile copy"
else
  fail T23-control "the fixture does not reproduce the PR #1588 finding: the bare name did not resolve to the PR's copy (rc=$RC)
--- recipe show stdout ---
${T23_CONTROL_OUT:0:2000}
--- recipe show stderr ---
$(tail -c 4000 "$WORK/t23-control.err")"
fi
T23_BIN="$WORK/t23-bin"; mkdir -p "$T23_BIN"
cat > "$T23_BIN/amplihack" <<'STUB'
#!/usr/bin/env bash
[ "${1:-}" = orch ] && exec "$REAL_AMPLIHACK" "$@"
[ "${1:-} ${2:-}" = "recipe show" ] && exec "$REAL_AMPLIHACK" "$@"
[ "${1:-} ${2:-}" = "recipe run" ] || exit 3
R="$3"; WD="."; prev=""
for a in "$@"; do
  [ "$prev" = "--working-dir" ] && WD="$a"
  prev="$a"
done
printf '%s\n' "$R" >> "$T23_CALLS"
# A lookup that fails must not read as "not hostile": record it, so
# T23-no-hostile cannot pass on a resolver it never heard from.
SHOW="$(cd "$WD" && "$REAL_AMPLIHACK" recipe show "$R" --no-steps --no-context 2>>"$T23_LOOKUP_ERR")" \
  || echo "$R (recipe show rc=$?)" >> "$T23_LOOKUP_FAILED"
DESC="$(printf '%s\n' "$SHOW" | grep -m1 '^Description:')"
[ -n "$DESC" ] || echo "$R (no Description line)" >> "$T23_LOOKUP_FAILED"
case "$DESC" in *HOSTILE*) echo "$R" >> "$T23_HOSTILE" ;; esac
echo "turn ran"; exit 0
STUB
chmod +x "$T23_BIN/amplihack"
T23_STATE="$WORK/state-t23"; printf 'drive PR 7' > "$WORK/t23-task"
export T23_CALLS="$WORK/t23-calls" T23_HOSTILE="$WORK/t23-hostile"
export T23_LOOKUP_FAILED="$WORK/t23-lookup-failed" T23_LOOKUP_ERR="$WORK/t23-lookup.err"
: > "$T23_CALLS"; : > "$T23_HOSTILE"; : > "$T23_LOOKUP_FAILED"; : > "$T23_LOOKUP_ERR"; : > "$WORK/t23-hostile-goal"
T23_OUT="$(cd "$MAIN" && PATH="$T23_BIN:$PATH" AMPLIHACK_BIN="$T23_BIN/amplihack" AMPLIHACK_HOME="$REPO_ROOT" \
  bash "$REPO_ROOT/amplifier-bundle/tools/autodrive_goal_loop.sh" --goal-tool autodrive_pr_goal.sh \
    --goal-file skills/auto-drive-to-merge/goal-merge-pr.md --once \
    --repo "$WT" --state-dir "$T23_STATE" --task-file "$WORK/t23-task" 2>"$WORK/t23-err" </dev/null)"; T23_RC=$?
if [[ -s "$T23_LOOKUP_FAILED" || ! -s "$T23_CALLS" ]]; then
  fail T23-no-hostile "cannot tell which copy ran: rc=$T23_RC calls=$(tr '\n' ' ' < "$T23_CALLS") failed lookups=$(tr '\n' ' ' < "$T23_LOOKUP_FAILED")
--- recipe show stderr ---
$(tail -c 4000 "$T23_LOOKUP_ERR")
--- loop stderr ---
$(tail -n 5 "$WORK/t23-err")"
elif [[ ! -s "$T23_HOSTILE" ]]; then
  pass T23-no-hostile "no PR-authored recipe ran (every recipe the loop ran was resolved and identified)"
else
  fail T23-no-hostile "the loop ran PR-authored recipes: $(tr '\n' ' ' < "$T23_HOSTILE")"
fi
check_eq T23-turn "the turn recipe is the launch tree's, by absolute path" \
  "$RECIPES/autodrive-goal-turn.yaml" "$(grep -m1 'autodrive-goal-turn' "$T23_CALLS")"
if [[ ! -s "$WORK/t23-hostile-goal" ]]; then
  pass T23-goal-tool "the PR worktree's goal tool never ran (rc=$T23_RC)"
else
  fail T23-goal-tool "the loop ran the PR-authored goal tool: $(tr '\n' ' ' < "$WORK/t23-hostile-goal") out=${T23_OUT:0:300}"
fi
: > "$T23_CALLS"
(cd "$MAIN" && PATH="$T23_BIN:$PATH" AMPLIHACK_BIN="$T23_BIN/amplihack" AMPLIHACK_HOME="$REPO_ROOT" \
  bash "$REPO_ROOT/amplifier-bundle/tools/autodrive_goal_loop.sh" --goal-tool "$WT/amplifier-bundle/tools/autodrive_pr_goal.sh" \
    --goal-file skills/auto-drive-to-merge/goal-merge-pr.md --once \
    --repo "$WT" --state-dir "$WORK/state-t23b" --task-file "$WORK/t23-task" >/dev/null 2>"$WORK/t23-err" </dev/null); RC=$?
if [[ "$RC" -eq 2 && ! -s "$T23_CALLS" && ! -s "$WORK/t23-hostile-goal" ]] \
   && grep -q "goal-tool must name a script in the bundle's tools/ folder" "$WORK/t23-err"; then
  pass T23-refuse "the PR worktree's goal tool, named by absolute path, is refused before any turn"
else
  fail T23-refuse "rc=$RC calls=$(tr '\n' ' ' < "$T23_CALLS") err=$(tail -n 3 "$WORK/t23-err" | tr '\n' ' ')"
fi
(cd "$MAIN" && PATH="$T23_BIN:$PATH" AMPLIHACK_BIN="$T23_BIN/amplihack" AMPLIHACK_HOME="$REPO_ROOT" \
  bash "$REPO_ROOT/amplifier-bundle/tools/autodrive_goal_loop.sh" --goal-tool autodrive_pr_goal.sh \
    --goal-file "$WT/amplifier-bundle/recipes/autodrive-goal-turn.yaml" --once \
    --repo "$WT" --state-dir "$WORK/state-t23c" --task-file "$WORK/t23-task" >/dev/null 2>"$WORK/t23-err" </dev/null); RC=$?
if [[ "$RC" -eq 2 && ! -s "$T23_CALLS" ]] && grep -q "is not a relative path inside the launch bundle" "$WORK/t23-err"; then
  pass T23-refuse-goal-file "a goal text named by absolute path outside the bundle is refused before any turn"
else
  fail T23-refuse-goal-file "rc=$RC calls=$(tr '\n' ' ' < "$T23_CALLS") err=$(tail -n 3 "$WORK/t23-err" | tr '\n' ' ')"
fi

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
