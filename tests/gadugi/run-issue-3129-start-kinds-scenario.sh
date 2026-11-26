#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #3129 PR 2: each kind of
# start that is not a new-branch build, run end to end through
# smart-orchestrator. The new-branch build is the #2806 scenario's part 1 and
# the #3129 build loop scenario.
#
# What runs, unchanged: the real `amplihack` binary and the pinned recipe
# runner execute the shipped `smart-orchestrator` recipe three times, each in
# its own temporary repository whose origin is its own local bare repository:
#
#   continue     a continuation of an existing branch and pull request:
#                `existing_branch` names a branch that is on origin only and
#                has open draft pull request 42, with `allow_existing_pr=true`.
#                smart-orchestrator routes it to default-workflow 4.0.0, whose
#                step 04 checks the branch out from origin and whose build loop
#                finds pull request 42 from the branch.
#   quick        a quick-change (`-c workflow=quick-change`) whose task gives
#                the pull request's own title and description.
#   investigate  an investigation: the classifier's answer is Investigation,
#                so smart-orchestrator routes it to investigation-workflow.
#
# What is stood in for, and nothing else, each with its state kept per run
# (QA_RUN_LABEL names the run):
#   gh      answers the pull request, issue and account calls the shipped
#           steps make, as the #3129 build loop scenario's does. It keeps the
#           pull request's title, body, branch and draft flag in
#           $WORK/gh-state-<run>, so an edit or a `pr ready` changes what the
#           next read returns, records every call in $WORK/gh-calls-<run>,
#           `pr create` in $WORK/gh-create-<run>.argv and .body, any
#           `pr merge` in $WORK/gh-merge-<run>.argv, and exits 96 on any call
#           it does not answer (recorded in $WORK/gh-unexpected-<run>).
#   claude  gives every agent step one fixed answer per run: one line holding
#           one JSON object, the union of the keys the steps on that route
#           parse, as the #2806 scenario's build does. The classifier keys in
#           it choose the route: Development for continue and quick,
#           Investigation for investigate. For quick it also carries the
#           `title` and `body` the task gives, which is what the
#           quick-change-pr-text step returns from such a task. In a work tree
#           inside the scenario's root on a branch other than main it writes
#           the run's task file (second.txt for continue, feature.txt for
#           quick, none for investigate): that is the task's whole change.
#           The build loop's turn (the prompt holding "You are one turn of a
#           goal-driven loop") is answered from the turn's own reading, quoted
#           in the prompt between the UNTRUSTED-READING markers, as a
#           competent agent would act on it, and only for continue: when the
#           pushed branch does not hold second.txt it commits the file in the
#           run's worktree, pushes with plain `git push` and chooses
#           `continue`; with the work pushed and its own pull request found,
#           `mark-ready`; then `ready`. A previous action that failed, a git
#           step that fails, no pull request found on the branch, or a turn
#           in the quick or investigate run ends in a hand-off naming the
#           operator, so a fault ends the run rather than looping. Every call
#           is recorded in $WORK/agent-calls-<run>.jsonl.
#   codex   answers --version and plugin and exits 47 otherwise.
#
# Cases, from recorded facts only:
#   continuation_runs_to_its_end              the run exited 0.
#   continuation_pushes_onto_the_existing_branch
#       origin's branch moved forward from the seeded head, which is its
#       ancestor, and holds greeting.txt and second.txt.
#   continuation_opens_no_second_pr           no `pr create` was recorded.
#   continuation_marks_the_existing_pr_ready  `pr ready 42` was recorded and
#       the pull request no longer reads as a draft.
#   continuation_keeps_the_pr_text            the pull request's title and
#       body are byte for byte the ones it had before the run.
#   continuation_never_merges                 no `pr merge` was recorded.
#   quick_change_runs_to_its_end              the run exited 0.
#   quick_change_opens_its_pr_with_the_task_text
#       one `pr create`, with the task's title and its description byte for
#       byte.
#   quick_change_opens_the_pr_without_the_build_loop
#       no agent call was a build loop turn: quick-change finished on its own
#       route and did not hand the work over.
#   quick_change_pushes_its_branch            the pull request's branch starts
#       with quick/ and is on origin holding feature.txt.
#   quick_change_never_merges                 no `pr merge` was recorded.
#   investigation_runs_to_its_end             the run exited 0.
#   investigation_writes_its_report           a non-empty
#       investigation_report.md is in the run's repository.
#   investigation_changes_nothing_on_origin   no `pr create`; origin holds
#       only main, at the seeded head; the repository's main did not move.
#   every_case_this_run_emitted_is_asserted_by_the_descriptor
#
# Isolation is by construction, as in the #2806 and #3129 scenarios: umask
# 077, a 0700 root, `env -i` carrying only the variables named in
# scenario_env, HOME and the agent configuration folders inside the root (one
# HOME per run), a PATH holding only $WORK/bin, no Git system or global
# configuration, no prompts, only the file transport, and a repository-local
# insteadOf rule that makes each origin a GitHub URL. The commit identity is
# qa@example.com: quick-change refuses a `.invalid` address as unsafe before
# its agent runs. The real ~/.claude and ~/.amplihack are never read or
# written. Nothing reaches GitHub or a live agent.
#
# A missing binary, runner or host tool is a failure, not a skip. CI runs this
# script directly, in the host-install job, after the pinned recipe runner is
# installed; it is never run by hand on a shared host. Set
# AMPLIHACK_3129_START_QA_BIN to an amplihack binary to skip the build, and
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

