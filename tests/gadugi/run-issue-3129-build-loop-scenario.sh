#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #3129 PR 2: the build
# goal loop, end to end, with only the outside world stood in for.
#
# What runs, unchanged: the real `amplihack` binary and the pinned recipe
# runner execute the shipped `default-workflow` recipe (version 4.0.0) on a
# task in a temporary repository whose origin is a local bare repository:
# workflow-prep, workflow-worktree, build-goal-setup, the shared goal loop
# (autodrive-goal-loop) with the build goal tool, and build-report. Each turn
# of the loop is a fresh `amplihack recipe run autodrive-goal-turn`, whose one
# agent step names the next action.
#
# What is stood in for, and nothing else:
#   gh      answers the pull request, issue and account calls the shipped
#           steps make: pr list, pr create, pr view, pr edit, pr ready,
#           pr comment, pr checks, issue view and list, the read-only account
#           calls, the repository's merge methods, its branch rules and
#           protection, and the read of issue #1. It keeps the pull request's
#           title, body and draft flag in $WORK/gh-state, so an edit or a
#           `pr ready` changes what the next read returns. It records every
#           call in $WORK/gh-calls, the arguments and body of `pr create` in
#           $WORK/gh-create.argv and $WORK/gh-create.body, any `pr merge` in
#           $WORK/gh-merge.argv, and exits 96 on anything else (recorded in
#           $WORK/gh-unexpected).
#   claude  gives every agent step of workflow-prep one fixed answer, as the
#           #2806 scenario's build does. The loop's turn (the prompt holding
#           "You are one turn of a goal-driven loop") is answered from the
#           turn's own reading, quoted in the prompt between the
#           UNTRUSTED-READING markers, the way a competent agent would act on
#           it: with no pushed branch it commits the task's file in the run's
#           worktree, pushes with plain `git push` and chooses `continue`;
#           with the branch pushed and no pull request it chooses `open-pr`
#           with the title and body the task text gives; then `mark-ready`;
#           then `ready`. A git step that fails, or a turn past the eighth,
#           ends in a hand-off naming the operator, so a fault ends the run
#           instead of looping. Every call is recorded in
#           $WORK/agent-calls.jsonl.
#
# Cases, from recorded facts only:
#   build_opens_its_pr_with_the_task_text   the stand-in gh recorded one
#       `pr create` whose title is the task's PR title and whose body is the
#       task's PR description, byte for byte.
#   build_marks_its_pr_ready                `pr ready 42` was recorded.
#   build_never_merges                      no `pr merge` was recorded.
#   build_branch_is_pushed_at_the_worktree_head
#       the branch on the bare origin is at the head of the run's worktree on
#       that branch, read from `git worktree list`.
#   build_keeps_its_worktree_and_branch     the worktree and the local branch
#       still exist after the run (software-factory #307: cleanup never
#       removes what a later step needs).
#   build_reports_the_open_pr               the run exited 0 and the
#       build-report step's workflow_result names pull request 42 with
#       terminal_state IMPLEMENTED_VERIFIED or EXISTING_OPEN_PR.
#   pr_ready_tool_marks_a_head_one_commit_behind (#2787)
#       workflow_pr_ready.sh, run against a second stand-in gh that reports
#       the pull request's head one commit behind the local head, marks it
#       ready.
#   pr_ready_tool_reads_the_pr_worktree_from_a_checkout_on_main (#2787)
#       run from a checkout on main with the pull request's branch in a
#       linked worktree, the tool reads that worktree and marks it ready.
#   every_case_this_run_emitted_is_asserted_by_the_descriptor
#
# Isolation is by construction, as in the #2806 scenario: umask 077, a 0700
# root, `env -i` carrying only the variables named in scenario_env, HOME and
# the agent configuration folders inside the root, a PATH holding only
# $WORK/bin, no Git system or global configuration, no prompts, only the file
# transport, and a repository-local insteadOf rule that makes the origin a
# GitHub URL: the build goal names the repository from
# `git config remote.origin.url`. The real ~/.claude and ~/.amplihack are
# never read or written. Nothing reaches GitHub or a live agent.
#
# A missing binary, runner or host tool is a failure with evidence, not a skip.
# CI runs this script directly; it is never run by hand on a shared host.
# Set AMPLIHACK_3129_QA_BIN to an amplihack binary to skip the build, and
# RECIPE_RUNNER_RS_PATH (or put recipe-runner-rs on PATH) for the runner. Set
# QA_KEEP_WORK=1 to keep the temporary root afterwards for a post-mortem.
# On success it prints ALL_CASES_PASSED and exits 0; otherwise SCENARIO_FAILED
# and exit 1, with each evidence file dumped under its own header.
# No `-e` on purpose: every case must run and report. Setup is guarded with
# `|| die`. Nothing here sleeps, polls or times out.
set -uo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
HOST_TMP="${TMPDIR:-/tmp}"

