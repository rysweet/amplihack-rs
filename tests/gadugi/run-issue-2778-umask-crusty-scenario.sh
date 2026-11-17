#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2778: under umask 0002
# the crusty loop set its own review checkout aside and failed its round.
#
# Step-02-crusty-loop of amplifier-bundle/recipes/autodrive-crusty-loop.yaml
# runs `amplihack internal review-allocate <pr worktree> <state dir>`, then
# starts amplifier-bundle/tools/autodrive_loop.sh with the allocated checkout as
# --review-dir. review-allocate made <state>/review-XXXXXX with the caller's
# umask: 0775 under umask 0002, the default on hosts with user private groups.
# The loop's first act, autodrive_private_dir in autodrive_state.sh, moves every
# group-writable entry of the state folder into untrusted-*, so the checkout it
# had just been handed disappeared and the loop stopped before its first round.
# CI runs at umask 0022, where the folder is 0755, so nothing showed there; this
# script sets umask 0002 itself.
#
# Black boxes, all shipped code, run unchanged:
#   - the real binary: `internal review-allocate` and `review-release`, and the
#     completion helpers `orch helper autodrive-completion-prepare`, `-seal`,
#     `-write` and `-dispatch`, with every other `internal` and `orch helper`
#     command the loop and the round tools call;
#   - amplifier-bundle/tools/autodrive_loop.sh, and the trust scan in
#     autodrive_state.sh (autodrive_private_dir, autodrive_untrusted_entries),
#     which the loop runs first and this script also runs on its own;
#   - the round tools step-00, step-02b and step-06 of autodrive-crusty-round.yaml
#     call: autodrive_worktree_guard.sh, autodrive_crusty_record.sh and
#     autodrive_crusty_posted.sh.
#
# Stand-ins, in $WORK/bin and first on PATH: `amplihack` passes `internal` and `orch helper`
# to the real binary and stands in for `recipe show` and `recipe run`, as SHIM
# in crates/amplihack-cli/tests/support/autodrive_completion_boundary.rs does.
# Its crusty round runs $WORK/round.sh, which follows that file's ROUND_SCRIPT:
# the shipped tools do the round's bash steps, and the round's agents (round
# context, a CLEAN review, the next-action choice `complete`) are played by
# fixed values. `gh` logs its arguments to $WORK/gh-calls and answers `pr view`
# from the fixture; nothing reaches GitHub. `claude`, `copilot` and `codex`
# only record that they were called, which fails the scenario.
#
# Every scenario command runs under `env -i` with an explicit allowlist (see
# scenario_env below): no token, key or setting from the caller's environment
# reaches it. Everything is created under one mktemp folder that the EXIT trap
# removes; nothing is written to the checkout except by the cargo build.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml) runs
# this script directly against the amplihack binary that job installs. CI does
# not run gadugi-test itself; tests/gadugi/scenarios/
# issue-2778-crusty-loop-under-umask-0002.yaml wraps this script for anyone
# running it through gadugi-test by hand.
#
# Run by hand: bash tests/gadugi/run-issue-2778-umask-crusty-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2778_QA_BIN to an
# amplihack binary to skip the build. On success it prints ALL_CASES_PASSED and
# exits 0; otherwise SCENARIO_FAILED and exit 1.
#
# It is bash, not Rust, because every gadugi driver is bash and because it
# drives the shipped shell tools exactly as the recipe step does.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
TOOLS="$REPO_ROOT/amplifier-bundle/tools"
RECIPES="$REPO_ROOT/amplifier-bundle/recipes"