WORK="$(mktemp -d "${HOST_TMP%/}/issue-3129-start-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }
readonly WORK
chmod 700 "$WORK"

RUNS=(continue quick investigate)
EVIDENCE_FILES=()
for r in "${RUNS[@]}"; do
  EVIDENCE_FILES+=("run-$r.out" "run-$r.err" "gh-calls-$r" "gh-unexpected-$r" "gh-create-$r.argv"
    "gh-create-$r.body" "gh-merge-$r.argv" "agent-calls-$r.jsonl")
done

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
BIN="${AMPLIHACK_3129_START_QA_BIN:-}"
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
private_dir() {
  mkdir -p -- "$1" || return 1
  chmod 700 "$1" || return 1
  [ "$(stat -c %a -- "$1")" = 700 ]
}
for d in "$BINDIR" "$WORK/home-setup" "$WORK/tmp-setup" "$WORK/work-setup"; do
  private_dir "$d" || die "cannot create a 0700 folder: $d"
done
for r in "${RUNS[@]}"; do
  for d in "$WORK/home-$r" "$WORK/tmp-$r" "$WORK/work-$r" "$WORK/gh-state-$r"; do
    private_dir "$d" || die "cannot create a 0700 folder: $d"
  done
done

# Tools a step may call; the list serves the product, not this driver, which
# never runs `sleep` or `timeout` itself. `bc` is here for the investigation
# report's efficiency step.
REQUIRED_TOOLS=(bash sh env git jq cat grep sed awk sort tr head tail cut dirname
  basename mkdir rm cp mv ln chmod date stat mktemp uniq wc find xargs tee
  sha256sum touch readlink realpath cmp comm)
OPTIONAL_TOOLS=(rmdir id uname hostname nproc diff tar gzip egrep fgrep bc
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

# The file each run's task asks for, which the stand-in agent writes.
task_file_of() {
  case "$1" in
    continue) printf 'second.txt' ;;
    quick) printf 'feature.txt' ;;
    *) printf '' ;;
  esac
}