WORK="$(mktemp -d "${HOST_TMP%/}/issue-3129-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }
readonly WORK
chmod 700 "$WORK"

EVIDENCE_FILES=(build.out build.err gh-calls gh-unexpected gh-create.argv gh-create.body
  gh-merge.argv agent-calls.jsonl tools-a.out tools-b.out gh-2787-calls)

on_exit() {
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    local f
    for f in "${EVIDENCE_FILES[@]}"; do
      if [ -f "$WORK/$f" ]; then
        echo "===== $f ====="
        cat -- "$WORK/$f"
      fi
    done
    echo "SCENARIO_FAILED"
  fi
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
BIN="${AMPLIHACK_3129_QA_BIN:-}"
[ -n "$BIN" ] || BIN="$(command -v amplihack 2>/dev/null || true)"
if [ -z "$BIN" ]; then
  BIN="$(cd "$REPO_ROOT" && cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/cargo-build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')" \
    || { tail -n 40 "$WORK/cargo-build.log"; die "cargo build -p amplihack --bin amplihack failed"; }
fi
case "$BIN" in /*) ;; *) BIN="$PWD/$BIN" ;; esac
[ -f "$BIN" ] && [ -x "$BIN" ] || die "no executable amplihack binary (got '$BIN')"
echo "binary under test: $BIN"

RUNNER="${RECIPE_RUNNER_RS_PATH:-}"
[ -n "$RUNNER" ] || RUNNER="$(command -v recipe-runner-rs 2>/dev/null || true)"
[ -n "$RUNNER" ] || die "recipe-runner-rs is required (set RECIPE_RUNNER_RS_PATH or put it on PATH)"
case "$RUNNER" in /*) ;; *) RUNNER="$PWD/$RUNNER" ;; esac
[ -f "$RUNNER" ] && [ -x "$RUNNER" ] || die "recipe-runner-rs is not an executable file: $RUNNER"

# --- B: isolation ------------------------------------------------------------
BINDIR="$WORK/bin"
BIN2787="$WORK/bin-2787"
private_dir() {
  mkdir -p -- "$1" || return 1
  chmod 700 "$1" || return 1
  [ "$(stat -c %a -- "$1")" = 700 ]
}
for d in "$BINDIR" "$BIN2787" "$WORK/home" "$WORK/gh-state" "$WORK/gh-2787" "$WORK/tmp-build" \
    "$WORK/tmp-tools" "$WORK/work-build" "$WORK/work-tools"; do
  private_dir "$d" || die "cannot create a 0700 folder: $d"
done

# Tools a step may call; the list serves the product, not this driver, which
# never runs `sleep` or `timeout` itself.
REQUIRED_TOOLS=(bash sh env git jq cat grep sed awk sort tr head tail cut dirname
  basename mkdir rm cp mv ln chmod date stat mktemp uniq wc find xargs tee
  sha256sum touch readlink realpath cmp comm)
OPTIONAL_TOOLS=(rmdir id uname hostname nproc diff tar gzip egrep fgrep
  python3 sleep timeout tty od base64 printf true false test expr seq cksum iconv)
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

scenario_env() {
  local label="$1"; shift
  env -i \
    HOME="$WORK/home" \
    PATH="$BINDIR" \
    TERM=dumb LANG=C \
    TMPDIR="$WORK/tmp-$label" \
    XDG_CONFIG_HOME="$WORK/home/.config" \
    CLAUDE_CONFIG_DIR="$WORK/home/.claude" \
    CODEX_HOME="$WORK/home/.codex" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_GLOBAL=/dev/null \
    GIT_TERMINAL_PROMPT=0 GIT_ALLOW_PROTOCOL=file GIT_ASKPASS=true SSH_ASKPASS=true \
    GIT_AUTHOR_NAME=qa GIT_AUTHOR_EMAIL=qa@example.invalid \
    GIT_COMMITTER_NAME=qa GIT_COMMITTER_EMAIL=qa@example.invalid \
    AMPLIHACK_HOME="$REPO_ROOT" \
    AMPLIHACK_WORK_ROOT="$WORK/work-$label" AMPLIHACK_WORK_ROOT_EXPLICIT=1 \
    AMPLIHACK_CLAUDE_PROFILE_ROOT="$WORK/home/profiles" \
    AMPLIHACK_LAUNCHER_BINARY="$BIN" AMPLIHACK_AGENT_BINARY=claude \
    AMPLIHACK_CLAUDE_BINARY_PATH="$BINDIR/claude" \
    AMPLIHACK_NONINTERACTIVE=1 AMPLIHACK_GOVERNOR=off AMPLIHACK_ROTATE_AUTOSTART=0 \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 AMPLIHACK_NO_UPDATE_CHECK=1 AMPLIHACK_NO_FRESHNESS_CHECK=1 \
    AMPLIHACK_USAGE_LIMIT_WAIT=0 \
    RECIPE_RUNNER_RS_PATH="$RUNNER" RECIPE_RUNNER_NO_UPDATE_CHECK=1 \
    QA_SCENARIO_ROOT="$WORK" QA_RUN_LABEL="$label" \
    "$@"
}
gitq() { scenario_env build git "$@"; }

# --- C: the fixture repository ----------------------------------------------
ORIGIN="$WORK/origin.git"
GH_URL="https://github.com/qa-owner/qa-repo.git"
{
  gitq init -q --bare -b main "$ORIGIN" \
  && gitq init -q -b main "$WORK/seed" \
  && printf '# qa-repo\n' > "$WORK/seed/README.md" \
  && printf 'target/\n' > "$WORK/seed/.gitignore" \
  && gitq -C "$WORK/seed" add . \
  && gitq -C "$WORK/seed" commit -q -m "base" \
  && gitq -C "$WORK/seed" push -q "$ORIGIN" HEAD:refs/heads/main
} || die "cannot prepare the bare origin"

# `git config --get remote.origin.url` gives the GitHub URL, which the build
# goal reads to name the repository; Git itself rewrites it to the bare folder.
make_clone() {
  local dest="$1"
  gitq clone -q "$ORIGIN" "$dest" \
  && gitq -C "$dest" remote set-url origin "$GH_URL" \
  && gitq -C "$dest" config "url.file://$ORIGIN.insteadOf" "$GH_URL" \
  && gitq -C "$dest" config core.hooksPath /dev/null \
  && gitq -C "$dest" config commit.gpgsign false
}
make_clone "$WORK/build-repo" || die "cannot prepare the build repository"

# The task gives the pull request's own title and description. The stand-in
# agent reads the same two files; the cases compare what gh received with them.
PR_TITLE="Add the feature file"
printf '%s' "$PR_TITLE" > "$WORK/pr-title.txt"
printf 'Adds feature.txt, a one-line text file, at the repository root.\n\nFixes #1\n' > "$WORK/pr-body.txt"
{
  printf 'Add a file named feature.txt at the repository root containing one line of text.\n\n'
  printf 'PR title: %s\n\nPR description, exactly:\n' "$PR_TITLE"
  cat "$WORK/pr-body.txt"
} > "$WORK/task.txt"

# --- D: the stand-ins --------------------------------------------------------
cat > "$BINDIR/gh" <<'GH_STUB'
#!/bin/bash
set -u
root="${QA_SCENARIO_ROOT:?}"
state="$root/gh-state"
printf '%s\n' "$*" >> "$root/gh-calls"

emit() {
  local json="$1" prog="" prev=""
  shift
  for a in "$@"; do
    case "$prev" in --jq|-q) prog="$a" ;; esac
    prev="$a"
  done
  if [ -n "$prog" ]; then printf '%s' "$json" | jq -r "$prog"; else printf '%s\n' "$json"; fi
}
drain_body() {
  local prev="" a
  for a in "$@"; do
    case "$prev$a" in --body-file-|-F-) cat > /dev/null; return 0 ;; esac
    case "$a" in --body-file=-) cat > /dev/null; return 0 ;; esac
    prev="$a"
  done
}
# take_text <args...>: --title/--title=, --body-file into the live text.
take_text() {
  local prev="" a
  for a in "$@"; do
    case "$a" in --title=*) printf '%s' "${a#--title=}" > "$state/title" ;; esac
    case "$prev" in
      --title) printf '%s' "$a" > "$state/title" ;;
      --body-file|-F) [ "$a" = - ] || cp -- "$a" "$state/body" ;;
    esac
    prev="$a"
  done
}

pr_json() {
  local branch oid draft=false
  branch="$(cat "$state/branch" 2>/dev/null || true)"
  oid="$(git --git-dir="$root/origin.git" rev-parse "refs/heads/$branch" 2>/dev/null || true)"
  [ ! -e "$state/draft" ] || draft=true
  jq -nc --arg b "$branch" --arg o "$oid" --argjson d "$draft" \
    --rawfile t "$state/title" --rawfile body "$state/body" \
    '{number:42,url:"https://github.com/qa-owner/qa-repo/pull/42",state:"OPEN",title:$t,
      body:$body,headRefName:$b,headRefOid:$o,baseRefName:"main",isCrossRepository:false,
      isDraft:$d,mergeable:"MERGEABLE",mergeStateStatus:"CLEAN",reviewDecision:"",
      statusCheckRollup:[{"__typename":"CheckRun","name":"ci","status":"COMPLETED","conclusion":"SUCCESS"}],
      mergedAt:null,mergeCommit:null,createdAt:"2026-11-23T00:00:00Z",
      headRepositoryOwner:{login:"qa-owner",id:"qa-owner-id"},
      headRepository:{name:"qa-repo",id:"qa-repo-id"},
      author:{login:"qa-owner"},labels:[],comments:[],reviews:[]}'
}

case "${1-} ${2-}" in
  "auth status") echo "Logged in to github.com as qa-owner"; exit 0 ;;
  "repo view")
    emit '{"nameWithOwner":"qa-owner/qa-repo","name":"qa-repo","owner":{"login":"qa-owner"},"url":"https://github.com/qa-owner/qa-repo","isFork":false,"defaultBranchRef":{"name":"main"},"viewerPermission":"ADMIN"}' "$@"
    exit 0 ;;
  "issue view")
    emit '{"number":1,"title":"Add a feature file","body":"Add feature.txt","state":"OPEN","url":"https://github.com/qa-owner/qa-repo/issues/1","labels":[],"comments":[]}' "$@"
    exit 0 ;;
  "issue list") emit '[]' "$@"; exit 0 ;;
  "issue comment"|"label create") drain_body "$@"; exit 0 ;;
  "issue create") drain_body "$@"; echo "https://github.com/qa-owner/qa-repo/issues/1"; exit 0 ;;
  "api user"|"api /user")
    emit '{"login":"qa-owner","id":1,"type":"User","name":"qa owner"}' "$@"
    exit 0 ;;
  "api rate_limit"|"api /rate_limit")
    emit '{"resources":{"core":{"limit":5000,"used":0,"remaining":5000,"reset":4102444800},"graphql":{"limit":5000,"used":0,"remaining":5000,"reset":4102444800}},"rate":{"limit":5000,"used":0,"remaining":5000,"reset":4102444800}}' "$@"
    exit 0 ;;
  "api repos/{owner}/{repo}"|"api repos/qa-owner/qa-repo")
    emit '{"allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":true}' "$@"
    exit 0 ;;
  "api repos/{owner}/{repo}/issues/1"|"api repos/qa-owner/qa-repo/issues/1")
    emit '{"number":1,"state":"open","pull_request":null,"html_url":"https://github.com/qa-owner/qa-repo/issues/1"}' "$@"
    exit 0 ;;
  "api repos/qa-owner/qa-repo/rules/branches/main") emit '[]' "$@"; exit 0 ;;
  "api repos/qa-owner/qa-repo/branches/main/protection/required_status_checks")
    echo "gh: Branch not protected (HTTP 404)" >&2; exit 1 ;;
  "pr list")
    if [ -e "$state/pr-created" ]; then emit "[$(pr_json)]" "$@"; else emit '[]' "$@"; fi
    exit 0 ;;
  "pr create")
    drain_body "$@"
    printf '%s\n' "$*" >> "$root/gh-create.argv"
    prev=""
    for a in "$@"; do
      case "$a" in
        --head=*) v="${a#--head=}"; printf '%s\n' "${v##*:}" > "$state/branch" ;;
        --draft) : > "$state/draft" ;;
      esac
      [ "$prev" != "--head" ] || printf '%s\n' "${a##*:}" > "$state/branch"
      [ "$prev" != "--body-file" ] || cp -- "$a" "$root/gh-create.body"
      prev="$a"
    done
    take_text "$@"
    [ -f "$state/title" ] || : > "$state/title"
    [ -f "$state/body" ] || : > "$state/body"
    : > "$state/pr-created"
    echo "https://github.com/qa-owner/qa-repo/pull/42"
    exit 0 ;;
  "pr view")
    if [ ! -e "$state/pr-created" ]; then echo "no pull requests found" >&2; exit 1; fi
    emit "$(pr_json)" "$@"
    exit 0 ;;
  "pr edit") drain_body "$@"; take_text "$@"; exit 0 ;;
  "pr ready") rm -f -- "$state/draft"; exit 0 ;;
  "pr comment")
    drain_body "$@"
    mkdir -p "$root/gh-comments"
    n="$(find "$root/gh-comments" -type f | wc -l)"
    prev=""
    for a in "$@"; do
      [ "$prev" != "--body-file" ] || [ "$a" = - ] || cp -- "$a" "$root/gh-comments/$n.md"
      prev="$a"
    done
    exit 0 ;;
  "pr checks")
    printf 'ci\tpass\t1s\thttps://example.invalid/ci\n'
    exit 0 ;;
  "pr merge")
    drain_body "$@"
    printf '%s\n' "$*" >> "$root/gh-merge.argv"
    exit 0 ;;
esac
printf '%s\n' "$*" >> "$root/gh-unexpected"
echo "gh stub: unexpected call: $*" >&2
exit 96
GH_STUB

cat > "$BINDIR/claude" <<'CLAUDE_STUB'
#!/bin/bash
set -u
root="${QA_SCENARIO_ROOT:?}"
if [ "${1-}" = "--version" ]; then echo "2.1.319 (Claude Code)"; exit 0; fi
input="$(cat)"
first="${input%%$'\n'*}"
hash="$(printf '%s' "$input" | sha256sum)"
hash="${hash%% *}"

# In a work tree on a branch other than main, inside the scenario's own root,
# the task's file is written: that is the task's whole change.
top="$(git rev-parse --show-toplevel 2>/dev/null)"
[ -z "$top" ] || top="$(cd "$top" 2>/dev/null && pwd -P)"
case "$top" in "$root"/*) ;; *) top="" ;; esac
branch_here="$(git branch --show-current 2>/dev/null)"
if [ -n "$top" ] && [ -n "$branch_here" ] && [ "$branch_here" != main ] && [ ! -e "$top/feature.txt" ]; then
  printf 'qa feature\n' > "$top/feature.txt"
fi

# workflow-prep's steps: one fixed answer, as the #2806 scenario's build has.
prose='Done. The requirements are clear and the work can start.'
response="$(jq -nc --arg m "$prose" \
  '{workflow:"default-workflow",selected_workflow:"default-workflow",
    task_type:"Development",workstream_count:1,status:"ok",
    verdict:"APPROVED",approved:true,action_taken:"none",
    reason:"the requirements are clear",
    message:$m,summary:$m,narrative:$m}')"
step=fallback

has() { case "$input" in *"$1"*) return 0 ;; esac; return 1; }
if has 'You are one turn of a goal-driven loop'; then
  step=autodrive-goal-turn/decide
  # The turn's reading, quoted in the prompt between its markers.
  reading="$(printf '%s\n' "$input" | awk '$0 == "<<<UNTRUSTED-READING" { f = 1; next } $0 == "UNTRUSTED-READING>>>" { f = 0 } f')"
  printf '%s' "$reading" | jq -e 'type == "object"' >/dev/null 2>&1 || reading='{}'
  r() { printf '%s' "$reading" | jq -r "$1" 2>/dev/null; }
  turn="$(r '.turn // 0')"
  own="$(r '.goal.prs.own // empty')"
  pushed="$(r '.goal.run.pushed_head // empty')"
  wt="$(r '.goal.run.worktree.path // empty')"
  br="$(r '.goal.run.branch // empty')"
  prev="$(r '.previous.outcome // "none"')"
  handoff() {
    jq -nc --arg d "$1" '{action:"handoff",args:{owner:"the operator who started this run",decision:$d,
      text:("The build could not go on: " + $d + ". The operator decides what happens next.")},outcomes:[],
      why:("I read the state and could not take the next step: " + $d + ".")}'
  }
  if [ "${turn:-0}" -gt 8 ]; then
    response="$(handoff "the build did not reach ready in eight turns; the last result was ${prev}")"
  elif [ -z "$own" ] && [ -z "$pushed" ]; then
    if [ -z "$wt" ] || [ -z "$br" ]; then
      response="$(handoff "the reading names no worktree or branch for this run")"
    elif [ -f "$wt/feature.txt" ] || printf 'qa feature\n' > "$wt/feature.txt"; then
      if git -C "$wt" add feature.txt >/dev/null 2>&1 \
          && git -C "$wt" -c user.name=qa -c user.email=qa@example.invalid commit -q -m "Add feature.txt" >/dev/null 2>&1 \
          && git -C "$wt" push -q -u origin "$br" >/dev/null 2>&1; then
        response="$(jq -nc '{action:"continue",args:{},outcomes:[],why:"The branch had no commit for the task, so I added feature.txt, committed it through the hooks and pushed the branch."}')"
      else
        response="$(handoff "committing or pushing feature.txt on ${br} failed")"
      fi
    else
      response="$(handoff "feature.txt could not be written in the worktree")"
    fi
  elif [ -z "$own" ]; then
    response="$(jq -nc --rawfile t "$root/pr-title.txt" --rawfile b "$root/pr-body.txt" \
      '{action:"open-pr",args:{title:$t,body:$b},outcomes:[],why:"The branch is pushed and has no pull request, so I open it with the title and description the task gives."}')"
  elif [ "$prev" != marked-ready ] && [ "$prev" != already-ready ]; then
    response="$(jq -nc '{action:"mark-ready",args:{},outcomes:[],why:"The pull request is open with the task text, so I mark it ready for review."}')"
  else
    response="$(jq -nc '{action:"ready",args:{text:"The change is pushed, and its pull request is open and ready for review."},outcomes:[],why:"The pull request is ready for review, so the build is done."}')"
  fi
fi

jq -nc --arg argv "$*" --arg pwd "$(pwd -P)" --arg hash "$hash" --arg first "$first" \
  --arg step "$step" --arg resp "$response" \
  '{argv:$argv,pwd:$pwd,stdin_sha256:$hash,first_line:$first,step:$step,response:$resp}' \
  >> "$root/agent-calls.jsonl"
printf '%s\n' "$response"
exit 0
CLAUDE_STUB

cat > "$BINDIR/codex" <<'CODEX_STUB'
#!/bin/bash
case "${1-}" in --version) echo "codex-cli 0.0.0"; exit 0 ;; plugin) exit 0 ;; esac
exit 47
CODEX_STUB

# The #2787 cases' own gh: pull request 7 answered from fixture files.
cat > "$BIN2787/gh" <<'GH_2787'
#!/bin/bash
G="${GH_STATE:?}"
printf '%s\n' "$*" >> "$G/../gh-2787-calls"
jqf() { local f=""; while [ $# -gt 0 ]; do case "$1" in --jq|-q) f="$2"; shift 2 ;; *) shift ;; esac; done
  if [ -n "$f" ]; then jq -r "$f"; else cat; fi; }
case "${1:-} ${2:-}" in
  "pr list") cat "$G/prs.json" ;;
  "pr view") jqf "$@" < "$G/pr-view.json" ;;
  "pr ready") : > "$G/ready-called" ;;
  "issue view") echo '{"state":"OPEN","url":"https://github.com/o/r/issues/1"}' | jqf "$@" ;;
  "repo view") echo '{"nameWithOwner":"o/r","defaultBranchRef":{"name":"main"}}' | jqf "$@" ;;
  "auth status") exit 0 ;;
  api*) echo '{}' ;;
  *) echo "stand-in gh: unexpected call: $*" >&2; exit 1 ;;
esac
GH_2787
chmod 700 "$BINDIR/gh" "$BINDIR/claude" "$BINDIR/codex" "$BIN2787/gh"
for f in "${EVIDENCE_FILES[@]}"; do : > "$WORK/$f"; chmod 600 "$WORK/$f"; done

# --- case bookkeeping --------------------------------------------------------
fail=0
passed=0
CASE_NAMES="$WORK/emitted-case-names"
: > "$CASE_NAMES"
pass() {
  echo "PASS: $1"
  passed=$((passed + 1))
  printf '%s\n' "${1%% - *}" >> "$CASE_NAMES"
}
flunk() {
  echo "FAIL: $1"
  fail=1
  local head="${1%%:*}"
  case "$head" in
    ''|*[!A-Za-z0-9_]*) ;;
    *) printf '%s\n' "$head" >> "$CASE_NAMES" ;;
  esac
}

# --- E: the build ------------------------------------------------------------
scenario_env build "$BINDIR/amplihack" recipe run default-workflow \
  -w "$WORK/build-repo" --no-detach -f json \
  --context-from-file "task_description=$WORK/task.txt" \
  -c "repo_path=$WORK/build-repo" -c base_branch=main \
  </dev/null >"$WORK/build.out" 2>"$WORK/build.err"
build_rc=$?
echo "build exit status: $build_rc"

BRANCH="$(cat "$WORK/gh-state/branch" 2>/dev/null)"
[[ $BRANCH =~ ^[A-Za-z0-9._/][A-Za-z0-9._/-]*$ ]] || BRANCH=""
# The run's worktree on that branch, as Git itself lists it.
WT=""
[ -z "$BRANCH" ] || WT="$(gitq -C "$WORK/build-repo" worktree list --porcelain 2>/dev/null \
  | awk -v want="refs/heads/$BRANCH" '/^worktree /{p = substr($0, 10)} /^branch /{if (substr($0, 8) == want) {print p; exit}}')"
WT_HEAD=""
[ -z "$WT" ] || WT_HEAD="$(gitq -C "$WT" rev-parse HEAD 2>/dev/null)"
ORIGIN_HEAD=""
[ -z "$BRANCH" ] || ORIGIN_HEAD="$(gitq --git-dir="$ORIGIN" rev-parse "refs/heads/$BRANCH" 2>/dev/null)"

creates="$(wc -l < "$WORK/gh-create.argv" | tr -d ' ')"
if [ "$creates" != 1 ]; then
  flunk "build_opens_its_pr_with_the_task_text: the stand-in recorded ${creates} pr create calls, not one (build exit $build_rc)"
elif ! grep -qF -- "--title=$PR_TITLE" "$WORK/gh-create.argv"; then
  flunk "build_opens_its_pr_with_the_task_text: pr create was not given the task's title: $(cat "$WORK/gh-create.argv")"
elif ! cmp -s "$WORK/gh-create.body" "$WORK/pr-body.txt"; then
  flunk "build_opens_its_pr_with_the_task_text: the body pr create received differs from the task's description"
else
  pass "build_opens_its_pr_with_the_task_text - $PR_TITLE"
fi

if awk '$1 == "pr" && $2 == "ready" && $3 == "42" { found = 1 } END { exit !found }' "$WORK/gh-calls"; then
  pass "build_marks_its_pr_ready"
else
  flunk "build_marks_its_pr_ready: no 'pr ready 42' call was recorded (build exit $build_rc)"
fi

if [ -s "$WORK/gh-merge.argv" ]; then
  flunk "build_never_merges: the build asked for a merge: $(cat "$WORK/gh-merge.argv")"
else
  pass "build_never_merges"
fi

if [ -z "$BRANCH" ]; then
  flunk "build_branch_is_pushed_at_the_worktree_head: no pull request named a branch"
elif [ -z "$WT_HEAD" ] || [ "$WT_HEAD" != "$ORIGIN_HEAD" ]; then
  flunk "build_branch_is_pushed_at_the_worktree_head: the worktree '${WT:-none}' is at '${WT_HEAD:-nothing}' and origin's '$BRANCH' at '${ORIGIN_HEAD:-nothing}'"
elif ! gitq --git-dir="$ORIGIN" cat-file -e "$ORIGIN_HEAD:feature.txt" 2>/dev/null; then
  flunk "build_branch_is_pushed_at_the_worktree_head: origin's '$BRANCH' does not hold feature.txt"
else
  pass "build_branch_is_pushed_at_the_worktree_head - $BRANCH at $ORIGIN_HEAD"
fi

if [ -z "$BRANCH" ]; then
  flunk "build_keeps_its_worktree_and_branch: no branch to look for"
elif [ -z "$WT" ] || [ ! -d "$WT" ]; then
  flunk "build_keeps_its_worktree_and_branch: Git lists no worktree on '$BRANCH' after the run"
elif ! gitq -C "$WORK/build-repo" rev-parse --verify -q "refs/heads/$BRANCH" >/dev/null; then
  flunk "build_keeps_its_worktree_and_branch: the local branch '$BRANCH' is gone"
else
  pass "build_keeps_its_worktree_and_branch - $WT"
fi

# build-report's output is workflow_result; the run prints it among its step
# results (`-f json`).
RESULT_JSON="$(sed -n '/^{/,$p' "$WORK/build.out" | jq -c '.' 2>/dev/null | tail -n 1)"
WORKFLOW_RESULT="$(printf '%s' "$RESULT_JSON" | jq -r '[.step_results[]? | select(.step_id == "build-report") | .output] | last // empty' 2>/dev/null)"
TS="$(printf '%s' "$WORKFLOW_RESULT" | jq -r '.terminal_state // empty' 2>/dev/null)"
RPR="$(printf '%s' "$WORKFLOW_RESULT" | jq -r '.pr_number // empty' 2>/dev/null)"
if [ "$build_rc" -ne 0 ]; then
  flunk "build_reports_the_open_pr: the build exited $build_rc"
elif [ "$TS" != IMPLEMENTED_VERIFIED ] && [ "$TS" != EXISTING_OPEN_PR ]; then
  flunk "build_reports_the_open_pr: workflow_result's terminal_state is '${TS:-unreadable}' (workflow_result: ${WORKFLOW_RESULT:-none})"
elif [ "$RPR" != 42 ]; then
  flunk "build_reports_the_open_pr: workflow_result names pull request '${RPR:-none}', not 42"
else
  pass "build_reports_the_open_pr - $TS"
fi

# --- F: #2787 at tool level ---------------------------------------------------
# workflow_pr_ready.sh is the 1.2.213 step-21 tool, kept for runs in flight.
# Its stand-in gh answers from $WORK/gh-2787; the origin is never contacted.
G2787="$WORK/gh-2787"
pr7_json() { # pr7_json <head sha>
  jq -nc --arg h "$1" '{number:7,url:"https://github.com/o/r/pull/7",state:"OPEN",isDraft:true,
    headRefName:"feat/x",baseRefName:"main",headRefOid:$h,isCrossRepository:false,title:"T",
    body:"Fixes #1",createdAt:"2026-11-23T00:00:00Z",headRepository:{name:"r"},headRepositoryOwner:{login:"o"}}'
}
tool_repo() { # tool_repo <dir>: on feat/x with two commits; prints the first
  local d="$1"
  gitq init -q -b main "$d" \
  && gitq -C "$d" remote add origin git@github.com:o/r.git \
  && printf 'a\n' > "$d/a.txt" && gitq -C "$d" add . && gitq -C "$d" commit -q -m base \
  && gitq -C "$d" checkout -q -b feat/x \
  && printf '1\n' > "$d/b" && gitq -C "$d" add . && gitq -C "$d" commit -q -m one \
  && gitq -C "$d" rev-parse HEAD \
  && printf '2\n' > "$d/b" && gitq -C "$d" commit -q -am two >/dev/null
}
run_ready() { # run_ready <cwd> <out> [extra VAR=value...]
  local dir="$1" out="$2"; shift 2
  ( cd "$dir" && scenario_env tools env PATH="$BIN2787:$BINDIR" GH_STATE="$G2787" \
      RECIPE_VAR_pr_number=7 RECIPE_VAR_pr_url=https://github.com/o/r/pull/7 "$@" \
      bash "$REPO_ROOT/amplifier-bundle/tools/workflow_pr_ready.sh" --base main ) >"$out" 2>&1
}

FIRST="$(tool_repo "$WORK/tool-a" | tail -n 1)" || FIRST=""
rm -f -- "$G2787/ready-called"
if [ -z "$FIRST" ]; then
  flunk "pr_ready_tool_marks_a_head_one_commit_behind: the fixture repository could not be made"
else
  pr7_json "$FIRST" > "$G2787/pr-view.json"
  printf '[%s]\n' "$(cat "$G2787/pr-view.json")" > "$G2787/prs.json"
  run_ready "$WORK/tool-a" "$WORK/tools-a.out"; rc=$?
  if [ "$rc" -eq 0 ] && [ -e "$G2787/ready-called" ]; then
    pass "pr_ready_tool_marks_a_head_one_commit_behind"
  else
    flunk "pr_ready_tool_marks_a_head_one_commit_behind: exit $rc, pr ready $([ -e "$G2787/ready-called" ] && echo called || echo 'not called')"
  fi
fi

rm -f -- "$G2787/ready-called"
if tool_repo "$WORK/tool-b" >/dev/null \
    && gitq -C "$WORK/tool-b" checkout -q main \
    && gitq -C "$WORK/tool-b" worktree add -q "$WORK/tool-b-wt" feat/x; then
  SECOND="$(gitq -C "$WORK/tool-b-wt" rev-parse HEAD)"
  pr7_json "$SECOND" > "$G2787/pr-view.json"
  printf '[%s]\n' "$(cat "$G2787/pr-view.json")" > "$G2787/prs.json"
  run_ready "$WORK/tool-b" "$WORK/tools-b.out" RECIPE_VAR_existing_branch=feat/x REPO_PATH="$WORK/tool-b"; rc=$?
  if [ "$rc" -eq 0 ] && [ -e "$G2787/ready-called" ]; then
    pass "pr_ready_tool_reads_the_pr_worktree_from_a_checkout_on_main"
  else
    flunk "pr_ready_tool_reads_the_pr_worktree_from_a_checkout_on_main: exit $rc, pr ready $([ -e "$G2787/ready-called" ] && echo called || echo 'not called')"
  fi
else
  flunk "pr_ready_tool_reads_the_pr_worktree_from_a_checkout_on_main: the fixture worktree could not be made"
fi

# --- G: the descriptor states this scenario in full -------------------------
DESCRIPTOR="$REPO_ROOT/tests/gadugi/scenarios/issue-3129-build-loop.yaml"
DESCRIPTOR_CASE="every_case_this_run_emitted_is_asserted_by_the_descriptor"
printf '%s\n' "$DESCRIPTOR_CASE" >> "$CASE_NAMES"
if [ ! -f "$DESCRIPTOR" ]; then
  flunk "the scenario descriptor is missing: $DESCRIPTOR"
else
  EMITTED="$WORK/case-names-sorted"
  ASSERTED="$WORK/descriptor-case-names"
  sort -u -- "$CASE_NAMES" > "$EMITTED"
  sed -n 's/.*expected: *"PASS: \([A-Za-z0-9_]*\)".*/\1/p' "$DESCRIPTOR" | sort -u > "$ASSERTED"
  undescribed="$(comm -23 "$EMITTED" "$ASSERTED" | tr '\n' ' ')"
  unemitted="$(comm -13 "$EMITTED" "$ASSERTED" | tr '\n' ' ')"
  if [ -n "${undescribed// /}" ]; then
    flunk "the descriptor does not assert: ${undescribed% }"
  elif [ -n "${unemitted// /}" ]; then
    flunk "the descriptor asserts cases this run never reached: ${unemitted% }"
  else
    pass "$DESCRIPTOR_CASE"
  fi
fi

echo "cases passed: $passed"
[ "$fail" -eq 0 ] || exit 1
echo "ALL_CASES_PASSED"