for c in git jq find id; do
  p="$(command -v "$c" 2>/dev/null)"
  case "$p" in
    /*) ;;
    *) echo "FAIL: $c is required"; echo "SCENARIO_FAILED"; exit 1 ;;
  esac
done
for f in "$TOOLS/autodrive_loop.sh" "$TOOLS/autodrive_state.sh" "$TOOLS/autodrive_worktree_guard.sh" \
         "$TOOLS/autodrive_crusty_record.sh" "$TOOLS/autodrive_crusty_posted.sh" \
         "$RECIPES/autodrive-crusty-round.yaml" "$RECIPES/loop-health-evaluator.yaml"; do
  [ -f "$f" ] || { echo "FAIL: missing ${f#"$REPO_ROOT"/}"; echo "SCENARIO_FAILED"; exit 1; }
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2778-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }
BIN=""
SYS_PATH="$(dirname -- "$(command -v git)"):$(dirname -- "$(command -v jq)"):/usr/bin:/bin"

# scenario_env <command...>: runs the command with only the variables below.
#   PATH                      the stand-ins first, then git, jq and the system tools
#   HOME, TMPDIR              scratch folders, so nothing reads or writes the caller's
#   LANG                      a fixed locale
#   GIT_CONFIG_GLOBAL/NOSYSTEM no user or system git config (hooks, signing, templates)
#   GIT_TERMINAL_PROMPT       git never waits on a credential prompt
#   AMPLIHACK_SESSION_DEPTH,
#   AMPLIHACK_MAX_DEPTH       the depth and ceiling a recipe step has (as the Rust Drive sets)
#   AMPLIHACK_BIN             the stand-in amplihack the loop and the round tools call
#   REAL_AMPLIHACK            the binary under test, which the stand-in hands
#                             `internal` and `orch helper` to
#   AMPLIHACK_HOME            the bundle root, as the recipe runner sets it
#   AMPLIHACK_AGENT_BINARY    claude: a name, not a path (the binary accepts only
#                             amplifier, claude, codex or copilot); $WORK/bin/claude
#                             is the stub that records the call
# `internal` returns before any startup work, and `orch helper` is on the
# self-heal skip list and is not a launch, so AMPLIHACK_SKIP_AUTO_INSTALL,
# AMPLIHACK_NO_FRESHNESS_CHECK and AMPLIHACK_NONINTERACTIVE are not needed.
scenario_env() {
  env -i \
    PATH="$WORK/bin:$SYS_PATH" \
    HOME="$WORK/home" \
    TMPDIR="$WORK/tmp" \
    LANG=C \
    GIT_CONFIG_GLOBAL=/dev/null \
    GIT_CONFIG_NOSYSTEM=1 \
    GIT_TERMINAL_PROMPT=0 \
    AMPLIHACK_SESSION_DEPTH=2 \
    AMPLIHACK_MAX_DEPTH=6 \
    AMPLIHACK_BIN="$WORK/bin/amplihack" \
    REAL_AMPLIHACK="$BIN" \
    AMPLIHACK_HOME="$REPO_ROOT" \
    AMPLIHACK_AGENT_BINARY=claude \
    "$@"
}

# shellcheck disable=SC2329 # run by the EXIT trap below
cleanup() {
  if [ -n "${WORK:-}" ] && [ -f "$WORK/review-dir" ] && [ -n "${BIN:-}" ]; then
    scenario_env "$BIN" internal review-release "$(cat "$WORK/review-dir")" >/dev/null 2>&1 || true
  fi
  if [ -n "${WORK:-}" ] && [ -d "$WORK" ]; then
    chmod -R -- u+rwx "$WORK" 2>/dev/null
    rm -rf -- "$WORK"
  fi
}
trap cleanup EXIT

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2778_QA_BIN:-}"
if [ -z "$BIN" ]; then
  # As run-issue-1527-model-scenario.sh does: json-render-diagnostics keeps
  # compiler errors on stderr while stdout carries the artifact records. One
  # awk reads every record and prints the last executable path, so cargo never
  # writes into a closed pipe (issue #1434) and pipefail reports cargo's status.
  # 14 is the length of `"executable":"`; 15 adds the closing quote. The build
  # runs in the caller's environment, before any scenario command.
  if ! BIN="$(cd "$REPO_ROOT" && cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')"; then
    echo "FAIL: cargo build -p amplihack --bin amplihack failed"
    tail -n 40 "$WORK/build.log"
    echo "SCENARIO_FAILED"; exit 1
  fi
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "FAIL: no executable amplihack binary (got '${BIN}')"
  echo "SCENARIO_FAILED"; exit 1
fi
# The scenario runs from the fixture's main checkout, so a relative path given
# in AMPLIHACK_2778_QA_BIN is anchored to the directory the script was run from.
case "$BIN" in
  /*) ;;
  *) BIN="$PWD/$BIN" ;;
esac
echo "binary under test: $BIN"

mkdir -p -- "$WORK/bin" "$WORK/home" "$WORK/tmp" || { echo "FAIL: cannot create the scratch folders"; echo "SCENARIO_FAILED"; exit 1; }

# --- the repository, as Drive::new lays it out ---------------------------------
# A bare origin, a main checkout with one baseline commit, and the pull
# request's linked worktree on its own branch at that commit.
ORIGIN="$WORK/origin.git"; MAIN="$WORK/main"; PR="$WORK/pr"
fixture_git() { scenario_env git -c user.name=QA -c user.email=qa@example.invalid "$@"; }
{ fixture_git init -q --bare "$ORIGIN" \
  && fixture_git init -q -b main "$MAIN" \
  && fixture_git -C "$MAIN" remote add origin "$ORIGIN" \
  && fixture_git -C "$MAIN" commit --allow-empty -qm baseline \
  && fixture_git -C "$MAIN" worktree add -qb quick/demo "$PR" \
  && scenario_env git -C "$PR" rev-parse HEAD > "$WORK/pr-head"; } >/dev/null 2>"$WORK/fixture.err" \
  || { echo "FAIL: could not create the fixture repository: $(tail -n 3 "$WORK/fixture.err" | tr '\n' ' ')"; echo "SCENARIO_FAILED"; exit 1; }

STATE="$WORK/state"
mkdir -m 700 -- "$STATE" || { echo "FAIL: cannot create the state folder"; echo "SCENARIO_FAILED"; exit 1; }
TASK="$WORK/task.txt"
printf 'Fix issue #2778\n' > "$TASK"

# --- stand-ins, written before the umask 0002 subshell ---------------------------
# Each one finds $WORK as the parent of its own folder.
cat > "$WORK/bin/amplihack" <<'STUB'
#!/usr/bin/env bash
# Stand-in amplihack, modelled on SHIM in
# crates/amplihack-cli/tests/support/autodrive_completion_boundary.rs.
set -uo pipefail
W="$(cd -- "$(dirname -- "$0")/.." && pwd -P)" || exit 2
if [ "${1:-}" = internal ]; then exec "$REAL_AMPLIHACK" "$@"; fi
if [ "${1:-} ${2:-}" = 'orch helper' ]; then exec "$REAL_AMPLIHACK" "$@"; fi
if [ "${1:-} ${2:-}" = 'recipe show' ]; then exit 0; fi
if [ "${1:-} ${2:-}" = 'recipe run' ] && [ "$#" -ge 3 ]; then
  name="$(basename -- "$3" .yaml)"
  printf '%s\n' "$name" >> "$W/recipe-calls"
  case "$name" in
    loop-health-evaluator)
      printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: DONE\n'
      exit 0 ;;
    autodrive-crusty-round)
      shift 3
      wd=""
      while [ "$#" -gt 0 ]; do
        case "$1" in
          -c) [ "$#" -ge 2 ] || break
              key="$(printf '%s' "${2%%=*}" | tr '[:lower:]' '[:upper:]')"
              export "$key=${2#*=}"
              shift 2 ;;
          --working-dir) [ "$#" -ge 2 ] || break; wd="$2"; shift 2 ;;
          *) shift ;;
        esac
      done
      exec bash "$W/round.sh" "$W" "$wd" ;;
  esac
fi
printf 'amplihack %s\n' "$*" >> "$W/unexpected.log"
exit 2
STUB

cat > "$WORK/bin/gh" <<'STUB'
#!/usr/bin/env bash
# Stand-in gh: logs its arguments and answers `pr view` from the fixture.
set -uo pipefail
W="$(cd -- "$(dirname -- "$0")/.." && pwd -P)" || exit 1
printf '%s\n' "$*" >> "$W/gh-calls"
case "${1:-} ${2:-}" in
  'pr view')
    filter=""; prev=""
    for a in "$@"; do
      case "$prev" in --jq|-q) filter="$a" ;; esac
      case "$a" in --jq=*) filter="${a#--jq=}" ;; esac
      prev="$a"
    done
    json="$(jq -cn --arg head "$(cat -- "$W/pr-head")" \
      '{number:7,url:"https://github.com/o/r/pull/7",state:"OPEN",isCrossRepository:false,headRefName:"quick/demo",headRefOid:$head,baseRefName:"main"}')" || exit 1
    if [ -n "$filter" ]; then printf '%s\n' "$json" | jq -r "$filter"; else printf '%s\n' "$json"; fi
    exit "$?" ;;
  'pr comment') exit 0 ;;
esac
exit 1
STUB

cat > "$WORK/agent-stub" <<'STUB'
#!/usr/bin/env bash
# No agent may start in this scenario: record the call and fail.
printf '%s %s\n' "$(basename -- "$0")" "$*" >> "$(cd -- "$(dirname -- "$0")/.." && pwd -P)/unexpected.log"
exit 2
STUB

# The crusty round the stand-in amplihack runs: $1 is $WORK, $2 is the round's
# --working-dir, and every `-c key=value` is exported as KEY.
cat > "$WORK/round.sh" <<'STUB'
#!/usr/bin/env bash
set -uo pipefail
W="$1" WD="$2"
if [ -n "${REVIEW_DIR:-}" ] && [ -d "$REVIEW_DIR" ] && [ -n "$WD" ] && [ -d "$WD" ]; then saw=yes; else saw=no; fi
printf '%s\n' "$saw" > "$W/round-saw-checkout"
[ "$saw" = yes ] || { echo "stand-in round: review_dir or --working-dir is not a directory" >&2; exit 1; }
cd -- "$WD" || exit 1
T="/${AUTODRIVE_LAUNCH_TOOLS#/}"
# step-00-review-dir-guard: the shipped guard stamps the review checkout.
idx="$(bash "$T/autodrive_worktree_guard.sh" round-guard)" || exit 1
export RECIPE_VAR_review_index="$idx"
# step-01-round-context (an agent): CONTEXT_SCRIPT's JSON.
head="$(git -C "${REPO_PATH:-.}" rev-parse HEAD)" || exit 1
resolved=""
if [ -n "${AUTODRIVE_RESOLVED_CONCERNS_FILE:-}" ] && [ -f "$AUTODRIVE_RESOLVED_CONCERNS_FILE" ]; then
  resolved="$(tr '\n' ' ' < "$AUTODRIVE_RESOLVED_CONCERNS_FILE")"
fi
ctx="$(jq -cn --arg pr "${PR_NUMBER:-}" --arg head "$head" --arg resolved "$resolved" \
  --arg label "${AUTODRIVE_ROUND_LABEL:-round}" \
  '{pr:$pr,head_sha:$head,resolved_concerns:$resolved,round_label:$label,blockers:[],attempts:[]}')" || exit 1
export RECIPE_VAR_crusty_round_context="$ctx"
# step-02-crusty-review (an agent) reviews nothing here. step-02b: the shipped
# guard checks the review checkout is unchanged.
bash "$T/autodrive_worktree_guard.sh" round-index-check "${REVIEW_DIR:-}" "${RECIPE_VAR_review_index:-}" step-02-crusty-review || exit 1
# step-03-extract-crusty-verdict for a CLEAN review: an empty findings file,
# written under umask 077, and the step's output.
( umask 077 && : > "${AUTODRIVE_ROUND_RECORD:?}.findings" ) || exit 1
export RECIPE_VAR_crusty_verdict='{"crusty_verdict":"CLEAN","verdict_source":"crusty","concern_count":0}'
# step-03c posts nothing (an agent); step-04 and step-05 run only for CONCERNS.
# step-06-write-round-record: the shipped record writer (bind and seal), then
# the shipped posted-verdict recorder, which always exits 0.
bash "$T/autodrive_crusty_record.sh" || exit 1
bash "$T/autodrive_crusty_posted.sh" || :
# step-07-select-next-action (an agent) runs the real writer with `complete`.
amplihack orch helper autodrive-completion-write --private-state "${AUTODRIVE_COMPLETION_STATE:?}" \
  --action complete --reason 'Review objective assessed against preserved evidence' || exit 1
exit 0
STUB

stubs_ok=1
chmod -- 755 "$WORK/bin/amplihack" "$WORK/bin/gh" "$WORK/agent-stub" "$WORK/round.sh" || stubs_ok=0
for a in claude copilot codex; do
  cp -p -- "$WORK/agent-stub" "$WORK/bin/$a" || stubs_ok=0
done
[ "$stubs_ok" = 1 ] || { echo "FAIL: could not write the stand-ins"; echo "SCENARIO_FAILED"; exit 1; }

mode_of() { stat -c %a -- "$1" 2>/dev/null || stat -f %Lp -- "$1" 2>/dev/null; }

# --- the scenario --------------------------------------------------------------
# The sequence of step-02-crusty-loop in
# amplifier-bundle/recipes/autodrive-crusty-loop.yaml, under umask 0002: allocate
# the review generation in the state folder, then run the loop with it as
# --review-dir and step-02's --context list (the budget keys empty, as step-02
# passes them when no budget is set). Right after the allocation, the mode of
# the allocated folder is recorded and the shipped trust scan is run on the
# state folder, because the loop may move that folder.
(
  umask 0002
  cd -- "$MAIN" || exit 11
  REVIEW_DIR="$(scenario_env "$BIN" internal review-allocate "$PR" "$STATE")" || exit 10
  printf '%s' "$REVIEW_DIR" > "$WORK/review-dir"
  ALLOC="$(dirname -- "$REVIEW_DIR")"
  printf '%s' "$ALLOC" > "$WORK/alloc-dir"
  mode_of "$ALLOC" > "$WORK/alloc-mode"
  # shellcheck disable=SC2016 # expanded by the inner bash, with its arguments
  scenario_env bash -c '. "$1/autodrive_state.sh" && autodrive_untrusted_entries "$2" "$(id -u)" -print' \
    trust-scan "$TOOLS" "$STATE" > "$WORK/trust-scan"
  scenario_env bash "$TOOLS/autodrive_loop.sh" --task-file "$TASK" --loop-name crusty \
    --round-recipe "$RECIPES/autodrive-crusty-round.yaml" --clean-token CLEAN --also-clean-token MINOR_ONLY \
    --verdict-field crusty_verdict --repo "$PR" --review-dir "$REVIEW_DIR" --state-dir "$STATE" \
    --context "repo_path=$PR" --context "step_recovery_dir=" --context "pr_number=7" \
    --context "pr_url=https://github.com/o/r/pull/7" --context "no_merge=true" --context "autodrive_dry_run=false" \
    --context "autodrive_budget_loop_rounds=" --context "autodrive_budget_total_rounds=" \
    --context "autodrive_budget_wall_seconds="
) >"$WORK/loop.out" 2>"$WORK/loop.err"; LOOP_RC=$?

fail=0
pass() { echo "PASS: $1 - $2"; }
fl() { echo "FAIL: $1 - $2"; fail=1; }
# shellcheck disable=SC2012 # a listing for the failure detail only
DIAG="rc=$LOOP_RC loop.err: $(tail -n 8 "$WORK/loop.err" 2>/dev/null | tr '\n' ' ') state: $(ls -A -- "$STATE" 2>/dev/null | tr '\n' ' ')"

# 1. review-allocate made the review-* folder in the state folder owner-only.
ALLOC="$(cat "$WORK/alloc-dir" 2>/dev/null)"
MODE="$(cat "$WORK/alloc-mode" 2>/dev/null)"
if [ -n "$ALLOC" ] && [ "$(dirname -- "$ALLOC")" = "$STATE" ] && case "${ALLOC##*/}" in review-*) true ;; *) false ;; esac \
   && [ "$MODE" = 700 ]; then
  pass allocated_review_folder_is_0700 "review-allocate under umask 0002 made ${ALLOC#"$WORK"/} with mode 700"