scenario_env() {
  local label="$1"; shift
  env -i \
    HOME="$WORK/home-$label" \
    PATH="$BINDIR" \
    TERM=dumb LANG=C \
    TMPDIR="$WORK/tmp-$label" \
    XDG_CONFIG_HOME="$WORK/home-$label/.config" \
    CLAUDE_CONFIG_DIR="$WORK/home-$label/.claude" \
    CODEX_HOME="$WORK/home-$label/.codex" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_GLOBAL=/dev/null \
    GIT_TERMINAL_PROMPT=0 GIT_ALLOW_PROTOCOL=file GIT_ASKPASS=true SSH_ASKPASS=true \
    GIT_AUTHOR_NAME="qa tester" GIT_AUTHOR_EMAIL=qa@example.com \
    GIT_COMMITTER_NAME="qa tester" GIT_COMMITTER_EMAIL=qa@example.com \
    AMPLIHACK_HOME="$REPO_ROOT" \
    AMPLIHACK_WORK_ROOT="$WORK/work-$label" AMPLIHACK_WORK_ROOT_EXPLICIT=1 \
    AMPLIHACK_CLAUDE_PROFILE_ROOT="$WORK/home-$label/profiles" \
    AMPLIHACK_LAUNCHER_BINARY="$BIN" AMPLIHACK_AGENT_BINARY=claude \
    AMPLIHACK_CLAUDE_BINARY_PATH="$BINDIR/claude" \
    AMPLIHACK_NONINTERACTIVE=1 AMPLIHACK_GOVERNOR=off AMPLIHACK_ROTATE_AUTOSTART=0 \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 AMPLIHACK_NO_UPDATE_CHECK=1 AMPLIHACK_NO_FRESHNESS_CHECK=1 \
    AMPLIHACK_USAGE_LIMIT_WAIT=0 \
    RECIPE_RUNNER_RS_PATH="$RUNNER" RECIPE_RUNNER_NO_UPDATE_CHECK=1 \
    QA_SCENARIO_ROOT="$WORK" QA_RUN_LABEL="$label" QA_TASK_FILE="$(task_file_of "$label")" \
    "$@"
}
gitq() { scenario_env setup git "$@"; }

# --- C: the fixture repositories ---------------------------------------------
GH_URL="https://github.com/qa-owner/qa-repo.git"
CONT_BRANCH="feat/issue-1-greeting"
{
  gitq init -q -b main "$WORK/seed" \
  && printf '# qa-repo\n' > "$WORK/seed/README.md" \
  && printf 'target/\n' > "$WORK/seed/.gitignore" \
  && gitq -C "$WORK/seed" add . \
  && gitq -C "$WORK/seed" commit -q -m "base"
} || die "cannot prepare the seed repository"
BASE_HEAD="$(gitq -C "$WORK/seed" rev-parse HEAD)" || die "cannot read the seed head"
for r in "${RUNS[@]}"; do
  { gitq init -q --bare -b main "$WORK/origin-$r.git" \
    && gitq -C "$WORK/seed" push -q "$WORK/origin-$r.git" HEAD:refs/heads/main; } \
    || die "cannot prepare the bare origin for $r"
done
# The continuation's branch: on origin only, one commit ahead of main.
{
  gitq -C "$WORK/seed" checkout -q -b "$CONT_BRANCH" \
  && printf 'hello\n' > "$WORK/seed/greeting.txt" \
  && gitq -C "$WORK/seed" add greeting.txt \
  && gitq -C "$WORK/seed" commit -q -m "Add greeting.txt" \
  && gitq -C "$WORK/seed" push -q "$WORK/origin-continue.git" "HEAD:refs/heads/$CONT_BRANCH"
} || die "cannot prepare the continuation's branch"
CONT_SEED_HEAD="$(gitq -C "$WORK/seed" rev-parse HEAD)" || die "cannot read the continuation's seeded head"

# `git config --get remote.origin.url` gives the GitHub URL, which the steps
# read to name the repository; Git itself rewrites it to the run's own folder.
make_clone() {
  local r="$1" dest="$WORK/repo-$1"
  gitq clone -q "$WORK/origin-$r.git" "$dest" \
  && gitq -C "$dest" remote set-url origin "$GH_URL" \
  && gitq -C "$dest" config "url.file://$WORK/origin-$r.git.insteadOf" "$GH_URL" \
  && gitq -C "$dest" config core.hooksPath /dev/null \
  && gitq -C "$dest" config commit.gpgsign false
}
for r in "${RUNS[@]}"; do make_clone "$r" || die "cannot prepare the repository for $r"; done

