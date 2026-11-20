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
#   claude  answers the prompt. In part 1 it writes one fixed answer whatever
#           step asked: one line holding one JSON object, the union of the keys
#           the steps on that path parse, with values that steer one legitimate
#           route, and the sentence a step without parse_json would read as a
#           field of that object rather than beside it, because a step that
#           declares parse_json parses the whole of its agent's stdout as
#           exactly one object.
#           In part 2 one answer cannot serve every step: the three preflights
#           and the two readers need shapes that contradict each other, and the
#           first line does not separate them, since four shipped steps open
#           with the same pager sentence. So the stand-in carries a dispatch
#           table, one entry per agent step of the drive, keyed on a substring
#           of the prompt that occurs in exactly one shipped prompt (or on the
#           first line where that alone is unambiguous). Each entry says which
#           step it answers and why that answer is what a competent agent
#           would report of this fixture, and the facts it rests on are read
#           rather than asserted: the head, branch and base come from the PR
#           worktree, the private state directory is the one the driver gave
#           the drive, and where a step asks for an action rather than a
#           report (post the Crusty verdict, write the completion decision,
#           save the measured qa record) the stand-in performs it and then
#           reports what it did. A prompt matching no entry keeps part 1's
#           fixed answer and is recorded in $WORK/agent-unmatched.jsonl, which
#           a case of its own reads: part 1's prompts are all there by design,
#           and a part 2 prompt there is a shipped prompt reworded out from
#           under its discriminator.
#           In a work tree inside the scenario's root it writes the
#           task's file, and nowhere else: a step may call the agent from the
#           repository the product was pointed at, and a stand-in that wrote
#           there would be the scenario failing its own last case. Its first
#           call also leaves one repository copy with an uncommitted file in
#           its own TMPDIR, which part 3 looks for. Every
#           call is recorded in $WORK/agent-calls.jsonl (argv, working
#           directory, HOME, a hash of the prompt, its first line, the entry
#           that answered it, the answer).
#   codex   answers --version and plugin, records every call in
#           $WORK/codex-calls, exits 47 otherwise. It is on PATH only so a
#           probe cannot reach a host codex.
#
# Part 1  the whole build. Checks are PENDING. The run ends with a pull request
#         opened on the stub for the run's branch and that branch pushed to the
#         bare origin at the local head, and finalisation reports a terminal
#         state that is not FAILED_IMPLEMENTATION and names the open pull
#         request (issue 2343: pending checks hand the pull request on).
#         Finalisation must have run, and not merely have been spoken for: the
#         case requires the hosting call
#         amplifier-bundle/tools/workflow_final_status.sh makes just after it
#         emits its terminal evidence, recorded by the gh stand-in. That
#         helper is shell and not an agent, and the claude stand-in cannot
#         write gh-calls, so this case cannot be satisfied by an agent's own
#         answer. The helper's bare `key=value` evidence lines would be the
#         obvious anchor and are not readable here: the recipe runner captures
#         a bash step's output at every depth, so nothing a bash step prints
#         reaches this script.
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
#         stub. A second case covers the stand-in's own reach: every agent
#         step the drive ran must have matched an entry of the dispatch table,
#         so a reworded shipped prompt fails the scenario by name rather than
#         silently changing what part 2 exercises.
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
#         Last, AMPLIHACK_HOME points the product at this checkout, so the
#         checkout's `git status --porcelain` listing is taken before the build
#         and compared after the drive: neither run may have added an entry.
#         Entries that were there already are not this case's business.
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
# RECIPE_RUNNER_RS_PATH (or put recipe-runner-rs on PATH) for the runner. Set
# QA_KEEP_WORK=1 to keep the temporary root afterwards for a post-mortem.
# On success it prints ALL_CASES_PASSED and exits 0; otherwise SCENARIO_FAILED
# and exit 1, with each evidence file dumped under its own header.
# No `-e` on purpose: every case must run and report, so an early exit would
# hide the failures after the first one. Setup is guarded with `|| die`.
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
  gh-create.argv gh-create.cwd gh-merge.argv agent-calls.jsonl
  agent-unmatched.jsonl codex-calls)

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
    "${HOST_TMP%/}"/*)
      # QA_KEEP_WORK keeps the root for a post-mortem by hand. The failure
      # dump above prints the evidence files, but not the two run folders,
      # the clones or the bare origin, which is where a failure inside a
      # recipe run has to be read. Unset in CI, so CI still cleans up.
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

# The drive's private auto-drive state directory, named once. The driver passes
# it to the drive as the operator's `autodrive_state_dir` and to the stand-in
# through the environment, so the two cannot name different directories:
# autodrive-finalize asks the authorization helper to confirm that the
# directory finalisation was given is the one the runner bound to this
# invocation, and any other path ends the drive in a hand-off. The runner
# creates it; the per-PR worktree is deliberately not its parent.
AUTODRIVE_STATE="$WORK/home/autodrive-state"
# The checkout the drive is pointed at, and the only repository the stand-in
# reads the head, branch and base from.
DRIVE_REPO="$WORK/pr-worktree"

# Tools a step may call. The first group is required; the rest are linked when
# the host has them. The list serves the product, not this driver: `sleep` and
# `timeout` are here because the shipped steps running inside this PATH may
# call them, and this script never does. Dropping a tool the driver happens
# not to use trades a tidier list for a failure deep inside a recipe run,
# which is far harder to read than the untidiness it cures.
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
    QA_AUTODRIVE_STATE_DIR="$AUTODRIVE_STATE" QA_DRIVE_REPO="$DRIVE_REPO" \
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
# The version the product gates on: below 2.1.319 it refuses to launch,
# because that is the first Claude Code that accepts the operating contract
# through `--append-system-prompt-file`. A stand-in naming an older one would
# stop the build at its first agent step and test the gate, not the build.
if [ "${1-}" = "--version" ]; then echo "2.1.319 (Claude Code)"; exit 0; fi
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

# In a work tree on a branch other than main, there is a change to commit, at
# the repository root the task names. Only in a work tree inside the scenario's
# own root: a step may call the agent from the repository the product was
# pointed at, which is this checkout, and a stand-in that wrote a file there
# would be the scenario writing into the repository under test. Part 3 reads
# that as the product's doing and would report a fault that is the scenario's.
top="$(git rev-parse --show-toplevel 2>/dev/null)"
[ -z "$top" ] || top="$(cd "$top" 2>/dev/null && pwd -P)"
case "$top" in
  "$root"/*) ;;
  *) top="" ;;
esac
branch="$(git branch --show-current 2>/dev/null)"
if [ -n "$top" ] && [ -n "$branch" ] && [ "$branch" != main ] && [ ! -e "$top/feature.txt" ]; then
  printf 'qa feature\n' > "$top/feature.txt"
fi

# The fallback answer, and part 1's only answer. One line, one JSON object,
# and nothing after it: a step that declares parse_json is documented as
# parsing its agent's whole stdout as exactly one object, so a sentence
# printed beside it would fail every such step. The sentence a step without
# parse_json would read is a field of the object instead, so one answer serves
# both kinds.
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
step=fallback

# --- the drive's dispatch ----------------------------------------------------
# Part 2 only. Part 1 never enters this block, so the build keeps the single
# fixed answer above and its cases are unaffected by anything below.
#
# One entry per agent step the drive runs, each naming the step it answers and
# why that answer is the one a competent agent would give here. The first line
# alone does not separate them: four steps open with the same pager sentence.
# Where a first line is the whole discriminator it is matched as a prefix;
# otherwise a substring of the full prompt is, chosen so that it occurs in
# exactly one shipped prompt.
#
# The facts the answers rest on are read, not asserted: the head, branch and
# base come from the PR worktree, and the private state directory is the one
# the driver passed to the drive, so the stand-in and the operator cannot name
# different directories.
has() { case "$input" in *"$1"*) return 0 ;; esac; return 1; }
opens() { case "$first" in "$1"*) return 0 ;; esac; return 1; }
if [ "${QA_RUN_LABEL:-}" = drive ]; then
  REPO="${QA_DRIVE_REPO:-}"
  STATE="${QA_AUTODRIVE_STATE_DIR:-}"
  PR=42
  PR_URL="https://github.com/qa-owner/qa-repo/pull/42"
  drive_git() { git -C "$REPO" "$@" 2>/dev/null; }
  HEAD_SHA="$(drive_git rev-parse HEAD)"
  BRANCH="$(drive_git branch --show-current)"
  BASE_SHA="$(drive_git rev-parse refs/remotes/origin/main)"
  [ -n "$BASE_SHA" ] || BASE_SHA="$HEAD_SHA"
  j() { jq -nc --arg u "$PR_URL" --arg b "$BRANCH" --arg h "$HEAD_SHA" \
          --arg bs "$BASE_SHA" --arg d "$STATE" --arg r "$REPO" "$@"; }

  # First, because this one prompt quotes a round's own output back at the
  # agent: a later entry's discriminator could appear inside the quotation.
  if has '# Loop-health evaluation'; then
    # loop-health-evaluator/step-02-evaluate-loop-health, the merge-ready
    # loop's terminator. The prompt states the round's own verdict and the
    # loop's clean token, in backquotes; DONE is legal only when they are the
    # same word, and anything else stops the loop rather than spending another
    # round on a judgement this stand-in cannot make.
    step=loop-health-evaluator/step-02-evaluate-loop-health
    tail_of="${input#*own verdict \`}"; round_verdict="${tail_of%%\`*}"
    tail_of="${tail_of#*equals \`}"; clean_token="${tail_of%%\`*}"
    if [ -n "$round_verdict" ] && [ "$round_verdict" = "$clean_token" ]; then
      response="$(j --arg v "$round_verdict" '{loop_verdict:"DONE",not_converging:[],
        moved:["the round reached its clean verdict"],
        recommended_action:("the round verdict is " + $v + "; advance to the next phase")}')"
    else
      response="$(j --arg v "$round_verdict" --arg t "$clean_token" \
        '{loop_verdict:"STUCK",
          not_converging:[("the round verdict \"" + $v + "\" is not the clean token \"" + $t + "\"")],
          moved:[],recommended_action:"stop and read the round record"}')"
    fi

  elif has 'Determine QA prerequisites using the trusted qa-team'; then
    # auto-drive-to-merge/autodrive-prerequisites. The sandbox's PATH holds
    # git, gh and jq, and the fixture is a Rust-shaped repository, where
    # qa-team's own table does not require gadugi.
    step=autodrive-prerequisites
    response="$(j '{prerequisites:["git","gh","jq"],gadugi_required:false,blockers:[],attempts:[]}')"

  elif has 'step-02 decides from your facts'; then
    # autodrive-build/step-01-build-preflight. The stub reports PR 42 OPEN at
    # the branch the build pushed, and the drive runs in that branch's linked
    # worktree, so the observed facts are a found, open PR with a verified
    # worktree. step-02 then skips default-workflow, which is correct: the
    # build already happened in part 1.
    step=autodrive-build/step-01-build-preflight
    response="$(j '{pr_found:"true",pr:"42",pr_url:$u,pr_state:"OPEN",branch:$b,
      head_sha:$h,base_branch:"main",base_sha:$bs,state_dir:$d,repo:$r,
      reason:"PR 42 is OPEN and its head is the head of this linked worktree",
      blockers:[],attempts:[]}')"

  elif has 'Return task_description and requirements from the inputs below'; then
    # autodrive-build/step-01b-no-merge-directive. It only appends the
    # directive to the inputs; nothing downstream reads it once step-02 is
    # skipped.
    step=autodrive-build/step-01b-no-merge-directive
    response="$(j '{task_description:"Drive pull request 42 to merge. [amplihack:no-merge] Build and publish the PR and leave it open.",
      requirements:"[amplihack:no-merge] Build and publish the PR and leave it open. No merge is authorized in default-workflow."}')"

  elif opens 'Establish the PR identity and the worktree before continuing.'; then
    # autodrive-build/step-03-resolve-pr. The first line is the whole
    # discriminator: the three preflights carry that sentence too, but each of
    # them opens with the pager line instead. auto-drive-to-merge hands
    # build_result.pr and build_result.state_dir to the crusty and merge
    # loops, so these are the values the rest of the drive works from. The PR
    # was open when this run started and this run has not merged it, so
    # merged_before_run is false.
    step=autodrive-build/step-03-resolve-pr
    response="$(j '{should_run:"true",pr:"42",pr_url:$u,pr_state:"OPEN",branch:$b,
      head_sha:$h,base_branch:"main",base_sha:$bs,state_dir:$d,repo:$r,
      merged_before_run:false,terminal_state:"EXISTING_OPEN_PR",handed_off:"false",
      entry_evidence:"PR 42 read OPEN at this worktree head at run entry",
      reason:"the PR phase 1 opened is open at this worktree head",
      blockers:[],attempts:[]}')"

  elif opens 'Observe the exact PR from'; then
    # auto-drive-to-merge/autodrive-check-unmerged, after the crusty phase.
    step=auto-drive-to-merge/autodrive-check-unmerged
    response="$(j '{platform_state:"OPEN",action:"confirmed_open",head_sha:$h,
      blockers:[],attempts:[]}')"

  elif has 'complete-at-head'; then
    # autodrive-crusty-loop/step-01-crusty-loop-preflight. No Crusty round has
    # ever run for this PR, so the state folder holds no crusty record and
    # `complete-at-head` would be untrue: the loop has work to do.
    step=autodrive-crusty-loop/step-01-crusty-loop-preflight
    response="$(j '{should_run:"true",skip_reason:"",pr:"42",pr_url:$u,pr_state:"OPEN",
      branch:$b,head_sha:$h,base_branch:"main",base_sha:$bs,state_dir:$d,repo:$r,
      reason:"PR 42 is OPEN, the worktree is at its head, and no Crusty round exists at that head",
      blockers:[],attempts:[]}')"

  elif has '"resolved_concerns":"stable IDs"'; then
    # autodrive-crusty-round/step-01-round-context.
    step=autodrive-crusty-round/step-01-round-context
    response="$(j '{pr:"42",head_sha:$h,resolved_concerns:"",round_label:"round",
      blockers:[],attempts:[]}')"

  elif has 'crusty-old-engineer'; then
    # autodrive-crusty-round/step-02-crusty-review. The branch adds the one
    # text file the task asked for and changes nothing else, so a review of it
    # raises no concern. step-03 reads crusty_verdict out of this answer.
    step=autodrive-crusty-round/step-02-crusty-review
    response="$(j '{crusty_verdict:"CLEAN",concerns:[],reviewed_head_sha:$h,
      summary:"one added text file, nothing else changed; no concerns"}')"

  elif opens 'Assess what the reviewer changed in this PR worktree'; then
    # step-02b-assess-review-changes, shared word for word by the crusty round
    # and the merge round. The reviewer is this same stand-in, which writes
    # nothing in the PR worktree, so there is nothing to report or restore.
    step=step-02b-assess-review-changes
    response='The PR worktree is as the reviewer found it: git status --porcelain is empty, the index is unchanged, HEAD and the branch are unmoved, and there are no untracked files. Nothing needed restoring.'

  elif has "Post this Crusty round's verdict on PR"; then
    # autodrive-crusty-round/step-03c-post-verdict. The stand-in really posts
    # the comment through gh, as the step asks, and then reports what it did;
    # autodrive_crusty_posted.sh records the posted line from this answer, and
    # a claim with no comment behind it would be a false one.
    step=autodrive-crusty-round/step-03c-post-verdict
    posted=false
    if printf 'Crusty verdict: CLEAN at %s\n\nNo concerns were raised in this round.\n' "$HEAD_SHA" \
        | gh pr comment "$PR" --body-file - >/dev/null 2>&1; then
      posted=true
    fi
    response="$(j --arg p "$posted" '{verdict_posted:$p,head_sha:$h}')"

  elif has 'autodrive-completion-write'; then
    # autodrive-crusty-round/step-07-select-next-action. The action is read
    # from the round record the step names rather than asserted, and it is
    # written only through the Rust writer, as the step requires.
    step=autodrive-crusty-round/step-07-select-next-action
    tail_of="${input#*Read the round record at }"; record="${tail_of%% *}"
    tail_of="${input#*--private-state \"}"; private_state="${tail_of%%\"*}"
    action=evaluate
    why="the round record is not a clean round at the head with no findings"
    if [ -n "$record" ] && [ -f "$record" ]; then
      verdict="$(jq -r '.crusty_verdict // ""' "$record" 2>/dev/null)"
      reviewed="$(jq -r '.reviewed_head_sha // ""' "$record" 2>/dev/null)"
      at="$(jq -r '.head_sha // ""' "$record" 2>/dev/null)"
      commits="$(jq -r '.commits_this_round // 1' "$record" 2>/dev/null)"
      findings=1
      [ ! -f "${record}.findings" ] || findings="$(wc -c < "${record}.findings" | tr -d ' ')"
      case "$verdict" in
        CLEAN|MINOR_ONLY)
          if [ "$reviewed" = "$at" ] && [ "$commits" = 0 ] && [ "$findings" = 0 ]; then
            action=complete
            why="the round record is ${verdict} at ${at}, with no findings and no commit this round"
          fi ;;
      esac
    fi
    written="no private state was named in the prompt"
    [ -z "$private_state" ] || written="$(amplihack orch helper autodrive-completion-write \
      --private-state "$private_state" --action "$action" --reason "$why" 2>&1)"
    response="Action ${action}: ${why}. Writer: ${written}"

  elif has 'autodrive_record_crusty_loop_done'; then
    # autodrive-crusty-loop/step-03-record-crusty-phase. The loop ended DONE
    # on a CLEAN round with no minor finding, so the phase is recorded, and
    # only through the trusted function in the launch bundle.
    step=autodrive-crusty-loop/step-03-record-crusty-phase
    recorded=false
    tools="${AMPLIHACK_HOME:-}/amplifier-bundle/tools/autodrive_state.sh"
    if [ -n "$STATE" ] && [ -f "$tools" ] \
      && ( . "$tools" && autodrive_record_crusty_loop_done "$STATE" ) >/dev/null 2>&1; then
      recorded=true
    fi
    response="$(j --arg rec "$recorded" '{recorded:$rec,reviewed_head_sha:$h,
      followups:[],blockers:[],attempts:[]}')"

  elif has 'should_run=true only for an observed OPEN PR'; then
    # autodrive-merge-loop/step-01-merge-loop-preflight. autodrive-build
    # step-03 carries the same sentence and is matched above by its first
    # line, and the crusty preflight's version names skip_reason instead.
    # state_dir must be exactly the directory the runner bound to this
    # invocation: autodrive-finalize hands it to the authorization helper,
    # which refuses any other path and hands the PR off instead of merging.
    step=autodrive-merge-loop/step-01-merge-loop-preflight
    response="$(j '{should_run:"true",pr:"42",pr_url:$u,pr_state:"OPEN",branch:$b,
      head_sha:$h,base_branch:"main",base_sha:$bs,state_dir:$d,repo:$r,
      reason:"PR 42 is OPEN, the linked worktree is at its head, and the private state directory is readable",
      blockers:[],attempts:[]}')"

  elif has 'Return JSON with base, synced'; then
    # autodrive-merge-evidence/step-01-sync-with-base. The branch was cut from
    # main and main has not moved since, so it already contains its base.
    step=autodrive-merge-evidence/step-01-sync-with-base
    response="$(j '{base:"main",synced:"true",conflict:"false",head_sha:$h,
      git_observed:"the branch already contains origin/main; there was nothing to merge",
      blockers:[],attempts:[]}')"

  elif has 'Execute criterion 1 against the current observed PR head'; then
    # autodrive-merge-evidence/step-02-qa-team-scenarios. The fixture is a
    # Rust-shaped repository, where qa-team's table does not require gadugi,
    # and it declares no suite the sandbox can run, so the record says so.
    # The measured record is a file the next step hashes, so it is written.
    step=autodrive-merge-evidence/step-02-qa-team-scenarios
    tail_of="${input#*Save the actual measured qa-evidence.json to }"
    evidence_file="${tail_of%% *}"
    qa="$(j '{qa_status:"PASS",gadugi_required:false,gadugi_status:"NOT_REQUIRED",
      qa_criterion1_declaration:"none",
      qa_reason:"the repository declares no suite command at the merge base and the change is one text file",
      qa_command_paths:["feature.txt"],head_sha:$h,blockers:[],attempts:[]}')"
    case "$evidence_file" in
      /*) ( umask 077; printf '%s\n' "$qa" > "$evidence_file" ) || true ;;
    esac
    response="$qa"

  elif has 'Return JSON with ci_status GREEN'; then
    # autodrive-merge-evidence/step-03-wait-for-ci. The stub reports the one
    # check SUCCESS at this head for the whole of part 2.
    step=autodrive-merge-evidence/step-03-wait-for-ci
    response="$(j --arg sig "pass@$HEAD_SHA" '{ci_status:"GREEN",ci_total:1,ci_pending:0,
      ci_failing:0,ci_signal:$sig,head_sha:$h,blockers:[],attempts:[]}')"

  elif has 'Collect complete platform observations'; then
    # autodrive-merge-round/step-01-platform-facts, reporting what the stub's
    # own pr view says: open, not a draft, mergeable, approved, one green
    # check, no conversation thread of any kind.
    step=autodrive-merge-round/step-01-platform-facts
    response="$(j '{pr:"42",pr_url:$u,state:"OPEN",is_draft:false,branch:$b,head_sha:$h,
      base_branch:"main",base_sha:$bs,merge_base:$bs,mergeable:"MERGEABLE",
      merge_state:"CLEAN",review_decision:"APPROVED",approval_status:"MET",
      approval_source:"the review decision on the PR itself",approval_blocker:"",
      required_approvals:"",changes_requested:0,unresolved_threads:0,
      checks_total:1,checks_pending:0,checks_failing:0,
      protections:"no classic branch protection and no ruleset on main",
      blockers:[],attempts:[]}')"

  elif has 'Judge criterion 3 for PR'; then
    # autodrive-merge-round/step-01b-crusty-evidence. The Crusty round posted
    # its verdict comment on the PR at this head earlier in this same drive.
    step=autodrive-merge-round/step-01b-crusty-evidence
    response="$(j '{crusty_status:"DONE_CLEAN",
      crusty_reason:"the newest Crusty verdict comment on the PR reads CLEAN at the current head",
      crusty_reviewed_head_sha:$h,crusty_first_unreviewed_sha:""}')"

  elif has '# merge-ready assessment'; then
    # autodrive-merge-round/step-02-merge-ready-assessment. The assessment is
    # posted as a comment, as the step asks, and its closing sentence states
    # the outcome. Nothing parses this text; step-03 reads it and answers in
    # one word.
    step=autodrive-merge-round/step-02-merge-ready-assessment
    response="Merge-ready assessment of PR #${PR} at ${HEAD_SHA}.
All 13 criteria are met at this head. Criterion 1: this repository declares no suite command at the merge base and qa-team requires no gadugi scenario for it, so nothing is outstanding and the measured record says so. Criterion 3: the Crusty verdict comment on the PR reads CLEAN at this head. Criterion 8 and the rest: the one CI check at this head passed, the review decision is APPROVED, the merge state is CLEAN with neither a conflict nor a branch out of date with its base, there is no unresolved conversation thread, and the PR description carries the evidence for each criterion.
This pull request is ready to merge."
    printf '%s\n' "$response" | gh pr comment "$PR" --body-file - >/dev/null 2>&1 || true

  elif opens 'Read the merge-ready assessment between the markers'; then
    # autodrive-merge-round/step-03-outcome-reader. Exactly two bare lines and
    # nothing after them: autodrive_round_word.sh lower-cases the last
    # non-empty line and strips it to [a-z-], and reads the line before it as
    # the CI answer.
    step=autodrive-merge-round/step-03-outcome-reader
    response='ci: pass
ready'

  elif has 'Reconcile every minor crusty finding'; then
    # autodrive-merge-loop/step-02b-file-followups. The Crusty round was CLEAN
    # with no finding at all, so there is nothing to file and nothing to map.
    step=autodrive-merge-loop/step-02b-file-followups
    response="$(j '{mappings:[],blockers:[],attempts:[]}')"

  elif has 'before finalisation and state, in plain words'; then
    # autodrive-finalize/observe-finalization-entry, before the merge step.
    step=autodrive-finalize/observe-finalization-entry
    response="PR #${PR} is open, is not a draft, and its head is ${HEAD_SHA}. It was open when this run started too, so nothing merged it earlier in the run."

  elif has 'State in two or three plain sentences how finalisation'; then
    # autodrive-finalize/state-outcome, after the merge step. The merge step's
    # own result is quoted into this prompt; the sentence here is prose about
    # it and decides nothing.
    step=autodrive-finalize/state-outcome
    response="Finalisation of PR #${PR} ended with the result the merge step recorded, which is quoted in this step's own input. This step requested no merge and changed nothing on GitHub."
  fi
fi

# A prompt that matched no entry keeps the fixed answer and is recorded here,
# so a shipped prompt reworded out from under a discriminator is visible
# instead of silently changing what part 2 covers. Part 1's steps are all
# here by design: the build deliberately falls back.
if [ "$step" = fallback ]; then
  jq -nc --arg run "${QA_RUN_LABEL:-}" --arg first "$first" --arg hash "$hash" \
    '{run:$run,first_line:$first,stdin_sha256:$hash}' >> "$root/agent-unmatched.jsonl"
fi

jq -nc --arg argv "$*" --arg pwd "$(pwd -P)" --arg home "${HOME-}" \
  --arg cfg "${CLAUDE_CONFIG_DIR-}" --arg hash "$hash" --arg first "$first" \
  --arg step "$step" --arg resp "$response" \
  '{argv:$argv,pwd:$pwd,home:$home,claude_config_dir:$cfg,stdin_sha256:$hash,first_line:$first,step:$step,response:$resp}' \
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
# The cases this run reached, whichever way they went. A failing case is still
# a case the script covers, so both pass and flunk record the name; the
# descriptor check in section H reads this list in both directions.
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
  # A case flunk reads "case_name: what went wrong". The few flunks that
  # belong to no case say so in words, and those have a space before the
  # colon, so an all-[A-Za-z0-9_] head is what distinguishes the two.
  local head="${1%%:*}"
  case "$head" in
    ''|*[!A-Za-z0-9_]*) ;;
    *) printf '%s\n' "$head" >> "$CASE_NAMES" ;;
  esac
}

TASK="Add a file named feature.txt at the repository root containing one line of text."
BRANCH=""

# The checkout as it stands before any run. AMPLIHACK_HOME points the product
# at this repository, so a run could write into it; the last case compares this
# against the same listing afterwards. Taken as a listing rather than as a
# clean/dirty verdict, so a checkout that was already dirty still gives a
# usable comparison.
REPO_BEFORE="$WORK/repo-status-before"
git -C "$REPO_ROOT" status --porcelain > "$REPO_BEFORE" 2>/dev/null || : > "$REPO_BEFORE"

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

# Finalisation reports its decision as a named field and not as a loose word:
# the recipes' own prompts list every state by name, and a run that echoed one
# would otherwise read as having reached it. There are two field forms, and
# which one carried the state matters, so they are read apart and reported
# apart.
#
# The agent form is a field of the JSON result object an agent step prints.
# The stand-in prints exactly that form, so on its own it says only what the
# agent answered.
agent_reported_state() {
  grep -hoE '"terminal_state" *: *"[A-Z_]+"' "$@" | sed 's/.*"\([A-Z_]*\)"$/\1/'
}
# The helper form is a bare `key=value` line at the start of a line, which is
# how amplifier-bundle/tools/workflow_final_status.sh prints its terminal
# evidence (emit_terminal_evidence). That helper is shell, not an agent. It is
# read for the report only, and is normally empty: the runner captures a bash
# step's output, so the helper's own lines do not reach these two files. The
# case below anchors on the call the helper makes instead.
helper_reported_state() { grep -hoE '^terminal_state=[A-Z_]+$' "$@" | cut -d= -f2; }

AGENT_STATES="$(agent_reported_state "$WORK/build.out" "$WORK/build.err" | sort -u | tr '\n' ' ')"
HELPER_STATES="$(helper_reported_state "$WORK/build.out" "$WORK/build.err" | sort -u | tr '\n' ' ')"
FINAL_STATES="$(printf '%s %s' "$HELPER_STATES" "$AGENT_STATES")"

# Finalisation's own evidence, in a form the stand-in cannot produce.
#
# The helper also prints its terminal evidence as bare `key=value` lines, and
# that would be the obvious anchor, but it is unreadable from here: the recipe
# runner captures a bash step's standard output and standard error at every
# nesting depth, so neither stream reaches build.out or build.err (issue
# #2875). The
# `INFO: resolved open PR ... (#1596)` line the helper writes to standard
# error is unreadable for the same reason. What does reach this script is the
# call the helper makes after emitting that evidence
# (amplifier-bundle/tools/workflow_final_status.sh, `gh_pr_view_with_retry
# "$PR_URL" --json state,mergeable,reviews,statusCheckRollup`): the gh
# stand-in records every call, that field list appears at exactly that one
# call site in the whole repository, and it is downstream of
# emit_terminal_evidence. So a recorded call of that shape is proof the shell
# helper ran and got past its evidence emission.
#
# It keeps the property the case exists to have: the gh stand-in writes
# gh-calls and the claude stand-in cannot, so no agent answer can produce it.
# The call is matched by field position rather than as a loose substring, and
# it is read here, before part 2 runs, so only the build can have made it.
final_status_call_recorded() {
  awk '$1 == "pr" && $2 == "view" && $(NF - 1) == "--json" \
       && $NF == "state,mergeable,reviews,statusCheckRollup" { found = 1 }
       END { exit !found }' "$WORK/gh-calls"
}

if printf '%s' "$FINAL_STATES" | grep -qw FAILED_IMPLEMENTATION; then
  flunk "part1_finalization_is_not_failed_implementation: the build reported FAILED_IMPLEMENTATION with the pull request's checks pending (helper: ${HELPER_STATES:-none}, agent: ${AGENT_STATES:-none})"
elif [ ! -s "$WORK/gh-create.argv" ]; then
  flunk "part1_finalization_is_not_failed_implementation: no pull request was opened, so finalisation had nothing to report on"
elif ! final_status_call_recorded; then
  # Do not fall back to the agent form, which is the stand-in's own answer and
  # proves nothing about finalisation; and do not look for the helper's own
  # output in build.out or build.err, which cannot hold it (see above).
  flunk "part1_finalization_is_not_failed_implementation: gh-calls, dumped below, holds no \`pr view ... --json state,mergeable,reviews,statusCheckRollup\` call, the one amplifier-bundle/tools/workflow_final_status.sh makes after emitting its terminal evidence, so finalisation's own shell never ran and the only state reported is the stand-in agent's (agent: ${AGENT_STATES:-none}, build exit $build_rc)"
elif ! grep -qE '"terminal_state" *: *"EXISTING_OPEN_PR"|^terminal_state=EXISTING_OPEN_PR$|"pr_number" *: *"?42"?|/pull/42' \
    "$WORK/build.out" "$WORK/build.err"; then
  flunk "part1_finalization_is_not_failed_implementation: the run's output does not name the open pull request (build exit $build_rc, helper: ${HELPER_STATES:-none}, agent: ${AGENT_STATES:-none})"
else
  pass "part1_finalization_is_not_failed_implementation - finalisation reported ${HELPER_STATES:-no state line}; the agent answered ${AGENT_STATES:-nothing}"
fi

# --- F: part 2, the whole drive ---------------------------------------------
# The drive is given the task the pull request implements as well as the pull
# request's number. Both are the operator's to supply, and the Crusty loop
# needs the task: autodrive_loop.sh refuses to start a round without one, and
# so does autodrive_crusty_rereview.sh between merge rounds, so a drive
# launched on a bare number cannot review the pull request at all. That is
# issue #2874, and passing the task here sidesteps it rather than covering it:
# the bare-number path stays untested until #2874 is fixed.
printf 'SUCCESS\n' > "$WORK/gh-state/checks"
drive_started=0
if [ -n "$BRANCH" ] && make_clone "$WORK/pr-clone" \
    && gitq -C "$WORK/pr-clone" fetch -q origin "$BRANCH" \
    && gitq -C "$WORK/pr-clone" worktree add -q -B "$BRANCH" "$DRIVE_REPO" "origin/$BRANCH"; then
  drive_started=1
  scenario_env drive "$BINDIR/amplihack" recipe run auto-drive-to-merge \
    -w "$DRIVE_REPO" --no-detach \
    -c pr_number=42 -c "repo_path=$DRIVE_REPO" -c base_branch=main \
    -c autodrive_dry_run=false -c no_merge=false \
    -c "task_description=$TASK" \
    -c "autodrive_state_dir=$AUTODRIVE_STATE" \
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
#
# One awk reads both files to the end and keeps the last match it saw. A grep
# piped into a consumer that stops early would, under pipefail, turn the
# producer's write error into the pipeline's status and collapse the value to
# the empty string; see tests/issue_1434_no_early_exit_pipelines.sh.
drive_verdict=""
if [ "$drive_started" -eq 1 ]; then
  drive_verdict="$(awk '
    {
      rest = $0
      while (match(rest, /"stop_reason"[[:space:]]*:[[:space:]]*"[a-z_]+"/)) {
        last = substr(rest, RSTART, RLENGTH)
        rest = substr(rest, RSTART + RLENGTH)
      }
    }
    END { if (last != "") print last }
  ' "$WORK/drive.out" "$WORK/drive.err")"
fi

if [ "$drive_started" -ne 1 ]; then
  flunk "part2_drive_ends_at_verdict_and_merges: there was no branch to drive (the per-PR clone or worktree could not be made)"
elif [ "$merge_recorded" -ne 1 ]; then
  flunk "part2_drive_ends_at_verdict_and_merges: the stub recorded no pr merge for pull request 42"
elif [ -z "$drive_verdict" ]; then
  flunk "part2_drive_ends_at_verdict_and_merges: the merge reached the stub but the drive printed no terminal report naming how it ended"
else
  pass "part2_drive_ends_at_verdict_and_merges - $drive_verdict"
fi

# Every agent step of the drive reached an entry of the stand-in's dispatch.
# The stand-in records a prompt that matched none, so a shipped prompt
# reworded out from under its discriminator fails here, naming the prompt,
# instead of quietly falling back to the fixed answer and changing what part 2
# covers. Part 1's prompts are recorded too and are not this case's business:
# the build deliberately falls back, which is how its cases stay as they were.
DRIVE_UNMATCHED="$WORK/drive-unmatched"
jq -r 'select(.run == "drive") | .first_line' "$WORK/agent-unmatched.jsonl" \
  > "$DRIVE_UNMATCHED" 2>/dev/null || : > "$DRIVE_UNMATCHED"
unmatched_count=$(( $(wc -l < "$DRIVE_UNMATCHED") ))
if [ "$drive_started" -ne 1 ]; then
  flunk "part2_every_drive_agent_prompt_matched_a_discriminator: the drive did not start, so no prompt was dispatched"
elif [ "$unmatched_count" -ne 0 ]; then
  flunk "part2_every_drive_agent_prompt_matched_a_discriminator: $unmatched_count drive prompt(s) matched no entry and fell back to the fixed answer: $(tr '\n' '|' < "$DRIVE_UNMATCHED")"
else
  pass "part2_every_drive_agent_prompt_matched_a_discriminator"
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
elif [ ! -f "$DRIVE_REPO/feature.txt" ]; then
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

# Requirement 9: neither run wrote into the repository the product was pointed
# at. Entries present before the runs are not this case's business; only
# entries the runs added are.
REPO_AFTER="$WORK/repo-status-after"
if git -C "$REPO_ROOT" status --porcelain > "$REPO_AFTER" 2>/dev/null; then
  ADDED="$(comm -13 <(sort -u -- "$REPO_BEFORE") <(sort -u -- "$REPO_AFTER") | tr '\n' '|')"
  if [ -n "$ADDED" ]; then
    flunk "part3_the_runs_wrote_nothing_into_the_repository: $REPO_ROOT gained: $ADDED"
  else
    pass "part3_the_runs_wrote_nothing_into_the_repository"
  fi
else
  flunk "part3_the_runs_wrote_nothing_into_the_repository: cannot read the status of $REPO_ROOT"
fi

# --- H: the descriptor states this scenario in full -------------------------
DESCRIPTOR="$REPO_ROOT/tests/gadugi/scenarios/issue-2806-end-to-end-build-and-drive.yaml"
DESCRIPTOR_CASE="every_case_this_run_emitted_is_asserted_by_the_descriptor"
printf '%s\n' "$DESCRIPTOR_CASE" >> "$CASE_NAMES"
if [ ! -f "$DESCRIPTOR" ]; then
  flunk "the scenario descriptor is missing: $DESCRIPTOR"
else
  # Both directions, over the cases this run reached rather than the ones that
  # passed. Forwards: a case the script emits that the descriptor does not
  # assert is coverage the descriptor does not state. Backwards: an assertion
  # naming a case the script no longer emits at all is coverage the descriptor
  # claims and the script no longer provides, which is how a case deleted from
  # the driver would otherwise pass unnoticed.
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
