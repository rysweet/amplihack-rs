#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2806: one whole build and
# one whole drive, end to end, with only the outside world stood in for.
#
# What runs, unchanged: the real `amplihack` binary and the pinned recipe
# runner execute the shipped `smart-orchestrator` recipe on a task, routed to
# `default-workflow`, in a temporary repository whose origin is a local bare
# repository; then the shipped `auto-drive-to-merge` recipe on the pull request
# that build opened, from a per-PR clone with a linked worktree added from it.
#
# What is stood in for, and nothing else:
#   gh      answers auth status, repo view, issue view, issue list, issue
#           create, issue comment, label create, pr list, pr create, pr view,
#           pr checks, pr merge, pr ready/edit/comment, and the two read-only
#           account calls `api user` and `api rate_limit`. It reads the body from
#           standard input exactly where real gh does (`--body-file -`),
#           records every call in $WORK/gh-calls, and exits 96 on anything
#           else, GraphQL included (recorded in
#           $WORK/gh-unexpected). The pull request's checks are PENDING for the
#           build and SUCCESS for the drive; the state lives in
#           $WORK/gh-state/checks and is written below, in this file, once
#           before each run.
#   claude  reads and discards the prompt and writes one fixed answer, whatever
#           step asked. Its first call also leaves one repository copy with an
#           uncommitted file in its own TMPDIR, which part 3 looks for. Every
#           call is recorded in $WORK/agent-calls.jsonl (argv, working
#           directory, HOME, a hash of the prompt, its first line, the answer).
#           The answer is one line holding one JSON object: the union of the
#           keys the steps on the path parse, with values that steer one
#           legitimate route, and the sentence a step without parse_json would
#           read as a field of that object rather than beside it, because a
#           step that declares parse_json parses the whole of its agent's
#           stdout as exactly one object. It never looks at the prompt's
#           wording and never counts calls. Where one value provably cannot
#           serve two steps the stub may match the prompt's structural first
#           line exactly; the list of such cases is below and is empty until
#           CI evidence shows one is needed:
#             (none)
#   codex   answers --version and plugin, records every call in
#           $WORK/codex-calls, exits 47 otherwise. It is on PATH only so a
#           probe cannot reach a host codex.
#
# Part 1  the whole build. Checks are PENDING. The run ends with a pull request
#         opened on the stub for the run's branch and that branch pushed to the
#         bare origin at the local head, and finalisation reports a terminal
#         state that is not FAILED_IMPLEMENTATION and names the open pull
#         request (issue 2343: pending checks hand the pull request on).
# Part 2  the whole drive. Checks are SUCCESS, so the merge step's precondition
#         (no pending required check) holds. The stub recorded a `pr merge` for
#         that pull request and the drive printed its own terminal report, the
#         one object carrying `stop_reason`, which is its verdict however it
#         ended. The sentence beside it is the product's and is not read.
#         Merge permission comes from the product's own capture of
#         authorisation, from no_merge=false and autodrive_dry_run=false;
#         nothing is written into the drive's state by hand. The pull request
#         this task opens against rysweet/amplihack-rs is the deputy's to
#         merge and is never acted on here: the merge above is against the
#         stub.
# Part 3  what survived (issue 2785, issue 2802). Over the evidence of the two
#         runs, not a third run: the committed work is still on the branch in
#         the bare origin and readable in the per-PR worktree, and for the
#         repository copy the stand-in agent left a file in, the run's stderr
#         carries a line beginning `amplihack: leaving ` that names the folder
#         kept, that copy as the work tree, and the reason, and the folder is
#         still there with the file in it. The reason the product gives for a
#         copy with a file nobody committed is `has uncommitted changes`; the
#         rest of the line is read from the line itself, because the folder it
#         names belongs to the run's own layout. The line is accepted once or
#         twice, identical each time: at the end of a run the runner's own
#         removal and the end-of-run sweep can both reach a folder and each
#         says so once. A whole build may keep other folders too; their lines
#         are not read. No internal run-folder path or layout is asserted.
#
# Cleanup is left on: AMPLIHACK_CLEANUP_ON_EXIT, AMPLIHACK_SKIP_REFLECTION and
# AMPLIHACK_SKIP_POWER_STEERING are not set. Context keys that choose a route
# (workflow, force_single_workstream) are used; nothing that removes a phase is.
#
# Isolation is by construction: umask 077, a 0700 root, `env -i` carrying only
# the variables named in scenario_env, HOME and the agent configuration folders
# inside the root, a PATH holding only $WORK/bin (links to an explicit list of
# host tools, the binary, the runner and the three stand-ins), no Git system or
# global configuration, no prompts, only the file transport, and a
# repository-local insteadOf rule that makes the origin look like a GitHub
# remote. The real ~/.claude and ~/.amplihack are never read or written. The
# tool list below only links tools the product's own steps may call; this
# driver runs none of them as a pause or a time limit.
#
# A missing binary, runner or host tool is a failure with evidence, not a skip.
# A last case holds the descriptor beside this script to the PASS lines this
# run emitted. CI runs this script directly.
#
# Run by hand: bash tests/gadugi/run-issue-2806-end-to-end-build-and-drive-scenario.sh
# Set AMPLIHACK_2806_QA_BIN to an amplihack binary to skip the build, and
# RECIPE_RUNNER_RS_PATH (or put recipe-runner-rs on PATH) for the runner.
# On success it prints ALL_CASES_PASSED and exits 0; otherwise SCENARIO_FAILED
# and exit 1, with each evidence file dumped under its own header.
set -uo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
HOST_TMP="${TMPDIR:-/tmp}"