else
  fl allocated_review_folder_is_0700 "folder '${ALLOC#"$WORK"/}' mode '${MODE:-unknown}', want a review-* folder directly in state with mode 700; $DIAG"
fi

# 2. The trust scan flagged nothing after the allocation, and the loop set
#    nothing aside.
SET_ASIDE="$(find "$STATE" -mindepth 1 -maxdepth 1 -name 'untrusted-*' -print 2>/dev/null)"
if [ -f "$WORK/trust-scan" ] && [ ! -s "$WORK/trust-scan" ] && [ -z "$SET_ASIDE" ]; then
  pass no_entry_set_aside "the trust scan found no untrusted entry after the allocation, and the loop set nothing aside"
else
  SCAN="$(tr '\n' ' ' < "$WORK/trust-scan" 2>/dev/null)"
  ASIDE="$(printf '%s' "$SET_ASIDE" | tr '\n' ' ')"
  fl no_entry_set_aside "trust scan flagged: ${SCAN//"$WORK"\//} set aside: ${ASIDE//"$WORK"\//}; $DIAG"
fi

# 3. The round started with its review checkout and launch folder in place.
SAW="$(cat "$WORK/round-saw-checkout" 2>/dev/null)"
if [ "$SAW" = yes ]; then
  pass round_saw_review_checkout "the crusty round found its review_dir and its working folder"
