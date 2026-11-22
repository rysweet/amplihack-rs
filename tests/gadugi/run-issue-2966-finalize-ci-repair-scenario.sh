#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2966: a red CI at
# finalisation sends the run back to repair it. It never ends the run.
#
# The #2916 run (PR #2947, 22 November) reached workflow-finalize with the Install
# Smoke Test failing on "claude-strong: 2 launches of claude, want 1". The
# terminal-state probe named that BLOCKED_CI, the validator answered it from the
# probe's record, step recovery handed off, and the run ended exit 81 without
# one attempt to fix an ordinary failing test. The owner's ruling (22 November):
# a red CI means the run iterates until it has fixed the CI problem. Only a
# cause the run cannot fix itself ends it, and then with the cause and the exact
# step a person must take.
#
# What runs, unchanged: the real `amplihack` binary and the pinned recipe runner
# execute the shipped `workflow-finalize` recipe from this checkout's bundle,
# which is default-workflow's finalisation sub-recipe, with the context
# default-workflow gives it: the worktree, the pull request, the base branch and
# a step-recovery directory. Every shipped step runs: the terminal-state probe,
# the cleanup, the quality audit, the CI repair step, the final status, the
# evidence collection, the assessment, the validator, the recovery report and
# the completion step.
#
# What is stood in for, and nothing else:
#   gh      answers `auth status`, `pr view`, `pr list` and
#           `pr checks --watch`, accepts the one write a shipped step makes
#           here (`pr comment`, the "Ready for Final Review" note of
#           step-21-pr-ready, recorded in $CASE/gh-comments.log; every case
#           fails on more than one comment or on any other), and exits 96 on
#           anything else (merge, any api call, secret, variable, workflow,
#           `pr checks` without `--watch`), recording that call in
#           $CASE/gh-unexpected. The pull request is #7,
#           open and not a draft, at the tip the bare remote holds for the run
#           branch. Its checks are read from that tip, not from a flag the
#           driver flips: the `smoke` check fails until the tip holds the file
#           fixed.txt, so a repair counts only once it has been pushed. In the
#           unfixable case the `deploy` check fails whatever the tip holds. In
#           the pending case the `smoke` check is in progress until
#           `gh pr checks --watch` has been run, which returns it settled and
#           green, as the real command returns once nothing is pending.
#   claude  answers the prompt. It is a scripted stand-in for an agent and
#           dispatches on a literal phrase of the prompt, never evaluating any
#           of it. One entry per agent step of the recipe:
#             repair      the CI repair step ("Ensure PR is Mergeable"). It must
#                         be started in the linked worktree. In a case whose
#                         script says `repair` it applies the fix, commits it and
#                         pushes HEAD to the run branch without force; in a case
#                         whose script says `leave` it pushes nothing.
#             assess      the finalisation assessment ("Finalization Assessment").
#                         It reads the pull request through gh. A failing
#                         `smoke` check it repairs itself, as the repair step
#                         would, and reads the pull request again; a failing
#                         `deploy` check names a missing secret, which it cannot
#                         fix, so it reports BLOCKED_CI with the check, the cause
#                         and the owner's step. Otherwise, with every check green,
#                         it reports EXISTING_OPEN_PR for #7. A check still in
#                         progress it waits out with `gh pr checks --watch`
#                         first, and reads the pull request again.
#             recover     the step-recovery agent ("Recover a failed workflow
#                         step"). It hands off, to the default owner, with the
#                         step the failure line asked for.
#             the rest    the cleanup, the quality audit, the narrative and the
#                         adjudicator each get one line of prose.
#           A prompt matching none of these is recorded as `unmatched`, and every
#           case fails on one, so a shipped prompt reworded out from under its
#           key fails the scenario by name instead of changing what it runs.
#           Every call is recorded in $CASE/agent-calls.log (role and working
#           directory); every push in $CASE/push.log.
#
# Cases:
#   red_ci_is_repaired                  a head whose `smoke` check fails. The
#                                       repair step fixes and pushes it. The run
#                                       ends EXISTING_OPEN_PR with terminal_success
#                                       true, exit 0, and no hand-off on record.
#   assessment_finishes_the_repair      the same head, but the repair step ends
#                                       without a push. The assessment repairs it
#                                       and reports repaired_ci. The run ends
#                                       EXISTING_OPEN_PR, exit 0, and no
#                                       hand-off on record.
#   unfixable_cause_is_handed_to_its_owner
#                                       the `deploy` check fails on a secret the
#                                       repository does not define. The repair
#                                       step and the assessment each run once, no
#                                       push is made, and the run ends exit 81
#                                       with BLOCKED_CI, the check, the cause and
#                                       the human step in the hand-off record.
#   pending_checks_are_waited_on        the `smoke` check is still in progress.
#                                       The repair step pushes nothing; the
#                                       assessment waits with
#                                       `gh pr checks --watch`, reads the settled
#                                       green head and reports EXISTING_OPEN_PR.
#                                       Exit 0, no push, no hand-off.
#
# The baseline (opt in): AMPLIHACK_2966_BASELINE_REF=origin/main extracts that
# ref's amplifier-bundle with `git archive` into a separate temporary folder and
# runs the first case against it. It is the reproduction of the report: the run
# ends BLOCKED_CI with exit 81 and the repair step is never started. The fixed
# scenario prints BASELINE_CONFIRMED for it.
#
# What this scenario does NOT cover: an agent's judgement. Whether a real agent
# finds the right fix, or tells a missing secret from a flaky check, is the live
# run's to show, and the PR's own CI and review are where it shows. What is under
# test here is the wiring the owner's ruling needs: a red check reaches the
# repair step instead of ending the run, the assessment may finish the repair and
# decides on what it reads afterwards, and an unfixable cause ends the run with
# its owner's step in the record.
#
# Isolation: umask 077, a 0700 root under $TMPDIR, and `env -i` carrying only the
# variables named in scenario_env: no token, GH_* setting, SSH_AUTH_SOCK,
# GIT_ASKPASS or inherited CLAUDE_CONFIG_DIR reaches it. Each case has its own
# HOME, its own GIT_CONFIG_GLOBAL and its own folders; GIT_CONFIG_NOSYSTEM=1 and
# GIT_ALLOW_PROTOCOL=file keep Git to the local bare remote. The PATH holds
# links to an explicit list of host tools and the stand-ins, nothing else. The
# real ~/.claude and ~/.amplihack are never read or written. DEPLOY_TOKEN is a
# name in the fixtures, never a value.
#
# A missing binary, runner or host tool is a failure with evidence, not a skip.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml) runs this
# script directly against the binary it installs.
# tests/gadugi/scenarios/issue-2966-finalize-ci-repair.yaml wraps it for
# gadugi-test.
#
# Run by hand: bash tests/gadugi/run-issue-2966-finalize-ci-repair-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2966_QA_BIN to an amplihack
# binary to skip the build; its path is printed and it must be an executable
# file. recipe-runner-rs comes from RECIPE_RUNNER_RS_PATH or the PATH. Set
# QA_KEEP_WORK=1 to keep the temporary root for a post-mortem. On success it
# prints ALL_CASES_PASSED and exits 0; otherwise SCENARIO_FAILED and exit 1, with
# each case's evidence under its own header.
# No `-e` on purpose: every case must run and report, so an early exit would hide
# the failures after the first one. Setup is guarded with `|| die`.
set -uo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
HOST_TMP="${TMPDIR:-/tmp}"
BRANCH="feat/issue-2966-qa"
PR_URL="https://github.com/owner/repo/pull/7"
DEPLOY_CAUSE="deploy: the workflow reads the secret DEPLOY_TOKEN, which the repository does not define"
DEPLOY_STEP="repository administrator: add the DEPLOY_TOKEN secret under Settings, Secrets and variables, Actions, then re-run the failed deploy check"

