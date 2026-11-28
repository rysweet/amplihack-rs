#!/usr/bin/env bash
# Behaviour coverage for issue #3424. The loop cases use a stand-in agent and
# guidance waiter. The build cases exercise the build-run entry point with a
# temporary Git work root.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
TOOLS="$ROOT/amplifier-bundle/tools"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-3424.XXXXXX")" || exit 2
trap 'rm -rf -- "$WORK"' EXIT
PASS=0; FAIL=0
ok() { local name="$1"; shift; if "$@"; then PASS=$((PASS + 1)); echo "PASS $name"; else FAIL=$((FAIL + 1)); echo "FAIL $name" >&2; fi; }

make_loop_case() {
  local name="${1:-loop}" c base wrapper
  c="$WORK/$name"; base="$WORK/bin/amplihack"; wrapper="$WORK/bin/amplihack-wrapper"
  mkdir -p "$c" "$WORK/bin" "$WORK/home" "$c/bundle/tools" "$c/bundle/recipes"
  cp "$ROOT/tests/issue_1437_amplihack_stub.sh" "$base"; chmod 755 "$base"
  cp -R "$TOOLS"/* "$c/bundle/tools/"
  cp -R "$ROOT/amplifier-bundle/recipes"/* "$c/bundle/recipes/"
  printf '%s\n' 'issue 3424 fixture goal' > "$c/bundle/goal.md"
  cat > "$c/bundle/tools/issue_3424_goal.sh" <<'GOAL'
#!/usr/bin/env bash
[ "${AUTODRIVE_GOAL_CONTRACT:-}" = 1 ] || exit 2
case "${1:-}" in
  actions) echo poke ;;
  final) n="$(cat "${STUB_STATE:?}/rounds" 2>/dev/null || echo 0)"; [ "$n" -lt 3 ] || echo fixture-end ;;
  read) printf '{"schema_version":1,"summary":"fixture"}\n' > "$2/goal.json" ;;
  publish) n="$(cat "${STUB_STATE}/publishes" 2>/dev/null || echo 0)"; printf '%s' "$((n + 1))" > "${STUB_STATE}/publishes" ;;
  *) exit 2 ;;
esac
GOAL
  chmod 755 "$c/bundle/tools/issue_3424_goal.sh"
  cat > "$wrapper" <<'WRAP'
#!/usr/bin/env bash
set -u
if [ "${1:-} ${2:-} ${3:-}" = "recipe guidance show" ]; then
  printf '{"entries":[],"newest":""}\n'
  exit 0
fi
if [ "${1:-} ${2:-} ${3:-}" = "recipe guidance wait" ]; then
  if [ -n "${AUTODRIVE_STATE_DIR:-}" ]; then
    n="$(cat "${STUB_STATE}/owner_waits" 2>/dev/null || echo 0)"
    printf '%s' "$((n + 1))" > "${STUB_STATE}/owner_waits"
  else
    n="$(cat "${STUB_STATE}/waits" 2>/dev/null || echo 0)"
    printf '%s' "$((n + 1))" > "${STUB_STATE}/waits"
  fi
  printf '{"event":"guidance","entry":"fixture-change"}\n'
  exit 0
fi
if [ "${1:-} ${2:-} ${3:-}" = "config get recipe.usage_limit_wait" ]; then
  printf 'true\n'
  exit 0
fi
exec "${BASE_STUB:?}" "$@"
WRAP
  chmod 755 "$wrapper"
  printf '%s\n' "$c|$wrapper|$base"
}

IFS='|' read -r LOOP_CASE LOOP_BIN LOOP_BASE < <(make_loop_case environmental)
MARK='amplihack.recipe.failure_class '
OUT1="${MARK}{\"class\":\"environmental\",\"signal\":\"untrusted bundle\",\"retryable\":false,\"completed_steps\":[],\"failed_steps\":[]}"
export BASE_STUB="$LOOP_BASE" STUB_STATE="$LOOP_CASE/stub" STUB_RC_1=1 STUB_RC_2=1 STUB_OUT_1="$OUT1" STUB_OUT_2="$OUT1"
mkdir -p "$STUB_STATE"
touch "$LOOP_CASE/run.log"
set +e
( cd "$LOOP_CASE" && HOME="$WORK/home" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 PATH="$WORK/bin:$PATH" \
    AMPLIHACK_BIN="$LOOP_BIN" AMPLIHACK_RECIPE_LOG="$LOOP_CASE/run.log" bash "$LOOP_CASE/bundle/tools/autodrive_goal_loop.sh" \
      --goal-tool issue_3424_goal.sh --goal-file goal.md --repo "$LOOP_CASE" --state-dir "$LOOP_CASE/state" </dev/null ) \
  > "$LOOP_CASE/out" 2> "$LOOP_CASE/err"
LOOP_RC=$?
set -e
ok repeated_pre_agent_wait test "$LOOP_RC" -eq 0
ok one_handoff test "$(cat "$STUB_STATE/publishes" 2>/dev/null || echo 0)" -eq 1
ok one_owner_wait test "$(cat "$STUB_STATE/owner_waits" 2>/dev/null || echo 0)" -eq 1
ok fresh_turn_after_guidance test "$(cat "$STUB_STATE/rounds" 2>/dev/null || echo 0)" -eq 3
ok failure_detail_recorded jq -e '.failure.class == "environmental" and .failure.detail == "untrusted bundle"' "$LOOP_CASE/state/turns/001/result.json" >/dev/null
ok wait_result_is_guidance jq -e '.outcome == "guidance-arrived"' "$LOOP_CASE/state/turns/002/result.json" >/dev/null
ok each_failed_turn_is_logged test "$(wc -l < "$LOOP_CASE/run.log")" -ge 2

# Usage-limit failures emitted before the decide step take the same pairwise
# owner wait as every other structured pre-agent failure.
IFS='|' read -r LIMIT_CASE LIMIT_BIN LIMIT_BASE < <(make_loop_case usage_limit)
LIMIT_OUT='amplihack.recipe.failure_class {"class":"usage_limit","signal":"ERROR: Your workspace is out of credits. Add credits to continue.","retryable":false,"completed_steps":[],"failed_steps":[]}'
export BASE_STUB="$LIMIT_BASE" STUB_STATE="$LIMIT_CASE/stub" STUB_RC_1=1 STUB_RC_2=1 STUB_OUT_1="$LIMIT_OUT" STUB_OUT_2="$LIMIT_OUT"
mkdir -p "$STUB_STATE"
set +e
( cd "$LIMIT_CASE" && HOME="$WORK/home" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 PATH="$WORK/bin:$PATH" \
    AMPLIHACK_BIN="$LIMIT_BIN" bash "$LIMIT_CASE/bundle/tools/autodrive_goal_loop.sh" \
      --goal-tool issue_3424_goal.sh --goal-file goal.md --repo "$LIMIT_CASE" --state-dir "$LIMIT_CASE/state" </dev/null ) \
  > "$LIMIT_CASE/out" 2> "$LIMIT_CASE/err"
LIMIT_RC=$?
set -e
ok usage_limit_pre_agent_wait test "$LIMIT_RC" -eq 0
ok usage_limit_one_handoff test "$(cat "$STUB_STATE/publishes" 2>/dev/null || echo 0)" -eq 1
ok usage_limit_one_owner_wait test "$(cat "$STUB_STATE/owner_waits" 2>/dev/null || echo 0)" -eq 1
ok usage_limit_fresh_turn_after_guidance test "$(cat "$STUB_STATE/rounds" 2>/dev/null || echo 0)" -eq 3

run_marker_case() {
  local label="$1" class="$2" first="$3" second="$4" third="${5:-}" expected_wait="$6" marker1 marker2 marker3
  IFS='|' read -r CASE BIN BASE < <(make_loop_case "$label")
  marker1="amplihack.recipe.failure_class {\"class\":\"$class\",\"signal\":\"$first\",\"retryable\":false,\"completed_steps\":[\"read\"],\"failed_steps\":[\"decide\"]}"
  marker2=""
  [ -n "$second" ] && marker2="amplihack.recipe.failure_class {\"class\":\"$class\",\"signal\":\"$second\",\"retryable\":false,\"completed_steps\":[\"read\"],\"failed_steps\":[\"decide\"]}"
  marker3=""
  [ -n "$third" ] && marker3="amplihack.recipe.failure_class {\"class\":\"$class\",\"signal\":\"$third\",\"retryable\":false,\"completed_steps\":[\"read\"],\"failed_steps\":[\"decide\"]}"
  export BASE_STUB="$BASE" STUB_STATE="$CASE/stub" STUB_RC_1=1 STUB_RC_2=1 STUB_RC_3=1 STUB_OUT_1="$marker1" STUB_OUT_2="$marker2" STUB_OUT_3="$marker3"
  mkdir -p "$STUB_STATE"
  set +e
  ( cd "$CASE" && HOME="$WORK/home" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 PATH="$WORK/bin:$PATH" \
      AMPLIHACK_BIN="$BIN" bash "$CASE/bundle/tools/autodrive_goal_loop.sh" \
        --goal-tool issue_3424_goal.sh --goal-file goal.md --repo "$CASE" --state-dir "$CASE/state" </dev/null ) \
    > "$CASE/out" 2> "$CASE/err"
  rc=$?
  set -e
  ok "${label}_exit" test "$rc" -eq 0
  ok "${label}_waits" test "$(cat "$STUB_STATE/owner_waits" 2>/dev/null || echo 0)" -eq "$expected_wait"
  ok "${label}_rounds" test "$(cat "$STUB_STATE/rounds" 2>/dev/null || echo 0)" -eq 3
}

run_marker_case failed_decide_same environmental "workspace unavailable" "workspace unavailable" "" 1
run_marker_case failed_decide_usage_limit usage_limit "ERROR: Your workspace is out of credits. Add credits to continue." "ERROR: Your workspace is out of credits. Add credits to continue." "" 1
run_marker_case failed_decide_different environmental "workspace unavailable" "different workspace failure" "" 0
run_marker_case failed_decide_unmarked environmental "workspace unavailable" "" "workspace unavailable" 0

make_build_case() {
  local name="$1" mode="$2" c="$WORK/$1" main="$WORK/$1/main" candidate="$WORK/$1/candidate" bundle="$WORK/$1/bundle"
  local remote alias_branch
  mkdir -p "$c" "$bundle/tools" "$bundle/recipes" "$c/home"
  git init -q "$main"
  git -C "$main" config user.name test; git -C "$main" config user.email test@example.com
  printf 'base\n' > "$main/file"; git -C "$main" add file; git -C "$main" commit -q -m base
  head="$(git -C "$main" rev-parse HEAD)"
  if [ "$mode" = alias ] || [ "$mode" = alias-wrong-upstream ]; then
    remote="$c/origin.git"
    git init --bare -q "$remote"
    git -C "$main" remote add origin "$remote"
    git -C "$main" checkout -q -b issue-3424
    git -C "$main" push -q -u origin issue-3424
    if [ "$mode" = alias-wrong-upstream ]; then
      git -C "$main" branch other "$head"
      git -C "$main" push -q origin other
    fi
    alias_branch=issue-3424--wt-1
    git -C "$main" worktree add -q --track -b "$alias_branch" "$candidate" origin/issue-3424
    if [ "$mode" = alias-wrong-upstream ]; then
      git -C "$main" branch --set-upstream-to=origin/other "$alias_branch" >/dev/null
    fi
  else
    git -C "$main" worktree add -q -b issue-3424 "$candidate" "$head"
  fi
  if [ "$mode" = other-head ]; then printf 'changed\n' >> "$candidate/file"; git -C "$candidate" add file; git -C "$candidate" commit -q -m changed; fi
  cp -R "$TOOLS"/* "$bundle/tools/"
  cp -R "$ROOT/amplifier-bundle/recipes"/* "$bundle/recipes/"
  cat > "$bundle/tools/autodrive_goal_loop.sh" <<'DRIVER'
#!/usr/bin/env bash
state=""
while [ "$#" -gt 0 ]; do
  [ "${1:-}" = --state-dir ] && state="${2:-}"
  shift
done
printf '%s\n' invoked > "$state/driver-invoked"
exit 0
DRIVER
  chmod 755 "$bundle/tools/autodrive_goal_loop.sh"
  mkdir -p "$c/state/turns" "$c/state/no-worktree"
  identity_worktree="$c/missing"
  [ "$mode" = alias-wrong-upstream ] && identity_worktree="$candidate"
  jq -n --arg b issue-3424 --arg w "$identity_worktree" --arg h "$head" \
    '{schema_version:1,branch:$b,worktree:$w,source:"worktree_setup",base:"main",head:$h,started:"2026-01-01T00:00:00Z"}' > "$c/state/identity.json"
  set +e
  ( cd "$main" && env -u AMPLIHACK_HOME -u CODEX_HOME HOME="$c/home" TMPDIR="$c" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
      AMPLIHACK_BIN=/bin/false RECIPE_VAR_repo_path="$main" \
      bash "$bundle/tools/autodrive_build_run.sh" loop "$c/state" ) > "$c/out" 2> "$c/err"
  rc=$?
  set -e
  printf '%s|%s|%s|%s\n' "$rc" "$c" "$candidate" "$head"
}

IFS='|' read -r ADOPT_RC ADOPT_CASE ADOPT_CANDIDATE ADOPT_HEAD < <(make_build_case adopt clean)
ok listed_clean_worktree_adopted test "$ADOPT_RC" -eq 0
ok adopted_path_persisted test "$(jq -r .worktree "$ADOPT_CASE/state/identity.json")" = "$(cd "$ADOPT_CANDIDATE" && pwd -P)"
ok adopted_loop_started test -f "$ADOPT_CASE/state/driver-invoked"

IFS='|' read -r REFUSE_RC REFUSE_CASE REFUSE_CANDIDATE REFUSE_HEAD < <(make_build_case refuse other-head)
ok different_head_refused test "$REFUSE_RC" -eq 2
ok refusal_before_loop test ! -e "$REFUSE_CASE/state/driver-invoked"

IFS='|' read -r ALIAS_RC ALIAS_CASE ALIAS_CANDIDATE ALIAS_HEAD < <(make_build_case alias alias)
ok alias_worktree_adopted test "$ALIAS_RC" -eq 0
ok alias_path_persisted test "$(jq -r .worktree "$ALIAS_CASE/state/identity.json")" = "$(cd "$ALIAS_CANDIDATE" && pwd -P)"
ok alias_loop_started test -f "$ALIAS_CASE/state/driver-invoked"
ok alias_worktree_check_accepts bash -c '
  cd "$1/main" || exit 1
  source "$1/bundle/tools/autodrive_build_lib.sh"
  build_worktree_check "$2" issue-3424 "$1/main"
' _ "$ALIAS_CASE" "$ALIAS_CANDIDATE"

IFS='|' read -r WRONG_ALIAS_RC WRONG_ALIAS_CASE WRONG_ALIAS_CANDIDATE WRONG_ALIAS_HEAD < <(make_build_case alias-wrong-upstream alias-wrong-upstream)
ok wrong_alias_fixture_created test "$WRONG_ALIAS_RC" -eq 0
ok wrong_alias_reason test "$(
  cd "$WRONG_ALIAS_CASE/main" || exit 1
  set +e
  source "$WRONG_ALIAS_CASE/bundle/tools/autodrive_build_lib.sh"
  build_worktree_check "$WRONG_ALIAS_CANDIDATE" issue-3424 "$WRONG_ALIAS_CASE/main"
  printf '%s' "$BUILD_WT_WHY"
)" = "the recorded worktree does not track origin/issue-3424 or the local branch issue-3424"
ok wrong_alias_worktree_check_refused bash -c '
  cd "$1/main" || exit 1
  source "$1/bundle/tools/autodrive_build_lib.sh"
  ! build_worktree_check "$2" issue-3424 "$1/main"
' _ "$WRONG_ALIAS_CASE" "$WRONG_ALIAS_CANDIDATE"

echo "${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