else
  fl round_saw_review_checkout "the stand-in round reported '${SAW:-no round ran}'; recipes run: $(tr '\n' ' ' < "$WORK/recipe-calls" 2>/dev/null); $DIAG"
fi

# 4. The loop completed the round: exit 0, a DONE result, and the latest
#    record in the state folder holds the CLEAN verdict.
if [ "$LOOP_RC" -eq 0 ] && grep -qF '"loop_result":"DONE"' "$WORK/loop.out" \
   && [ -f "$STATE/crusty-latest.json" ] && jq -e '.crusty_verdict == "CLEAN"' "$STATE/crusty-latest.json" >/dev/null 2>&1; then
  pass loop_completed_the_round "the loop exited 0 with DONE, and crusty-latest.json holds crusty_verdict CLEAN"
else
  fl loop_completed_the_round "out: $(tail -n 2 "$WORK/loop.out" 2>/dev/null | tr '\n' ' '); $DIAG"
fi

# 5. No agent started and the stand-in amplihack saw no command it does not know.
if [ ! -s "$WORK/unexpected.log" ]; then
  pass no_agent_or_unexpected_call "no agent CLI ran and every amplihack call was one the scenario expects"
else
  fl no_agent_or_unexpected_call "unexpected: $(tr '\n' '|' < "$WORK/unexpected.log")"
fi

if [ "$fail" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