# --- the temporary root and the failure dump (section I) --------------------
WORK="$(mktemp -d "${HOST_TMP%/}/issue-2806-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }
readonly WORK
chmod 700 "$WORK"

EVIDENCE_FILES=(build.out build.err drive.out drive.err gh-calls gh-unexpected
  gh-create.argv gh-create.cwd gh-merge.argv agent-calls.jsonl codex-calls)

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
  # Only the folder this script made: non-empty, not the root, not the
  # invoking user's home, and under the temporary folder it was made in.
  case "$WORK" in
    ""|/) ;;
    "${HOME:-/nonexistent}"|"${HOME:-/nonexistent}"/) ;;
    "${HOST_TMP%/}"/*) rm -rf -- "$WORK" ;;
  esac
  exit "$rc"
}
trap on_exit EXIT

die() {
  echo "FAIL: $*"
  exit 1
}

# --- A: preconditions --------------------------------------------------------
BIN="${AMPLIHACK_2806_QA_BIN:-}"
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
for d in "$BINDIR" "$WORK/home" "$WORK/gh-state" "$WORK/tmp-build" "$WORK/tmp-drive" \
    "$WORK/work-build" "$WORK/work-drive"; do
  private_dir "$d" || die "cannot create a 0700 folder: $d"
done

# Tools a step may call. The first group is required; the rest are linked when
# the host has them.
REQUIRED_TOOLS=(bash sh env git jq cat grep sed awk sort tr head tail cut dirname
  basename mkdir rm cp mv ln chmod date stat mktemp uniq wc find xargs tee
  sha256sum touch readlink realpath)
OPTIONAL_TOOLS=(rmdir id uname hostname nproc diff cmp tar gzip egrep fgrep
  python3 sleep timeout tty od base64 printf true false test expr seq)
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

# One wrapper for every command that runs the product or Git: env -i with an
# explicit list. Reflection, power steering and cleanup are deliberately left to
# their defaults.
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
  && printf '[workspace.package]\nversion = "0.1.0"\n' > "$WORK/seed/Cargo.toml" \
  && printf 'target/\n' > "$WORK/seed/.gitignore" \
  && gitq -C "$WORK/seed" add . \
  && gitq -C "$WORK/seed" commit -q -m "base" \
  && gitq -C "$WORK/seed" push -q "$ORIGIN" HEAD:refs/heads/main
} || die "cannot prepare the bare origin"

# A clone whose origin is a GitHub-shaped URL that Git rewrites to the bare
# repository, so the product parses a GitHub identity and no transport leaves
# the root.
#
# The two ways of reading the remote differ here, by Git's own design, and the
# difference is deliberate: `git config --get remote.origin.url` gives the
# GitHub URL, which is what the steps that decide the repository's identity
# read, while `git remote get-url origin` applies the rewrite and gives the
# file URL. The one shipped step that reads the second,
# workflow_identity_preflight.sh, skips a remote it cannot read as a GitHub
# one, which is a skip and not a failure.
make_clone() {
  local dest="$1"
  gitq clone -q "$ORIGIN" "$dest" \
  && gitq -C "$dest" remote set-url origin "$GH_URL" \
  && gitq -C "$dest" config "url.file://$ORIGIN.insteadOf" "$GH_URL" \
  && gitq -C "$dest" config core.hooksPath /dev/null \
  && gitq -C "$dest" config commit.gpgsign false
}
make_clone "$WORK/build-repo" || die "cannot prepare the build repository"

# --- D: the stand-ins --------------------------------------------------------
cat > "$BINDIR/gh" <<'GH_STUB'
#!/bin/bash
set -u
root="${QA_SCENARIO_ROOT:?}"
state="$root/gh-state"
printf '%s\n' "$*" >> "$root/gh-calls"
branch_file="$state/branch"

# Apply a --jq/-q program (data for jq, never for a shell) to a JSON document.
emit() {
  local json="$1" prog="" prev=""
  shift
  for a in "$@"; do
    case "$prev" in --jq|-q) prog="$a" ;; esac
    prev="$a"
  done
  if [ -n "$prog" ]; then printf '%s' "$json" | jq -r "$prog"; else printf '%s\n' "$json"; fi
}
want_json() { for a in "$@"; do [ "$a" = "--json" ] && return 0; done; return 1; }

# Real gh reads the body from standard input for `--body-file -` (and `-F -`),
# so a stub that never read it would leave the writer on a closed pipe. Only
# then: with no such argument, stdin may be a pipe nobody ever writes to or
# closes, and reading it would never return.
drain_body() {
  local prev="" a
  for a in "$@"; do
    case "$prev$a" in --body-file-|-F-) cat > /dev/null; return 0 ;; esac
    case "$a" in --body-file=-) cat > /dev/null; return 0 ;; esac
    prev="$a"
  done
}

pr_json() {
  local branch oid pr_state="OPEN" merged_at=null rollup
  branch="$(cat "$branch_file" 2>/dev/null || true)"
  oid="$(git --git-dir="$root/origin.git" rev-parse "refs/heads/$branch" 2>/dev/null || true)"
  if [ -e "$state/merged" ]; then
    pr_state="MERGED"
    merged_at='"2026-11-20T00:00:00Z"'
  fi
  if [ "$(cat "$state/checks" 2>/dev/null)" = SUCCESS ]; then
    rollup='[{"__typename":"CheckRun","name":"ci","status":"COMPLETED","conclusion":"SUCCESS"}]'
  else
    rollup='[{"__typename":"CheckRun","name":"ci","status":"IN_PROGRESS","conclusion":null}]'
  fi
  jq -nc --arg b "$branch" --arg o "$oid" --arg s "$pr_state" --argjson r "$rollup" \
    --argjson m "$merged_at" \
    '{number:42,url:"https://github.com/qa-owner/qa-repo/pull/42",state:$s,title:"qa change",
      body:"qa",headRefName:$b,headRefOid:$o,baseRefName:"main",isCrossRepository:false,
      isDraft:false,mergeable:"MERGEABLE",mergeStateStatus:"CLEAN",reviewDecision:"APPROVED",
      statusCheckRollup:$r,mergedAt:$m,mergeCommit:null,
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
    emit '{"number":1,"title":"Add a greeting file","body":"Add feature.txt","state":"OPEN","labels":[],"comments":[]}' "$@"
    exit 0 ;;
  "issue list") emit '[]' "$@"; exit 0 ;;
  "issue comment"|"label create") drain_body "$@"; exit 0 ;;
  # workflow-prep creates the task's issue and reads the last URL printed.
  "issue create") drain_body "$@"; echo "https://github.com/qa-owner/qa-repo/issues/1"; exit 0 ;;
  # The two read-only account calls whose answer is not a matter of judgement.
  # Every other `gh api` call, GraphQL included, is unanswered on purpose: a
  # made-up answer would be worse evidence than a recorded refusal, and the
  # steps that use GraphQL are written to carry on without it.
  "api user"|"api /user")
    emit '{"login":"qa-owner","id":1,"type":"User","name":"qa owner"}' "$@"
    exit 0 ;;
  "api rate_limit"|"api /rate_limit")
    emit '{"resources":{"core":{"limit":5000,"used":0,"remaining":5000,"reset":4102444800},"graphql":{"limit":5000,"used":0,"remaining":5000,"reset":4102444800}},"rate":{"limit":5000,"used":0,"remaining":5000,"reset":4102444800}}' "$@"
    exit 0 ;;
  "pr list")
    if [ -e "$state/pr-created" ]; then emit "[$(pr_json)]" "$@"; else emit '[]' "$@"; fi
    exit 0 ;;
  "pr create")
    drain_body "$@"
    printf '%s\n' "$*" >> "$root/gh-create.argv"
    # The local head and the branch, as they are in the folder gh was called
    # in: that folder is the run's own work tree, so this is the run's work
    # without the scenario knowing anything about the run's layout.
    printf '%s\n' "$(pwd -P)" >> "$root/gh-create.cwd"
    # Not guarded: a `pr create` from a folder with no committed head is not a
    # `pr create` the product could have meant, and swallowing it here would
    # surface later as an empty local head and point at the wrong cause.
    git rev-parse HEAD > "$root/gh-create.head" 2>/dev/null || {
      echo "gh stub: pr create in $(pwd -P), which has no committed HEAD" >&2
      exit 97
    }
    git branch --show-current > "$state/branch" 2>/dev/null || true
    # The shipped publish step writes `--head=<branch>`; `--head <branch>` is
    # the other form gh accepts. An `owner:branch` value keeps the branch.
    prev=""
    for a in "$@"; do
      case "$a" in
        --head=*) v="${a#--head=}"; printf '%s\n' "${v##*:}" > "$state/branch" ;;
      esac
      if [ "$prev" = "--head" ]; then printf '%s\n' "${a##*:}" > "$state/branch"; fi
      prev="$a"
    done
    : > "$state/pr-created"
    echo "https://github.com/qa-owner/qa-repo/pull/42"
    exit 0 ;;
  "pr view")
    if [ ! -e "$state/pr-created" ]; then echo "no pull requests found" >&2; exit 1; fi
    emit "$(pr_json)" "$@"
    exit 0 ;;
  "pr checks")
    if [ "$(cat "$state/checks" 2>/dev/null)" = SUCCESS ]; then
      if want_json "$@"; then emit '[{"name":"ci","state":"SUCCESS","bucket":"pass"}]' "$@"
      else printf 'ci\tpass\t1s\thttps://example.invalid/ci\n'; fi
      exit 0
    fi
    if want_json "$@"; then emit '[{"name":"ci","state":"PENDING","bucket":"pending"}]' "$@"
    else printf 'ci\tpending\t0\thttps://example.invalid/ci\n'; fi
    exit 8 ;;
  "pr merge")
    drain_body "$@"
    printf '%s\n' "$*" >> "$root/gh-merge.argv"
    : > "$state/merged"
    exit 0 ;;
  "pr ready"|"pr edit"|"pr comment") drain_body "$@"; exit 0 ;;
esac
printf '%s\n' "$*" >> "$root/gh-unexpected"
echo "gh stub: unexpected call: $*" >&2
exit 96
GH_STUB

cat > "$BINDIR/claude" <<'CLAUDE_STUB'
#!/bin/bash
set -u
root="${QA_SCENARIO_ROOT:?}"
if [ "${1-}" = "--version" ]; then echo "2.0.0 (Claude Code)"; exit 0; fi
input="$(cat)"
first="${input%%$'\n'*}"   # the prompt's first line
hash="$(printf '%s' "$input" | sha256sum)"
hash="${hash%% *}"         # sha256sum prints the digest, two spaces, then the name

# The first call leaves one repository copy with a file nobody committed, in
# this process's own TMPDIR, for part 3.
marker="$root/stub-clone-${QA_RUN_LABEL:-run}"
if [ ! -e "$marker" ]; then
  clone="${TMPDIR:-/tmp}/stub-work"
  if git clone -q "$root/origin.git" "$clone" 2>/dev/null; then
    printf 'not committed\n' > "$clone/unsaved.txt"
    printf '%s\n' "$clone" > "$marker"
  fi
fi

# In a work tree on a branch other than main, there is a change to commit.
branch="$(git branch --show-current 2>/dev/null)"
if [ -n "$branch" ] && [ "$branch" != main ] && [ ! -e feature.txt ]; then
  printf 'qa feature\n' > feature.txt
fi

# One line, one JSON object, and nothing after it: a step that declares
# parse_json is documented as parsing its agent's whole stdout as exactly one
# object, so a sentence printed beside it would fail every such step. The
# sentence a step without parse_json would read is a field of the object
# instead, so one answer serves both kinds.
prose='Done. The work is complete and the pull request is open.'
response="$(jq -nc --arg m "$prose" \
  '{workflow:"default-workflow",selected_workflow:"default-workflow",
    task_type:"Development",workstream_count:1,status:"ok",
    verdict:"APPROVED",approved:true,
    terminal_state:"EXISTING_OPEN_PR",terminal_success:true,action_taken:"none",
    pr_number:"42",pr_url:"https://github.com/qa-owner/qa-repo/pull/42",
    reason:"the pull request is open at the pushed head",
    next_action:"See pull request #42",
    message:$m,summary:$m,narrative:$m}')"
jq -nc --arg argv "$*" --arg pwd "$(pwd -P)" --arg home "${HOME-}" \
  --arg cfg "${CLAUDE_CONFIG_DIR-}" --arg hash "$hash" --arg first "$first" \
  --arg resp "$response" \
  '{argv:$argv,pwd:$pwd,home:$home,claude_config_dir:$cfg,stdin_sha256:$hash,first_line:$first,response:$resp}' \
  >> "$root/agent-calls.jsonl"
printf '%s\n' "$response"
exit 0
CLAUDE_STUB

cat > "$BINDIR/codex" <<'CODEX_STUB'
#!/bin/bash
set -u
root="${QA_SCENARIO_ROOT:?}"
printf '%s\n' "$*" >> "$root/codex-calls"
case "${1-}" in
  --version) echo "codex-cli 0.0.0"; exit 0 ;;
  plugin) exit 0 ;;
esac
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
}

TASK="Add a file named feature.txt at the repository root containing one line of text."
BRANCH=""

# --- E: part 1, the whole build ---------------------------------------------
printf 'PENDING\n' > "$WORK/gh-state/checks"
scenario_env build "$BINDIR/amplihack" recipe run smart-orchestrator \
  -w "$WORK/build-repo" --no-detach \
  -c "task_description=$TASK" -c "repo_path=$WORK/build-repo" \
  -c workflow=default-workflow -c force_single_workstream=true -c base_branch=main \
  </dev/null >"$WORK/build.out" 2>"$WORK/build.err"
build_rc=$?
echo "build exit status: $build_rc"

# Branch and head, as the stub recorded them when the pull request was asked
# for: the branch of `--head`, in either of the two forms gh accepts, and
# otherwise the branch the folder gh was called in was on. The stub writes
# both as it goes, so gh's arguments are parsed in one place and not two; the
# argv file is read here only for whether a `pr create` was recorded at all.
if [ -s "$WORK/gh-create.argv" ]; then
  BRANCH="$(cat "$WORK/gh-state/branch" 2>/dev/null)"
fi
# Whole-string, and no leading dash: the value reaches `git fetch` and
# `git worktree add` as an argument, and `grep` would have matched one line of
# a multi-line value and accepted a value Git would read as an option.
[[ $BRANCH =~ ^[A-Za-z0-9._/][A-Za-z0-9._/-]*$ ]] || BRANCH=""
LOCAL_HEAD="$(cat "$WORK/gh-create.head" 2>/dev/null)"
ORIGIN_HEAD=""
[ -z "$BRANCH" ] || ORIGIN_HEAD="$(gitq --git-dir="$ORIGIN" rev-parse "refs/heads/$BRANCH" 2>/dev/null)"

if [ -z "$BRANCH" ]; then
  flunk "part1_build_opens_pr_and_pushes_branch: the stub recorded no pr create (build exit $build_rc)"
elif [ -z "$LOCAL_HEAD" ] || [ "$LOCAL_HEAD" != "$ORIGIN_HEAD" ]; then
  flunk "part1_build_opens_pr_and_pushes_branch: pr create for '$BRANCH' from '$(tail -n 1 "$WORK/gh-create.cwd" 2>/dev/null)' but the local head '$LOCAL_HEAD' is not the origin's '$ORIGIN_HEAD'"
else
  pass "part1_build_opens_pr_and_pushes_branch - $BRANCH at $ORIGIN_HEAD"
fi

# Finalisation reports its decision as a field of the result object it prints,
# so the state is read from the reported field and not from the word appearing
# anywhere in the run's output: the recipes' own prompts list every state by
# name, and a run that echoed one would otherwise read as having reached it.
reported_state() { grep -hoE '"terminal_state" *: *"[A-Z_]+"' "$@" | sed 's/.*"\([A-Z_]*\)"$/\1/'; }
FINAL_STATES="$(reported_state "$WORK/build.out" "$WORK/build.err" | sort -u | tr '\n' ' ')"

if printf '%s' "$FINAL_STATES" | grep -qw FAILED_IMPLEMENTATION; then
  flunk "part1_finalization_is_not_failed_implementation: the build reported FAILED_IMPLEMENTATION with the pull request's checks pending (states reported: $FINAL_STATES)"
elif [ ! -s "$WORK/gh-create.argv" ]; then
  flunk "part1_finalization_is_not_failed_implementation: no pull request was opened, so finalisation had nothing to report on"
elif ! grep -qE '"terminal_state" *: *"EXISTING_OPEN_PR"|"pr_number" *: *"?42"?|/pull/42' \
    "$WORK/build.out" "$WORK/build.err"; then
  flunk "part1_finalization_is_not_failed_implementation: the run's output does not name the open pull request (build exit $build_rc, states reported: ${FINAL_STATES:-none})"
else
  pass "part1_finalization_is_not_failed_implementation - ${FINAL_STATES:-no state field printed}"
fi

# --- F: part 2, the whole drive ---------------------------------------------
printf 'SUCCESS\n' > "$WORK/gh-state/checks"
drive_started=0
if [ -n "$BRANCH" ] && make_clone "$WORK/pr-clone" \
    && gitq -C "$WORK/pr-clone" fetch -q origin "$BRANCH" \
    && gitq -C "$WORK/pr-clone" worktree add -q -B "$BRANCH" "$WORK/pr-worktree" "origin/$BRANCH"; then
  drive_started=1
  scenario_env drive "$BINDIR/amplihack" recipe run auto-drive-to-merge \
    -w "$WORK/pr-worktree" --no-detach \
    -c pr_number=42 -c "repo_path=$WORK/pr-worktree" -c base_branch=main \
    -c autodrive_dry_run=false -c no_merge=false \
    -c "autodrive_state_dir=$WORK/home/autodrive-state" \
    </dev/null >"$WORK/drive.out" 2>"$WORK/drive.err"
  echo "drive exit status: $?"
fi

# The merge the stub was asked for, as `gh pr merge <number> ...`.
merge_recorded=0
awk '$1 == "pr" && $2 == "merge" && $3 == "42" { found = 1 } END { exit !found }' \
  "$WORK/gh-merge.argv" && merge_recorded=1
# The drive's verdict: the last step prints one typed report object carrying
# `stop_reason`, which is the drive's own word for how it ended. The wording
# of the sentence beside it is the product's and is not read here.
drive_verdict="$(grep -hoE '"stop_reason" *: *"[a-z_]+"' "$WORK/drive.out" "$WORK/drive.err" | tail -n 1)"

if [ "$drive_started" -ne 1 ]; then
  flunk "part2_drive_ends_at_verdict_and_merges: there was no branch to drive (the per-PR clone or worktree could not be made)"
elif [ "$merge_recorded" -ne 1 ]; then
  flunk "part2_drive_ends_at_verdict_and_merges: the stub recorded no pr merge for pull request 42"
elif [ -z "$drive_verdict" ]; then
  flunk "part2_drive_ends_at_verdict_and_merges: the merge reached the stub but the drive printed no terminal report naming how it ended"
else
  pass "part2_drive_ends_at_verdict_and_merges - $drive_verdict"
fi

# --- G: part 3, what survived -----------------------------------------------
# The build's commit must still be reachable on the branch. A drive is allowed
# to add to that branch, so the test is that the build's head is still an
# ancestor of the branch's tip, not that the tip has not moved.
BRANCH_TIP=""
[ -z "$BRANCH" ] || BRANCH_TIP="$(gitq --git-dir="$ORIGIN" rev-parse "refs/heads/$BRANCH" 2>/dev/null)"
if [ -z "$BRANCH" ] || [ "$drive_started" -ne 1 ]; then
  flunk "part3_committed_work_survives: there was no run branch to inspect"
elif [ -z "$BRANCH_TIP" ]; then
  flunk "part3_committed_work_survives: the branch '$BRANCH' is no longer in the origin"
elif ! gitq --git-dir="$ORIGIN" merge-base --is-ancestor "$ORIGIN_HEAD" "$BRANCH_TIP" 2>/dev/null; then
  flunk "part3_committed_work_survives: the build's commit '$ORIGIN_HEAD' is no longer on '$BRANCH' (now at '$BRANCH_TIP')"
elif ! gitq --git-dir="$ORIGIN" cat-file -e "$BRANCH_TIP:feature.txt" 2>/dev/null; then
  flunk "part3_committed_work_survives: the committed work (feature.txt) is not on '$BRANCH' in the origin"
elif [ ! -f "$WORK/pr-worktree/feature.txt" ]; then
  flunk "part3_committed_work_survives: the committed work is not readable in the per-PR worktree"
else
  pass "part3_committed_work_survives - $BRANCH at $BRANCH_TIP"
fi

count_exact() { WANT="$2" awk '$0 == ENVIRON["WANT"] { n++ } END { print n + 0 }' "$1"; }

# Every `amplihack: leaving ` line in $1 that is about the repository copy at
# $2, whether the folder judged was that copy or a folder holding it. A whole
# build can keep more than one folder; the other folders' lines are not this
# case's business and are not read.
lines_about() {
  CLONE="$2" awk '
    index($0, "amplihack: leaving ") != 1 { next }
    index($0, "work tree \"" ENVIRON["CLONE"] "\": ") > 0 { print; next }
    index($0, "amplihack: leaving \"" ENVIRON["CLONE"] "\" in place: ") == 1 { print }
  ' "$1"
}

kept_problems=""
for label in build drive; do
  marker="$WORK/stub-clone-$label"
  err="$WORK/$label.err"
  if [ ! -s "$marker" ]; then
    kept_problems="$kept_problems $label(no agent step ran, so nothing was left unsaved)"
    continue
  fi
  clone="$(cat "$marker")"
  found="$WORK/leaving-$label"
  lines_about "$err" "$clone" > "$found"
  n=$(( $(wc -l < "$found") ))
  line="$(head -n 1 "$found")"
  folder="$(printf '%s\n' "$line" | sed -n 's/^amplihack: leaving "\([^"]*\)" in place: .*$/\1/p')"
  if [ "$n" -lt 1 ]; then
    kept_problems="$kept_problems $label(no 'amplihack: leaving' line names $clone)"
  elif [ "$n" -gt 2 ] || [ "$(count_exact "$found" "$line")" != "$n" ]; then
    kept_problems="$kept_problems $label(expected 1 or 2 identical lines for $clone, found $n: $(tr '\n' '|' < "$found"))"
  elif ! printf '%s\n' "$line" | grep -q 'has uncommitted changes$'; then
    kept_problems="$kept_problems $label(the line does not say why $clone was kept: $line)"
  elif [ -z "$folder" ] || [ ! -d "$folder" ]; then
    kept_problems="$kept_problems $label(the folder the line named is gone: ${folder:-no folder read from the line} / $line)"
  elif [ "$(cat "$clone/unsaved.txt" 2>/dev/null)" != "not committed" ]; then
    kept_problems="$kept_problems $label(the file nobody committed did not survive in $clone)"
  fi
done
if [ -n "$kept_problems" ]; then
  flunk "part3_unsaved_work_folder_kept_with_reason:$kept_problems"
else
  pass "part3_unsaved_work_folder_kept_with_reason"
fi

# --- H: the descriptor states this scenario in full -------------------------
DESCRIPTOR="$REPO_ROOT/tests/gadugi/scenarios/issue-2806-end-to-end-build-and-drive.yaml"
DESCRIPTOR_CASE="every_case_this_run_emitted_is_asserted_by_the_descriptor"
printf '%s\n' "$DESCRIPTOR_CASE" >> "$CASE_NAMES"
if [ ! -f "$DESCRIPTOR" ]; then
  flunk "the scenario descriptor is missing: $DESCRIPTOR"
else
  undescribed=""
  while IFS= read -r case_name; do
    [ -n "$case_name" ] || continue
    grep -qF -- "PASS: $case_name" "$DESCRIPTOR" || undescribed="$undescribed $case_name"
  done < <(sort -u -- "$CASE_NAMES")
  if [ -n "$undescribed" ]; then
    flunk "the descriptor does not assert:$undescribed"
  else
    pass "$DESCRIPTOR_CASE"
  fi
fi

echo "cases passed: $passed"
[ "$fail" -eq 0 ] || exit 1
echo "ALL_CASES_PASSED"