# --- the temporary root and the failure dump ---------------------------------
WORK="$(mktemp -d "${HOST_TMP%/}/issue-2966-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }
readonly WORK
chmod 700 "$WORK"

EVIDENCE_FILES=(setup.log run.out run.err agent-calls.log push.log gh.log gh-unexpected
  recovery-report.out)

# dump_case <dir>: every evidence file of one case, each under its own header.
dump_case() {
  local dir="$1" f
  for f in "${EVIDENCE_FILES[@]}"; do
    if [ -f "$dir/$f" ]; then
      echo "===== $(basename "$dir")/$f ====="
      cat -- "$dir/$f"
    fi
  done
  if [ -d "$dir/recovery" ]; then
    for f in "$dir"/recovery/*.jsonl; do
      [ -f "$f" ] || continue
      echo "===== $(basename "$dir")/recovery/$(basename "$f") ====="
      cat -- "$f"
    done
  fi
}

on_exit() {
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    local d
    for d in "$WORK"/case-*; do
      [ -d "$d" ] && dump_case "$d"
    done
    echo "SCENARIO_FAILED"
  fi
  # Only the folder this script made: non-empty, not the root, not the invoking
  # user's home, and under the temporary folder it was made in.
  case "$WORK" in
    ""|/) ;;
    "${HOME:-/nonexistent}"|"${HOME:-/nonexistent}"/) ;;
    "${HOST_TMP%/}"/*)
      if [ -n "${QA_KEEP_WORK:-}" ]; then echo "KEPT: $WORK"; else rm -rf -- "$WORK"; fi
      ;;
  esac
  exit "$rc"
}
trap on_exit EXIT

die() {
  echo "FAIL: $*"
  exit 1
}

# --- A: preconditions --------------------------------------------------------
BIN="${AMPLIHACK_2966_QA_BIN:-}"
if [ -z "$BIN" ]; then
  BIN="$(cd "$REPO_ROOT" && cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/cargo-build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')" \
    || { tail -n 40 "$WORK/cargo-build.log"; die "cargo build -p amplihack --bin amplihack failed"; }
fi
case "$BIN" in /*) ;; *) BIN="$PWD/$BIN" ;; esac
[ -f "$BIN" ] && [ -x "$BIN" ] || die "no executable amplihack binary (got '$BIN'); set AMPLIHACK_2966_QA_BIN"
echo "binary under test: $BIN"

RUNNER="${RECIPE_RUNNER_RS_PATH:-}"
[ -n "$RUNNER" ] || RUNNER="$(command -v recipe-runner-rs 2>/dev/null || true)"
[ -n "$RUNNER" ] || die "recipe-runner-rs is required (set RECIPE_RUNNER_RS_PATH or put it on PATH)"
case "$RUNNER" in /*) ;; *) RUNNER="$PWD/$RUNNER" ;; esac
[ -f "$RUNNER" ] && [ -x "$RUNNER" ] || die "recipe-runner-rs is not an executable file: $RUNNER"

# The bundle under test: this checkout's, or, for the baseline, the one a ref
# held. The recipe and every helper it calls come from the same bundle.
BUNDLE_HOME="$REPO_ROOT"
BASELINE_REF="${AMPLIHACK_2966_BASELINE_REF:-}"
if [ -n "$BASELINE_REF" ]; then
  # The ref reaches git as an argument, so a value shaped like an option is
  # refused, and the ref is resolved to one commit before anything reads it.
  case "$BASELINE_REF" in
    -*) die "AMPLIHACK_2966_BASELINE_REF must name a commit, not an option: $BASELINE_REF" ;;
  esac
  BASELINE_SHA="$(cd "$REPO_ROOT" && git rev-parse --verify --quiet "${BASELINE_REF}^{commit}")" \
    || die "AMPLIHACK_2966_BASELINE_REF does not name a commit: $BASELINE_REF"
  BUNDLE_HOME="$WORK/baseline-bundle"
  mkdir -p -- "$BUNDLE_HOME" || die "cannot create the baseline folder"
  ( cd "$REPO_ROOT" && git archive --format=tar "$BASELINE_SHA" amplifier-bundle ) \
    | tar -x -C "$BUNDLE_HOME" || die "cannot extract amplifier-bundle from $BASELINE_REF ($BASELINE_SHA)"
  echo "baseline bundle: $BASELINE_REF ($BASELINE_SHA) extracted to $BUNDLE_HOME"
fi
RECIPE="$BUNDLE_HOME/amplifier-bundle/recipes/workflow-finalize.yaml"
[ -f "$RECIPE" ] || die "missing $RECIPE"

# --- B: the tools a step may call --------------------------------------------
BINDIR="$WORK/bin"
mkdir -p -- "$BINDIR" && chmod 700 "$BINDIR" || die "cannot create $BINDIR"
REQUIRED_TOOLS=(bash sh env git jq cat grep sed awk sort tr head tail cut dirname
  basename mkdir rm cp mv ln chmod date stat mktemp uniq wc find xargs tee
  sha256sum touch readlink realpath)
OPTIONAL_TOOLS=(rmdir id uname hostname nproc diff cmp tar gzip egrep fgrep
  sleep timeout tty od base64 printf true false test expr seq)
for t in "${REQUIRED_TOOLS[@]}"; do
  p="$(command -v "$t" 2>/dev/null || true)"
  case "$p" in
    /*) ln -sf -- "$p" "$BINDIR/$t" ;;
    *) die "host tool missing: $t" ;;
  esac
done
for t in "${OPTIONAL_TOOLS[@]}"; do
  p="$(command -v "$t" 2>/dev/null || true)"
  case "$p" in /*) [ -e "$BINDIR/$t" ] || ln -sf -- "$p" "$BINDIR/$t" ;; esac
done
ln -sf -- "$BIN" "$BINDIR/amplihack"
ln -sf -- "$RUNNER" "$BINDIR/recipe-runner-rs"

# --- C: the stand-ins --------------------------------------------------------
cat > "$BINDIR/gh" <<'GH_STUB'
#!/bin/bash
set -u
dir="${QA_CASE_DIR:?}"
printf '%s\n' "$*" >> "$dir/gh.log"

# Apply a --jq/-q program (data for jq, never for a shell) to a JSON document.
emit() {
  local json="$1" prog="" prev="" a
  shift
  for a in "$@"; do
    case "$prev" in --jq|-q) prog="$a" ;; esac
    prev="$a"
  done
  if [ -n "$prog" ]; then printf '%s' "$json" | jq -r "$prog"; else printf '%s\n' "$json"; fi
}

# The checks the head holds. The head is the tip the bare remote carries for the
# run branch, so a check can turn green only by a push.
rollup() {
  local tip="$1"
  local mode
  mode="$(cat "$dir/mode" 2>/dev/null)"
  if [ "$mode" = unfixable ]; then
    printf '%s' '[{"__typename":"CheckRun","name":"ci","status":"COMPLETED","conclusion":"SUCCESS"},{"__typename":"CheckRun","name":"deploy","status":"COMPLETED","conclusion":"FAILURE"}]'
  elif [ "$mode" = pending ] && [ ! -e "$dir/checks-settled" ]; then
    printf '%s' '[{"__typename":"CheckRun","name":"smoke","status":"IN_PROGRESS","conclusion":null}]'
  elif [ "$mode" = pending ] || git --git-dir="$dir/origin.git" cat-file -e "$tip:fixed.txt" 2>/dev/null; then
    printf '%s' '[{"__typename":"CheckRun","name":"smoke","status":"COMPLETED","conclusion":"SUCCESS"}]'
  else
    printf '%s' '[{"__typename":"CheckRun","name":"smoke","status":"COMPLETED","conclusion":"FAILURE"}]'
  fi
}

pr_json() {
  local tip
  tip="$(git --git-dir="$dir/origin.git" rev-parse "refs/heads/$QA_BRANCH" 2>/dev/null || true)"
  jq -nc --arg b "$QA_BRANCH" --arg o "$tip" --argjson r "$(rollup "$tip")" \
    '{number:7,url:"https://github.com/owner/repo/pull/7",state:"OPEN",
      title:"fix: repair the smoke check (#2966)",body:"Closes #2966",
      createdAt:"2026-11-22T00:00:00Z",
      headRefName:$b,headRefOid:$o,baseRefName:"main",isCrossRepository:false,
      isDraft:false,mergeable:"MERGEABLE",mergeStateStatus:"CLEAN",reviewDecision:"",
      statusCheckRollup:$r,mergedAt:null,mergeCommit:null,
      headRepositoryOwner:{login:"owner"},headRepository:{name:"repo"},
      author:{login:"owner"},labels:[],comments:[],reviews:[]}'
}

case "${1-} ${2-}" in
  "auth status") echo "Logged in to github.com as owner"; exit 0 ;;
  "pr view") emit "$(pr_json)" "$@"; exit 0 ;;
  "pr list") emit "[$(pr_json)]" "$@"; exit 0 ;;
  # The one write a shipped step makes here: step-21-pr-ready's "Ready for Final
  # Review" comment. It is accepted and recorded; nothing else is.
  "pr comment") note="$*"; printf '%s\n' "${note%%$'\n'*}" >> "$dir/gh-comments.log"; exit 0 ;;
  # Waiting on the checks: it returns once nothing is pending, so here it
  # settles the pending check. Without --watch it is not in the table.
  "pr checks")
    case " $* " in
      *" --watch "*) : > "$dir/checks-settled"; echo "All checks were successful"; exit 0 ;;
    esac ;;
esac
printf '%s\n' "$*" >> "$dir/gh-unexpected"
echo "gh stub: unexpected call: $*" >&2
exit 96
GH_STUB

cat > "$BINDIR/claude" <<'CLAUDE_STUB'
#!/bin/bash
set -u
dir="${QA_CASE_DIR:?}"
# The version the product gates on: below 2.1.319 it refuses to launch.
if [ "${1-}" = "--version" ]; then echo "2.1.319 (Claude Code)"; exit 0; fi
input="$(cat)"

# Dispatch on a literal phrase of the prompt. The prompt is only ever matched,
# never evaluated.
case "$input" in
  *"Recover a failed workflow step"*) role=recover ;;
  *"Ensure PR is Mergeable"*) role=repair ;;
  *"Finalization Assessment"*) role=assess ;;
  *"Terminal-State Adjudication"*) role=adjudicate ;;
  *"Final Cleanup and Verification"*) role=cleanup ;;
  *"Quality Audit Loop"*) role=audit ;;
  *"Agentic Finalization Narrative"*) role=narrative ;;
  *) role=unmatched ;;
esac
cwd="$(pwd -P)"
printf '%s\t%s\n' "$role" "$cwd" >> "$dir/agent-calls.log"
# The prompt itself, for a post-mortem (QA_KEEP_WORK=1); nothing reads it back.
mkdir -p "$dir/prompts"
printf '%s\n' "$input" > "$dir/prompts/$(wc -l < "$dir/agent-calls.log" | tr -d ' ')-$role.txt"

# The head and checks, as the run's own gh reports them.
view() { gh pr view 7 --repo owner/repo --json headRefOid,statusCheckRollup; }

# fix_and_push <who>: the repair a competent agent makes for the failing `smoke`
# check, in the directory it was started in, pushed to the run branch without
# force. The directory must be the linked worktree.
fix_and_push() {
  local who="$1" top gd cd_
  top="$(git rev-parse --show-toplevel 2>/dev/null)" || top=""
  gd="$(git rev-parse --absolute-git-dir 2>/dev/null)" || gd=""
  cd_="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" || cd_=""
  if [ "$top" != "$QA_WORKTREE" ] || [ -z "$gd" ] || [ "$gd" = "$cd_" ]; then
    printf '%s started in %s, which is not the linked worktree %s\n' "$who" "$(pwd -P)" "$QA_WORKTREE" >> "$dir/stub-errors"
    echo "the repair must run in the linked worktree" >&2
    exit 1
  fi
  printf 'fixed\n' > fixed.txt
  git add fixed.txt
  git commit -q -m "fix: make the smoke check pass" || { echo "commit failed" >&2; exit 1; }
  git push -q origin "HEAD:refs/heads/$QA_BRANCH" >> "$dir/push.err" 2>&1 \
    || { echo "push failed" >&2; exit 1; }
  printf '%s %s\n' "$who" "$(git rev-parse HEAD)" >> "$dir/push.log"
}

case "$role" in
  recover)
    ev="$(printf '%s' "$input" | awk -v re='/[^[:space:]`"]*evidence-[^[:space:]`"]*\\.json' \
      'n == 0 && match($0, re) { print substr($0, RSTART, RLENGTH); n = 1 }')"
    excerpt=""; owner="run-requester"
    if [ -n "$ev" ] && [ -f "$ev" ]; then
      excerpt="$(jq -r '.failure.output_excerpt // ""' "$ev" \
        | awk 'index($0, "failed closed") { last = $0 } END { if (last != "") print last }')"
      owner="$(jq -r '.default_owner // "run-requester"' "$ev")"
    fi
    needs="${excerpt#*next action: }"
    [ "$needs" != "$excerpt" ] || needs="read the failure line in the recovery record"
    jq -nc --arg o "$owner" --arg n "$needs" \
      '{step_recovery:{choice:"handoff",reason:"the step reports a blocker the run cannot repair",owner:$o,needs:$n}}'
    ;;
  repair)
    if [ "$(cat "$dir/script-repair" 2>/dev/null)" = repair ]; then
      fix_and_push repair
      echo "The smoke check was failing. I fixed it, pushed it, and the checks are green."
    else
      echo "I stopped before a fix was ready. Nothing was pushed."
    fi
    ;;
  assess)
    # The assessment is started where the run was, and finds the worktree in the
    # evidence it was given, as an agent would: the fenced `evidence` block of
    # its prompt, as data.
    wt="$(printf '%s\n' "$input" | awk '/^```evidence/ { f = 1; next } /^```/ { f = 0 } f' | jq -r '.git.worktree_path // empty' 2>/dev/null)"
    [ -z "$wt" ] || cd "$wt" || { echo "cannot enter $wt" >&2; exit 1; }
    pushed=none
    facts="$(view)"
    if printf '%s' "$facts" | jq -e '[.statusCheckRollup[] | select(.status != "COMPLETED")] | length > 0' >/dev/null; then
      gh pr checks 7 --repo owner/repo --watch >/dev/null || { echo "gh pr checks --watch failed" >&2; exit 1; }
      facts="$(view)"
    fi
    if printf '%s' "$facts" | jq -e '[.statusCheckRollup[] | select(.name == "smoke" and .conclusion == "FAILURE")] | length > 0' >/dev/null; then
      fix_and_push assess
      pushed=repaired_ci
      facts="$(view)"
    fi
    head="$(printf '%s' "$facts" | jq -r '.headRefOid')"
    counts="$(printf '%s' "$facts" | jq -r '[.statusCheckRollup[] | .conclusion] | "success=\(map(select(. == "SUCCESS")) | length) failing=\(map(select(. == "FAILURE")) | length) pending=0 unknown=0"')"
    if printf '%s' "$facts" | jq -e '[.statusCheckRollup[] | select(.name == "deploy" and .conclusion == "FAILURE")] | length > 0' >/dev/null; then
      jq -nc --arg r "$QA_DEPLOY_CAUSE" --arg n "$QA_DEPLOY_STEP" \
        '{terminal_state:"BLOCKED_CI",action_taken:"none",pr_number:"7",reason:$r,next_action:$n}'
    else
      jq -nc --arg a "$pushed" --arg r "head $head, $counts, read after the last write" \
        '{terminal_state:"EXISTING_OPEN_PR",action_taken:$a,pr_number:"7",reason:$r,next_action:"See pull request #7"}'
    fi
    ;;
  adjudicate) echo "The artifacts do not settle it."; echo "TERMINAL_VERDICT: UNCERTAIN" ;;
  cleanup) echo "Cleanup reviewed. Nothing to remove." ;;
  audit) echo "QUALITY AUDIT: CLEAN" ;;
  narrative) echo "The run published its work and the pull request is open. CI was repaired or handed to its owner." ;;
  *) echo "unmatched prompt" ;;
esac
CLAUDE_STUB
chmod 700 "$BINDIR/gh" "$BINDIR/claude"

# scenario_env <case dir> <command...>: one wrapper for everything that runs the
# product or Git. HOME, the Git configuration and every folder are the case's own.
scenario_env() {
  local dir="$1"; shift
  env -i \
    HOME="$dir/home" \
    PATH="$BINDIR" \
    TERM=dumb LANG=C \
    TMPDIR="$dir/tmp" \
    XDG_CONFIG_HOME="$dir/home/.config" \
    CLAUDE_CONFIG_DIR="$dir/home/.claude" \
    CODEX_HOME="$dir/home/.codex" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_GLOBAL="$dir/gitconfig" \
    GIT_TERMINAL_PROMPT=0 GIT_ALLOW_PROTOCOL=file GIT_ASKPASS=true SSH_ASKPASS=true \
    GIT_AUTHOR_NAME=qa GIT_AUTHOR_EMAIL=qa@example.invalid \
    GIT_COMMITTER_NAME=qa GIT_COMMITTER_EMAIL=qa@example.invalid \
    AMPLIHACK_HOME="$BUNDLE_HOME" \
    AMPLIHACK_WORK_ROOT="$dir/work" AMPLIHACK_WORK_ROOT_EXPLICIT=1 \
    AMPLIHACK_CLAUDE_PROFILE_ROOT="$dir/home/profiles" \
    AMPLIHACK_LAUNCHER_BINARY="$BIN" AMPLIHACK_AGENT_BINARY=claude \
    AMPLIHACK_CLAUDE_BINARY_PATH="$BINDIR/claude" \
    AMPLIHACK_NONINTERACTIVE=1 AMPLIHACK_GOVERNOR=off AMPLIHACK_ROTATE_AUTOSTART=0 \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 AMPLIHACK_NO_UPDATE_CHECK=1 AMPLIHACK_NO_FRESHNESS_CHECK=1 \
    AMPLIHACK_SKIP_POWER_STEERING=1 AMPLIHACK_SKIP_REFLECTION=1 AMPLIHACK_CLEANUP_ON_EXIT=0 \
    AMPLIHACK_USAGE_LIMIT_WAIT=0 \
    RECIPE_RUNNER_RS_PATH="$RUNNER" RECIPE_RUNNER_NO_UPDATE_CHECK=1 \
    REMOTE_HOST_TYPE=github ISSUE_NUMBER=2966 \
    QA_CASE_DIR="$dir" QA_BRANCH="$BRANCH" QA_WORKTREE="$dir/worktree" \
    QA_DEPLOY_CAUSE="$DEPLOY_CAUSE" QA_DEPLOY_STEP="$DEPLOY_STEP" \
    "$@" </dev/null
}

# qa_git <case dir> <git args...>: one setup Git command, its output in setup.log.
qa_git() {
  local dir="$1"; shift
  scenario_env "$dir" git "$@" >>"$dir/setup.log" 2>&1
}

# new_case <id> <mode> <repair script>: a private folder with a bare remote whose
# HEAD is main, a clone whose origin is a GitHub-shaped URL that Git rewrites to
# the bare remote, and a linked worktree on the run branch holding one committed,
# pushed change. <mode> is `fixable`, `unfixable` or `pending`; the repair
# script is what the stand-in CI repair step does, `repair` or `leave`. Prints
# the case folder.
new_case() {
  local id="$1" mode="$2" script="$3" dir
  dir="$WORK/case-$id"
  mkdir -p "$dir/home/.config" "$dir/home/profiles" "$dir/tmp" "$dir/work" "$dir/launch" || return 1
  : > "$dir/gitconfig"
  : > "$dir/setup.log"
  printf '%s' "$mode" > "$dir/mode"
  printf '%s' "$script" > "$dir/script-repair"
  qa_git "$dir" init -q --bare -b main "$dir/origin.git" || return 1
  qa_git "$dir" init -q -b main "$dir/seed" || return 1
  printf 'issue 2966 QA\n' > "$dir/seed/README.md"
  qa_git "$dir" -C "$dir/seed" add README.md || return 1
  qa_git "$dir" -C "$dir/seed" commit -q -m base || return 1
  qa_git "$dir" -C "$dir/seed" push -q "$dir/origin.git" HEAD:refs/heads/main || return 1
  qa_git "$dir" clone -q "$dir/origin.git" "$dir/clone" || return 1
  qa_git "$dir" -C "$dir/clone" remote set-url origin https://github.com/owner/repo.git || return 1
  qa_git "$dir" -C "$dir/clone" config "url.file://$dir/origin.git.insteadOf" https://github.com/owner/repo.git || return 1
  qa_git "$dir" -C "$dir/clone" config core.hooksPath /dev/null || return 1
  qa_git "$dir" -C "$dir/clone" config commit.gpgsign false || return 1
  qa_git "$dir" -C "$dir/clone" worktree add -q -b "$BRANCH" "$dir/worktree" origin/main || return 1
  printf 'the run work\n' > "$dir/worktree/feature.txt"
  qa_git "$dir" -C "$dir/worktree" add feature.txt || return 1
  qa_git "$dir" -C "$dir/worktree" commit -q -m "feat: the run work" || return 1
  qa_git "$dir" -C "$dir/worktree" push -q -u origin "$BRANCH" || return 1
  # The redirect is the fixture's own premise: the GitHub-shaped origin must
  # reach the bare remote and nothing else, so read it back through Git.
  local url remote_tip local_tip
  url="$(scenario_env "$dir" git -C "$dir/clone" config --get remote.origin.url)" || return 1
  [ "$url" = "https://github.com/owner/repo.git" ] || { echo "origin url is $url" >> "$dir/setup.log"; return 1; }
  remote_tip="$(scenario_env "$dir" git -C "$dir/worktree" ls-remote origin "refs/heads/$BRANCH" | cut -f1)" || return 1
  local_tip="$(scenario_env "$dir" git -C "$dir/worktree" rev-parse HEAD)" || return 1
  [ -n "$remote_tip" ] && [ "$remote_tip" = "$local_tip" ] \
    || { echo "ls-remote through the redirect gave '$remote_tip', want '$local_tip'" >> "$dir/setup.log"; return 1; }
  printf '%s' "$local_tip" > "$dir/initial-tip"
  if scenario_env "$dir" git -C "$dir/launch" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "the launch folder $dir/launch is inside a Git checkout" >> "$dir/setup.log"
    return 1
  fi
  # The step-recovery directory, owned by the caller so the run's own report
  # step leaves its records for this driver to read.
  mkdir -p "$dir/recovery" && chmod 700 "$dir/recovery" || return 1
  printf '%s\n' "$dir"
}

# run_case <case dir>: the shipped workflow-finalize through amplihack and the
# pinned runner, launched from the case's non-Git folder, with the context
# default-workflow gives it. Records the exit code in $dir/run.exit.
run_case() {
  local dir="$1" setup published rc
  setup="$(jq -nc --arg p "$dir/worktree" --arg b "$BRANCH" '{worktree_path: $p, branch_name: $b}')" || return 1
  published="$(jq -nc --arg u "$PR_URL" '{pr_url: $u, pr_number: "7"}')" || return 1
  ( cd "$dir/launch" && scenario_env "$dir" "$BIN" recipe run "$RECIPE" -w "$dir/launch" --no-detach -f json \
      -c "repo_path=$dir/clone" -c "worktree_setup=$setup" -c "branch_name=$BRANCH" \
      -c "base_branch=main" -c "pr_publish_result=$published" -c "pr_url=$PR_URL" -c "pr_number=7" \
      -c "issue_number=2966" -c "task_description=Repair the failing smoke check" \
      -c "final_requirements=Make the smoke check pass" -c "pr_feedback_changes=none" \
      -c "philosophy_check=ok" -c "patterns_check=ok" -c "no_merge=true" \
      -c "step_recovery_dir=$dir/recovery" ) >"$dir/run.out" 2>"$dir/run.err"
  rc=$?
  printf '%s' "$rc" > "$dir/run.exit"
  return 0
}

# --- D: assertions -----------------------------------------------------------
problems=()
passed=0
fail=0

calls_of() { grep -c "^$2	" "$1/agent-calls.log" 2>/dev/null || true; }

# check_common <dir>: what every case must show, whatever it decided.
check_common() {
  local dir="$1" n
  [ -s "$dir/gh.log" ] || problems+=("the gh stand-in recorded no call")
  [ -s "$dir/agent-calls.log" ] || problems+=("the claude stand-in recorded no call")
  [ ! -e "$dir/gh-unexpected" ] || problems+=("unexpected gh call: $(head -n 1 "$dir/gh-unexpected")")
  [ ! -e "$dir/stub-errors" ] || problems+=("stand-in error: $(head -n 1 "$dir/stub-errors")")
  n="$(calls_of "$dir" unmatched)"
  [ "${n:-0}" -eq 0 ] || problems+=("$n agent prompt(s) matched no stand-in key")
  # The one write a shipped step may make: step-21's "Ready for Final Review"
  # note, at most once. Any other comment is a write the case did not allow.
  if [ -e "$dir/gh-comments.log" ]; then
    n="$(wc -l < "$dir/gh-comments.log" | tr -d ' ')"
    [ "$n" -le 1 ] || problems+=("$n pull request comments were written, want at most one")
    if grep -qv 'Ready for Final Review' "$dir/gh-comments.log"; then
      problems+=("a pull request comment other than the Ready for Final Review note: $(awk 'n == 0 && index($0, "Ready for Final Review") == 0 { print; n = 1 }' "$dir/gh-comments.log")")
    fi
  fi
}

# expect_exit <dir> <want>
expect_exit() {
  local got
  got="$(cat "$1/run.exit" 2>/dev/null)" || got="<missing>"
  [ "$got" = "$2" ] || problems+=("recipe run exit '$got', want '$2'")
}

# expect_calls <dir> <role> <want>
expect_calls() {
  local got
  got="$(calls_of "$1" "$2")"
  [ "${got:-0}" = "$3" ] || problems+=("$2 stand-in called ${got:-0} time(s), want $3")
}

# remote_tip <dir>: the tip the bare remote holds for the run branch.
remote_tip() { git --git-dir="$1/origin.git" rev-parse "refs/heads/$BRANCH" 2>/dev/null; }

# result_field <dir> <key>: one field of the result validate-agentic-finalization
# reported, which is the run's terminal state, or <missing>. The runner prints
# each step's output as text, so the object is read back from that text.
result_field() {
  jq -rs --arg k "$2" \
    '[.[] | (.step_results // [])[] | select(.step_id == "validate-agentic-finalization")
       | .output | (if type == "string" then (fromjson? // {}) else . end) | select(type == "object" and has($k)) | .[$k]]
     | if length == 0 then "<missing>" else (last | tostring) end' \
    "$1/run.out" 2>/dev/null || echo "<missing>"
}

# expect_result <dir> <key> <want>
expect_result() {
  local got
  got="$(result_field "$1" "$2")"
  [ "$got" = "$3" ] || problems+=("result $2 '$got', want '$3'")
}

# expect_no_handoff <dir>: the recovery directory holds no hand-off, which is
# what the shipped report step ends a run on.
expect_no_handoff() {
  local dir="$1" rc
  scenario_env "$dir" "$BIN" orch helper step-recovery report --dir "$dir/recovery" \
    >"$dir/recovery-report.out" 2>&1
  rc=$?
  [ "$rc" -eq 0 ] || problems+=("the recovery report exits $rc: a hand-off is on record")
}

verdict() {
  local name="$1" dir="$2" summary="$3"
  if [ "${#problems[@]}" -eq 0 ]; then
    echo "PASS: $name - $summary"
    passed=$((passed + 1))
  else
    echo "FAIL: $name: ${problems[*]}"
    fail=1
    dump_case "$dir"
  fi
}

# --- E: the baseline (opt in) ------------------------------------------------
if [ -n "$BASELINE_REF" ]; then
  if dir="$(new_case baseline fixable repair)"; then
    run_case "$dir"
    problems=()
    check_common "$dir"
    expect_exit "$dir" 81
    expect_calls "$dir" repair 0
    # The hand-off record carries the state the probe named.
    if ! grep -q 'BLOCKED_CI' "$dir"/recovery/*.jsonl 2>/dev/null; then
      problems+=("no BLOCKED_CI in the hand-off record")
    fi
    # No push check here: the stand-in assessment repairs whatever the bundle's
    # own prompt allows, and the old validator answers from the probe's record
    # before it reads the assessment, so a push cannot change this ending.
    if [ "${#problems[@]}" -eq 0 ]; then
      echo "BASELINE_CONFIRMED: $BASELINE_REF ends BLOCKED_CI, exit 81, and never starts the repair step"
      exit 0
    fi
    echo "BASELINE_NOT_REPRODUCED: ${problems[*]}"
    dump_case "$dir"
    exit 1
  fi
  dump_case "$WORK/case-baseline"
  die "cannot set up the baseline case"
fi

# --- F: case 1, a red CI the repair step fixes -------------------------------
if dir="$(new_case red_ci_is_repaired fixable repair)"; then
  run_case "$dir"
  problems=()
  check_common "$dir"
  expect_exit "$dir" 0
  expect_calls "$dir" repair 1
  expect_calls "$dir" recover 0
  [ -s "$dir/push.log" ] || problems+=("no repair push was made")
  tip="$(remote_tip "$dir")"
  [ -n "$tip" ] && [ "$tip" != "$(cat "$dir/initial-tip")" ] || problems+=("the remote tip did not move")
  git --git-dir="$dir/origin.git" cat-file -e "$tip:fixed.txt" 2>/dev/null || problems+=("the pushed tip does not hold the fix")
  expect_result "$dir" terminal_state EXISTING_OPEN_PR
  expect_result "$dir" terminal_success true
  expect_no_handoff "$dir"
  verdict red_ci_is_repaired "$dir" "the repair step pushed the fix; EXISTING_OPEN_PR, exit 0, no hand-off"
else
  dump_case "$WORK/case-red_ci_is_repaired"
  die "red_ci_is_repaired: cannot set up the case"
fi

# --- G: case 2, the assessment finishes the repair ---------------------------
if dir="$(new_case assessment_finishes_the_repair fixable leave)"; then
  run_case "$dir"
  problems=()
  check_common "$dir"
  expect_exit "$dir" 0
  expect_calls "$dir" repair 1
  expect_calls "$dir" assess 1
  expect_calls "$dir" recover 0
  if grep -q '^repair ' "$dir/push.log" 2>/dev/null; then problems+=("the repair step pushed"); fi
  grep -q '^assess ' "$dir/push.log" 2>/dev/null || problems+=("the assessment made no repair push")
  tip="$(remote_tip "$dir")"
  git --git-dir="$dir/origin.git" cat-file -e "$tip:fixed.txt" 2>/dev/null || problems+=("the pushed tip does not hold the fix")
  expect_result "$dir" terminal_state EXISTING_OPEN_PR
  expect_result "$dir" terminal_success true
  case "$(result_field "$dir" evidence_used)" in
    *assessment_action=repaired_ci*) ;;
    *) problems+=("evidence_used does not record assessment_action=repaired_ci: $(result_field "$dir" evidence_used)") ;;
  esac
  expect_no_handoff "$dir"
  verdict assessment_finishes_the_repair "$dir" "the assessment pushed the fix and reported repaired_ci; EXISTING_OPEN_PR, exit 0"
else
  dump_case "$WORK/case-assessment_finishes_the_repair"
  die "assessment_finishes_the_repair: cannot set up the case"
fi

# --- H: case 3, a cause the run cannot fix -----------------------------------
if dir="$(new_case unfixable_cause_is_handed_to_its_owner unfixable leave)"; then
  run_case "$dir"
  problems=()
  check_common "$dir"
  expect_exit "$dir" 81
  # One look by each, and then the run ends: nothing here loops.
  expect_calls "$dir" repair 1
  expect_calls "$dir" assess 1
  expect_calls "$dir" recover 1
  [ ! -s "$dir/push.log" ] || problems+=("a push was made for a cause no push can fix")
  [ "$(remote_tip "$dir")" = "$(cat "$dir/initial-tip")" ] || problems+=("the remote tip moved")
  records="$(cat "$dir"/recovery/*.jsonl 2>/dev/null)"
  [ -n "$records" ] || problems+=("no hand-off record")
  for want in BLOCKED_CI "$DEPLOY_CAUSE" "$DEPLOY_STEP"; do
    case "$records" in
      *"$want"*) ;;
      *) problems+=("the hand-off record does not carry '$want'") ;;
    esac
  done
  case "$(cat "$dir/run.err")" in
    *"failed closed: BLOCKED_CI: $DEPLOY_CAUSE; next action: $DEPLOY_STEP"*) ;;
    *) problems+=("the run's stderr lacks the one failure line naming the state, the cause and the step") ;;
  esac
  verdict unfixable_cause_is_handed_to_its_owner "$dir" "BLOCKED_CI with the check, the cause and the owner's step on record; exit 81; one look each, no loop"
else
  dump_case "$WORK/case-unfixable_cause_is_handed_to_its_owner"
  die "unfixable_cause_is_handed_to_its_owner: cannot set up the case"
fi

# --- I: case 4, a check still in progress is waited on -----------------------
if dir="$(new_case pending_checks_are_waited_on pending leave)"; then
  run_case "$dir"
  problems=()
  check_common "$dir"
  expect_exit "$dir" 0
  expect_calls "$dir" repair 1
  expect_calls "$dir" assess 1
  expect_calls "$dir" recover 0
  [ ! -s "$dir/push.log" ] || problems+=("a push was made for a check that only needed waiting on")
  [ "$(remote_tip "$dir")" = "$(cat "$dir/initial-tip")" ] || problems+=("the remote tip moved")
  grep -q '^pr checks 7 .*--watch' "$dir/gh.log" || problems+=("the assessment did not wait with gh pr checks --watch")
  expect_result "$dir" terminal_state EXISTING_OPEN_PR
  expect_result "$dir" terminal_success true
  expect_no_handoff "$dir"
  verdict pending_checks_are_waited_on "$dir" "the assessment waited with gh pr checks --watch and decided on the settled head; EXISTING_OPEN_PR, exit 0, no push"
else
  dump_case "$WORK/case-pending_checks_are_waited_on"
  die "pending_checks_are_waited_on: cannot set up the case"
fi

echo "cases passed: $passed"
if [ "$fail" -eq 0 ] && [ "$passed" -eq 4 ]; then
  echo "issue 2966 red CI at finalisation is repaired, and an unfixable cause is handed to its owner"
  echo "ALL_CASES_PASSED"
  exit 0
fi
exit 1