# Pull request 42 on the continuation's branch: open, a draft, with its own
# title and description, as an earlier run left it.
ST="$WORK/gh-state-continue"
printf '%s\n' "$CONT_BRANCH" > "$ST/branch"
printf '%s' "Add the greeting file" > "$ST/title"
printf 'Adds greeting.txt at the repository root.\n\nFixes #1\n' > "$ST/body"
cp -- "$ST/title" "$WORK/continue-pr-title.txt"
cp -- "$ST/body" "$WORK/continue-pr-body.txt"
: > "$ST/draft"
: > "$ST/pr-created"

{
  printf 'Continue the open pull request on the branch %s, which works on issue #1.\n\n' "$CONT_BRANCH"
  printf 'Add a file named second.txt at the repository root containing one line of text, commit it on that branch and push it. '
  printf 'Keep the pull request'"'"'s title and description as they are, and mark it ready for review once the work is pushed.\n'
} > "$WORK/task-continue.txt"

# The quick-change's task gives the pull request's own title and description.
QUICK_TITLE="Add the feature file"
printf '%s' "$QUICK_TITLE" > "$WORK/pr-title-quick.txt"
printf 'Adds feature.txt, a one-line text file, at the repository root.\n\nFixes #1\n' > "$WORK/pr-body-quick.txt"
{
  printf 'Add a file named feature.txt at the repository root containing one line of text.\n\n'
  printf 'PR title: %s\n\nPR description, exactly:\n' "$QUICK_TITLE"
  cat "$WORK/pr-body-quick.txt"
} > "$WORK/task-quick.txt"

printf '%s\n' "Investigate what the README file in this repository says the project is, and report the answer. Change no file, open no pull request and push nothing." \
  > "$WORK/task-investigate.txt"

# --- D: the stand-ins --------------------------------------------------------
cat > "$BINDIR/gh" <<'GH_STUB'
#!/bin/bash
set -u
root="${QA_SCENARIO_ROOT:?}"
label="${QA_RUN_LABEL:?}"
state="$root/gh-state-$label"
printf '%s\n' "$*" >> "$root/gh-calls-$label"

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
  oid="$(git --git-dir="$root/origin-$label.git" rev-parse "refs/heads/$branch" 2>/dev/null || true)"
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
  "api --include")
    case "${!#}" in
      repos/qa-owner/qa-repo/rules/branches/main) body='[]' ;;
      repos/qa-owner/qa-repo/branches/main/protection/required_status_checks)
        printf 'HTTP/2 404\r\n\r\n'; echo 'gh: Branch not protected (HTTP 404)' >&2; exit 1 ;;
      repos/qa-owner/qa-repo)
        body='{"allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":true}' ;;
      repos/qa-owner/qa-repo/commits/*/check-runs\?*)
        body='{"check_runs":[{"name":"ci","status":"completed","conclusion":"success"}]}' ;;
      repos/qa-owner/qa-repo/commits/*/status\?*) body='{"statuses":[]}' ;;
      *) body='' ;;
    esac
    if [ -n "$body" ]; then printf 'HTTP/2 200\r\n\r\n%s\n' "$body"; exit 0; fi ;;
  "auth status") echo "Logged in to github.com as qa-owner"; exit 0 ;;
  "repo view")
    emit '{"nameWithOwner":"qa-owner/qa-repo","name":"qa-repo","owner":{"login":"qa-owner"},"url":"https://github.com/qa-owner/qa-repo","isFork":false,"defaultBranchRef":{"name":"main"},"viewerPermission":"ADMIN"}' "$@"
    exit 0 ;;
  "issue view")
    emit '{"number":1,"title":"Add the greeting and feature files","body":"Add greeting.txt, second.txt and feature.txt","state":"OPEN","url":"https://github.com/qa-owner/qa-repo/issues/1","labels":[],"comments":[]}' "$@"
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
    printf '%s\n' "$*" >> "$root/gh-create-$label.argv"
    prev=""
    for a in "$@"; do
      case "$a" in
        --head=*) v="${a#--head=}"; printf '%s\n' "${v##*:}" > "$state/branch" ;;
        --draft) : > "$state/draft" ;;
      esac
      [ "$prev" != "--head" ] || printf '%s\n' "${a##*:}" > "$state/branch"
      [ "$prev" != "--body-file" ] || cp -- "$a" "$root/gh-create-$label.body"
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
  "pr comment") drain_body "$@"; exit 0 ;;
  "pr checks")
    printf 'ci\tpass\t1s\thttps://example.invalid/ci\n'
    exit 0 ;;
  "pr merge")
    drain_body "$@"
    printf '%s\n' "$*" >> "$root/gh-merge-$label.argv"
    exit 0 ;;
esac
printf '%s\n' "$*" >> "$root/gh-unexpected-$label"
echo "gh stub: unexpected call: $*" >&2
exit 96
GH_STUB

cat > "$BINDIR/claude" <<'CLAUDE_STUB'
#!/bin/bash
set -u
root="${QA_SCENARIO_ROOT:?}"
label="${QA_RUN_LABEL:-}"
task_file="${QA_TASK_FILE:-}"
if [ "${1-}" = "--version" ]; then echo "2.1.319 (Claude Code)"; exit 0; fi
input="$(cat)"
first="${input%%$'\n'*}"
hash="$(printf '%s' "$input" | sha256sum)"
hash="${hash%% *}"

# In a work tree on a branch other than main, inside the scenario's own root,
# the run's task file is written: that is the task's whole change.
top="$(git rev-parse --show-toplevel 2>/dev/null)"
[ -z "$top" ] || top="$(cd "$top" 2>/dev/null && pwd -P)"
case "$top" in "$root"/*) ;; *) top="" ;; esac
branch_here="$(git branch --show-current 2>/dev/null)"
if [ -n "$task_file" ] && [ -n "$top" ] && [ -n "$branch_here" ] && [ "$branch_here" != main ] && [ ! -e "$top/$task_file" ]; then
  printf 'qa change\n' > "$top/$task_file"
fi

# One fixed answer per run: one line, one JSON object, the union of the keys
# the steps on that route parse. The classifier keys choose the route.
prose='Done. The work the task asks for is complete.'
case "$label" in
  quick)
    response="$(jq -nc --arg m "$prose" --rawfile t "$root/pr-title-quick.txt" --rawfile b "$root/pr-body-quick.txt" \
      '{workflow:"quick-change",selected_workflow:"quick-change",recommended_workflow:"quick-change",
        workflow_reason:"a one-file change on a new branch",
        task_type:"Development",workstream_count:1,status:"ok",goal_status:"ACHIEVED",
        verdict:"APPROVED",approved:true,action_taken:"none",no_merge:false,
        reason:"the change is small and clear",title:$t,body:$b,
        message:$m,summary:$m,narrative:$m}')" ;;
  investigate)
    response="$(jq -nc --arg m "$prose" \
      '{workflow:"investigation-workflow",task_type:"Investigation",workstream_count:1,
        workstreams:[{name:"readme",description:"what the README says",recipe:"investigation-workflow"}],
        status:"ok",goal_status:"ACHIEVED",verdict:"APPROVED",approved:true,action_taken:"none",
        no_merge:false,reason:"the question is answered from the README",
        executive_summary:"The README names the project qa-repo.",
        key_insights:["The README holds one heading, qa-repo."],remaining_unknowns:[],
        message:$m,summary:$m,narrative:$m}')" ;;
  *)
    response="$(jq -nc --arg m "$prose" \
      '{workflow:"default-workflow",selected_workflow:"default-workflow",recommended_workflow:"default-workflow",
        workflow_reason:"a continuation of an existing branch and pull request",
        task_type:"Development",workstream_count:1,status:"ok",goal_status:"ACHIEVED",
        verdict:"APPROVED",approved:true,action_taken:"none",no_merge:false,
        reason:"the requirements are clear",
        message:$m,summary:$m,narrative:$m}')" ;;
esac
step=fallback

has() { case "$input" in *"$1"*) return 0 ;; esac; return 1; }
if has 'You are one turn of a goal-driven loop'; then
  step=autodrive-goal-turn/decide
  reading="$(printf '%s\n' "$input" | awk '$0 == "<<<UNTRUSTED-READING" { f = 1; next } $0 == "UNTRUSTED-READING>>>" { f = 0 } f')"
  printf '%s' "$reading" | jq -e 'type == "object"' >/dev/null 2>&1 || reading='{}'
  r() { printf '%s' "$reading" | jq -r "$1" 2>/dev/null; }
  own="$(r '.goal.prs.own // empty')"
  pushed="$(r '.goal.run.pushed_head // empty')"
  wt="$(r '.goal.run.worktree.path // empty')"
  br="$(r '.goal.run.branch // empty')"
  prev="$(r '.previous.outcome // "none"')"
  work_pushed=false
  if [ -n "$task_file" ] && [ -n "$pushed" ] && [ -n "$wt" ] && git -C "$wt" cat-file -e "${pushed}:${task_file}" 2>/dev/null; then
    work_pushed=true
  fi
  handoff() {
    jq -nc --arg d "$1" '{action:"handoff",args:{owner:"the operator who started this run",decision:$d,
      text:("The run could not go on: " + $d + ". The operator decides what happens next.")},outcomes:[],
      why:("I read the state and could not take the next step: " + $d + ".")}'
  }
  case "$prev" in
    none|continued|continue|opened|already-open|reviewed|marked-ready|already-ready|checks-complete|checks-pending|guidance-arrived|skipped-guidance) prev_ok=true ;;
    *) prev_ok=false ;;
  esac
  if [ "$label" != continue ]; then
    response="$(handoff "the ${label} run reached the build loop, which this route should not need")"
  elif [ "$prev_ok" != true ]; then
    response="$(handoff "the previous action ended ${prev}")"
  elif [ "$work_pushed" != true ]; then
    if [ -z "$wt" ] || [ -z "$br" ]; then
      response="$(handoff "the reading names no worktree or branch for this run")"
    elif { [ -f "$wt/$task_file" ] || printf 'qa change\n' > "$wt/$task_file"; } \
        && git -C "$wt" add -- "$task_file" >/dev/null 2>&1 \
        && git -C "$wt" commit -q -m "Add $task_file" >/dev/null 2>&1 \
        && git -C "$wt" push -q origin "$br" >/dev/null 2>&1; then
      response="$(jq -nc --arg f "$task_file" '{action:"continue",args:{},outcomes:[],why:("The branch did not hold " + $f + ", so I added it, committed it and pushed the branch.")}')"
    else
      response="$(handoff "writing, committing or pushing ${task_file} on ${br} failed")"
    fi
  elif [ -z "$own" ]; then
    response="$(handoff "no pull request was found on the existing branch ${br}")"
  elif [ "$prev" != marked-ready ] && [ "$prev" != already-ready ]; then
    response="$(jq -nc '{action:"mark-ready",args:{},outcomes:[],why:"The work is pushed to the existing pull request, so I mark it ready for review."}')"
  else
    response="$(jq -nc '{action:"ready",args:{text:"The work is pushed to the existing pull request, which is ready for review."},outcomes:[],why:"The pull request is ready for review, so the continuation is done."}')"
  fi
fi

jq -nc --arg argv "$*" --arg pwd "$(pwd -P)" --arg hash "$hash" --arg first "$first" \
  --arg step "$step" --arg resp "$response" \
  '{argv:$argv,pwd:$pwd,stdin_sha256:$hash,first_line:$first,step:$step,response:$resp}' \
  >> "$root/agent-calls-${label:-unknown}.jsonl"
printf '%s\n' "$response"
exit 0
CLAUDE_STUB

cat > "$BINDIR/codex" <<'CODEX_STUB'
#!/bin/bash
case "${1-}" in --version) echo "codex-cli 0.0.0"; exit 0 ;; plugin) exit 0 ;; esac
exit 47
CODEX_STUB
chmod 700 "$BINDIR/gh" "$BINDIR/claude" "$BINDIR/codex"
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
# called <run> <verb>: gh recorded `pr <verb>` in that run.
called() { awk -v v="$2" '$1 == "pr" && $2 == v { found = 1 } END { exit !found }' "$WORK/gh-calls-$1"; }

run_kind() { # run_kind <run> [-c key=value...]
  local r="$1"; shift
  scenario_env "$r" "$BINDIR/amplihack" recipe run smart-orchestrator \
    -w "$WORK/repo-$r" --no-detach \
    --context-from-file "task_description=$WORK/task-$r.txt" \
    -c "repo_path=$WORK/repo-$r" -c force_single_workstream=true "$@" \
    </dev/null >"$WORK/run-$r.out" 2>"$WORK/run-$r.err"
}

# --- E: the continuation -----------------------------------------------------
run_kind continue -c base_branch=main -c "existing_branch=$CONT_BRANCH" -c allow_existing_pr=true
cont_rc=$?
echo "continuation exit status: $cont_rc"

if [ "$cont_rc" -eq 0 ]; then pass "continuation_runs_to_its_end"
else flunk "continuation_runs_to_its_end: the run exited $cont_rc"; fi

CONT_HEAD="$(gitq --git-dir="$WORK/origin-continue.git" rev-parse "refs/heads/$CONT_BRANCH" 2>/dev/null)"
if [ -z "$CONT_HEAD" ] || [ "$CONT_HEAD" = "$CONT_SEED_HEAD" ]; then
  flunk "continuation_pushes_onto_the_existing_branch: origin's '$CONT_BRANCH' is at '${CONT_HEAD:-nothing}', the seeded head"
elif ! gitq --git-dir="$WORK/origin-continue.git" merge-base --is-ancestor "$CONT_SEED_HEAD" "$CONT_HEAD"; then
  flunk "continuation_pushes_onto_the_existing_branch: origin's '$CONT_BRANCH' at $CONT_HEAD does not descend from the seeded head $CONT_SEED_HEAD"
elif ! gitq --git-dir="$WORK/origin-continue.git" cat-file -e "$CONT_HEAD:second.txt" 2>/dev/null \
    || ! gitq --git-dir="$WORK/origin-continue.git" cat-file -e "$CONT_HEAD:greeting.txt" 2>/dev/null; then
  flunk "continuation_pushes_onto_the_existing_branch: origin's '$CONT_BRANCH' at $CONT_HEAD does not hold both greeting.txt and second.txt"
else
  pass "continuation_pushes_onto_the_existing_branch - $CONT_BRANCH at $CONT_HEAD"
fi

if called continue create; then
  flunk "continuation_opens_no_second_pr: the run asked for a new pull request: $(cat "$WORK/gh-create-continue.argv")"
else
  pass "continuation_opens_no_second_pr"
fi

if ! awk '$1 == "pr" && $2 == "ready" && $3 == "42" { found = 1 } END { exit !found }' "$WORK/gh-calls-continue"; then
  flunk "continuation_marks_the_existing_pr_ready: no 'pr ready 42' call was recorded (exit $cont_rc)"
elif [ -e "$WORK/gh-state-continue/draft" ]; then
  flunk "continuation_marks_the_existing_pr_ready: pull request 42 still reads as a draft"
else
  pass "continuation_marks_the_existing_pr_ready"
fi

if cmp -s "$WORK/gh-state-continue/title" "$WORK/continue-pr-title.txt" \
    && cmp -s "$WORK/gh-state-continue/body" "$WORK/continue-pr-body.txt"; then
  pass "continuation_keeps_the_pr_text"
else
  flunk "continuation_keeps_the_pr_text: the pull request's title or body changed during the run"
fi

if [ -s "$WORK/gh-merge-continue.argv" ]; then
  flunk "continuation_never_merges: the run asked for a merge: $(cat "$WORK/gh-merge-continue.argv")"
else
  pass "continuation_never_merges"
fi

# --- F: the quick-change -------------------------------------------------------
run_kind quick -c base_branch=main -c workflow=quick-change
quick_rc=$?
echo "quick-change exit status: $quick_rc"

if [ "$quick_rc" -eq 0 ]; then pass "quick_change_runs_to_its_end"
else flunk "quick_change_runs_to_its_end: the run exited $quick_rc"; fi

creates="$(wc -l < "$WORK/gh-create-quick.argv" | tr -d ' ')"
if [ "$creates" != 1 ]; then
  flunk "quick_change_opens_its_pr_with_the_task_text: the stand-in recorded ${creates} pr create calls, not one (exit $quick_rc)"
elif ! grep -qF -- "--title=$QUICK_TITLE" "$WORK/gh-create-quick.argv"; then
  flunk "quick_change_opens_its_pr_with_the_task_text: pr create was not given the task's title: $(cat "$WORK/gh-create-quick.argv")"
elif ! cmp -s "$WORK/gh-create-quick.body" "$WORK/pr-body-quick.txt"; then
  flunk "quick_change_opens_its_pr_with_the_task_text: the body pr create received differs from the task's description"
else
  pass "quick_change_opens_its_pr_with_the_task_text - $QUICK_TITLE"
fi

if grep -q '"step":"autodrive-goal-turn/decide"' "$WORK/agent-calls-quick.jsonl"; then
  flunk "quick_change_opens_the_pr_without_the_build_loop: quick-change handed its work to the build loop"
else
  pass "quick_change_opens_the_pr_without_the_build_loop"
fi

QBRANCH="$(cat "$WORK/gh-state-quick/branch" 2>/dev/null)"
[[ $QBRANCH =~ ^[A-Za-z0-9._/][A-Za-z0-9._/-]*$ ]] || QBRANCH=""
QHEAD=""
[ -z "$QBRANCH" ] || QHEAD="$(gitq --git-dir="$WORK/origin-quick.git" rev-parse "refs/heads/$QBRANCH" 2>/dev/null)"
case "$QBRANCH" in
  quick/*)
    if [ -z "$QHEAD" ]; then
      flunk "quick_change_pushes_its_branch: origin has no branch '$QBRANCH'"
    elif ! gitq --git-dir="$WORK/origin-quick.git" cat-file -e "$QHEAD:feature.txt" 2>/dev/null; then
      flunk "quick_change_pushes_its_branch: origin's '$QBRANCH' at $QHEAD does not hold feature.txt"
    else
      pass "quick_change_pushes_its_branch - $QBRANCH at $QHEAD"
    fi ;;
  *) flunk "quick_change_pushes_its_branch: the pull request's branch is '${QBRANCH:-none}', not a quick/ branch" ;;
esac

if [ -s "$WORK/gh-merge-quick.argv" ]; then
  flunk "quick_change_never_merges: the run asked for a merge: $(cat "$WORK/gh-merge-quick.argv")"
else
  pass "quick_change_never_merges"
fi

# --- G: the investigation ------------------------------------------------------
run_kind investigate
inv_rc=$?
echo "investigation exit status: $inv_rc"

if [ "$inv_rc" -eq 0 ]; then pass "investigation_runs_to_its_end"
else flunk "investigation_runs_to_its_end: the run exited $inv_rc"; fi

REPORT="$(find "$WORK/repo-investigate" -name investigation_report.md -type f -size +0 2>/dev/null | awk 'NR == 1')"
if [ -n "$REPORT" ]; then
  pass "investigation_writes_its_report - ${REPORT#"$WORK/"}"
else
  flunk "investigation_writes_its_report: no non-empty investigation_report.md in the run's repository"
fi

INV_REFS="$(gitq --git-dir="$WORK/origin-investigate.git" for-each-ref --format='%(refname) %(objectname)' 2>/dev/null)"
INV_MAIN="$(gitq -C "$WORK/repo-investigate" rev-parse refs/heads/main 2>/dev/null)"
if called investigate create; then
  flunk "investigation_changes_nothing_on_origin: the run asked for a pull request: $(cat "$WORK/gh-create-investigate.argv")"
elif [ "$INV_REFS" != "refs/heads/main $BASE_HEAD" ]; then
  flunk "investigation_changes_nothing_on_origin: origin's refs are now: $(printf '%s' "$INV_REFS" | tr '\n' ';')"
elif [ "$INV_MAIN" != "$BASE_HEAD" ]; then
  flunk "investigation_changes_nothing_on_origin: the repository's main moved to '${INV_MAIN:-nothing}'"
else
  pass "investigation_changes_nothing_on_origin"
fi

# --- H: the descriptor states this scenario in full -------------------------
DESCRIPTOR="$REPO_ROOT/tests/gadugi/scenarios/issue-3129-start-kinds.yaml"
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
