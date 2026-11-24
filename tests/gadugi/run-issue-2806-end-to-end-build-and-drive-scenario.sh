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
#           pr checks, pr merge, pr ready/edit/comment, the two read-only
#           account calls `api user` and `api rate_limit`, and the read of the
#           repository's allowed merge methods, `api repos/{owner}/{repo}`
#           (#1888). It reads the body from standard input exactly where real
#           gh does (`--body-file -`), records every call in $WORK/gh-calls,
#           and exits 96 on anything else, GraphQL included (recorded in
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
#           exactly one object. The one exception in part 1 is the build
#           loop's turn (#3129: default-workflow 4.0.0 builds through the
#           shared goal loop), which needs an action: it is answered from the
#           turn's own reading, quoted in the prompt, as a competent agent
#           would act on it. With no pushed branch it commits feature.txt in
#           the run's worktree and pushes with plain `git push` (`continue`);
#           with no pull request it chooses `open-pr` with a plain title and
#           description; then `mark-ready`; then `ready`. A git step that
#           fails, or a turn past the eighth, hands off, so a fault ends the
#           build instead of looping.
#           In part 2 one answer cannot serve every step: the build stage's
#           preflight, its no-merge directive, its PR resolution and the goal
#           loop's turn (#3129) need shapes that contradict each other. So the
#           stand-in carries a dispatch table, one entry per agent step of the
#           drive, keyed on a substring of the prompt that occurs in exactly
#           one shipped prompt (or on the first line where that alone is
#           unambiguous). The turn's answer asks for the merge at the head it
#           read; a second turn hands off, so a fault ends the drive. Each
#           entry says which step it answers and why that answer is what a
#           competent agent would report of this fixture, and the facts it
#           rests on are read rather than asserted: the head, branch and base
#           come from the PR worktree, and the private state directory is the
#           one the driver gave the drive. A prompt matching no entry keeps part 1's
#           fixed answer and is recorded in $WORK/agent-unmatched.jsonl, which
#           a case of its own reads: part 1's prompts other than the build
#           loop's turn are there by design, and a part 2 prompt there is a
#           shipped prompt reworded out from under its discriminator.
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
# Part 1  the whole build. Checks are PENDING. default-workflow 4.0.0 (#3129)
#         builds through the shared goal loop and ends when the pull request
#         is open and marked ready for review; a build never merges and no
#         longer runs finalisation. The run ends with a pull request opened
#         on the stub for the run's branch and that branch pushed to the bare
#         origin at the head of the run's worktree, read from
#         `git worktree list`. The build loop marked that pull request ready
#         (`pr ready 42`, recorded by the gh stand-in, which the claude
#         stand-in cannot write), asked for no merge, and the run exited 0.
#         Pending checks do not hold the build up (issue 2343): the build
#         ends with the pull request open whatever the checks say.
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
  # The repository's own allowed merge methods, read once by autodrive_merge
  # immediately before the merge request (#1888). Real gh expands
  # {owner}/{repo} itself; this stand-in is gh, so the path arrives exactly as
  # the tool wrote it. All three methods are allowed, as they are in a
  # repository created on GitHub today, so the drive settles on squash and
  # records the merge this scenario has always recorded.
  "api repos/{owner}/{repo}"|"api repos/qa-owner/qa-repo")
    emit '{"allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":true}' "$@"
    exit 0 ;;
  # The goal loop's reading (#3129) names the repository from the PR URL and
  # asks whether the base requires strict status checks: no ruleset, and no
  # classic protection (GitHub's own 404 for an unprotected branch).
  "api repos/qa-owner/qa-repo/rules/branches/main") emit '[]' "$@"; exit 0 ;;
  "api repos/qa-owner/qa-repo/branches/main/protection/required_status_checks")
    echo "gh: Branch not protected (HTTP 404)" >&2; exit 1 ;;
  "pr list")
    if [ -e "$state/pr-created" ]; then emit "[$(pr_json)]" "$@"; else emit '[]' "$@"; fi
    exit 0 ;;
  "pr create")
    drain_body "$@"
    printf '%s\n' "$*" >> "$root/gh-create.argv"
    # The folder gh was called in, as evidence only. The build goal (#3129)
    # runs `gh pr create` from the launch checkout with `--head <branch>`, so
    # the branch comes from that argument below, and the run's head is read
    # afterwards from the run's own worktree, never from this folder.
    printf '%s\n' "$(pwd -P)" >> "$root/gh-create.cwd"
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
# Part 2 only. Part 1 enters only the build loop's turn entry below; every
# other part 1 step keeps the single fixed answer above.
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
# --- the build loop's turn (part 1) ------------------------------------------
# default-workflow 4.0.0 (#3129) builds through the shared goal loop, whose
# turn needs an action that the fixed answer above does not carry. Every other
# part 1 step keeps the fixed answer. The answer is read from the turn's own
# reading, which the turn prompt quotes between its UNTRUSTED-READING markers,
# and is the one a competent agent would give: commit the task's file in the
# run's worktree and push with plain `git push`, then open the pull request
# with a plain title and description, then mark it ready, then end with
# `ready`. A git step that fails, or a turn past the eighth, hands off to the
# operator, so a fault ends the build instead of looping.
if [ "${QA_RUN_LABEL:-}" = build ] && has 'You are one turn of a goal-driven loop'; then
  step=autodrive-goal-turn/decide
  reading="$(printf '%s\n' "$input" | awk '$0 == "<<<UNTRUSTED-READING" { f = 1; next } $0 == "UNTRUSTED-READING>>>" { f = 0 } f')"
  printf '%s' "$reading" | jq -e 'type == "object"' >/dev/null 2>&1 || reading='{}'
  r() { printf '%s' "$reading" | jq -r "$1" 2>/dev/null; }
  turn="$(r '.turn // 0')"
  own="$(r '.goal.prs.own // empty')"
  pushed="$(r '.goal.run.pushed_head // empty')"
  wt="$(r '.goal.run.worktree.path // empty')"
  br="$(r '.goal.run.branch // empty')"
  prev="$(r '.previous.outcome // "none"')"
  build_handoff() {
    jq -nc --arg d "$1" '{action:"handoff",args:{owner:"the operator who started this run",decision:$d,
      text:("The build could not go on: " + $d + ". The operator decides what happens next.")},outcomes:[],
      why:("I read the state and could not take the next step: " + $d + ".")}'
  }
  if [ "${turn:-0}" -gt 8 ]; then
    response="$(build_handoff "the build did not reach ready in eight turns; the last result was ${prev}")"
  elif [ -z "$own" ] && [ -z "$pushed" ]; then
    if [ -z "$wt" ] || [ -z "$br" ]; then
      response="$(build_handoff "the reading names no worktree or branch for this run")"
    elif { [ -f "$wt/feature.txt" ] || printf 'qa feature\n' > "$wt/feature.txt"; } \
        && git -C "$wt" add feature.txt >/dev/null 2>&1 \
        && git -C "$wt" -c user.name=qa -c user.email=qa@example.invalid commit -q -m "Add feature.txt" >/dev/null 2>&1 \
        && git -C "$wt" push -q -u origin "$br" >/dev/null 2>&1; then
      response="$(jq -nc '{action:"continue",args:{},outcomes:[],why:"The branch had no commit for the task, so I added feature.txt, committed it through the hooks and pushed the branch."}')"
    else
      response="$(build_handoff "committing or pushing feature.txt on ${br} failed")"
    fi
  elif [ -z "$own" ]; then
    response="$(jq -nc '{action:"open-pr",args:{title:"Add feature.txt at the repository root",
      body:"Adds feature.txt, a one-line text file, at the repository root.\n"},outcomes:[],
      why:"The branch is pushed and has no pull request, so I open it with a plain title and description."}')"
  elif [ "$prev" != marked-ready ] && [ "$prev" != already-ready ]; then
    response="$(jq -nc '{action:"mark-ready",args:{},outcomes:[],why:"The pull request is open, so I mark it ready for review."}')"
  else
    response="$(jq -nc '{action:"ready",args:{text:"The change is pushed, and its pull request is open and ready for review."},outcomes:[],why:"The pull request is ready for review, so the build is done."}')"
  fi
fi

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

  # First, because the turn prompt quotes the task text, the reading and the
  # previous record back at the agent: a later entry's discriminator could
  # appear inside a quotation.
  if has 'You are one turn of a goal-driven loop'; then
    # autodrive-goal-turn/decide (#3129): the loop's one agent step per turn.
    # Every check passed at this head and merging is allowed, so a competent
    # agent asks for the merge at the head it read. The task text and guidance
    # require no outcome, so it lists `outcomes` as an empty list, as the turn
    # prompt asks: a merge decision that lists none is refused (#3129 R2). A
    # second turn means the merge did not end the goal; the stand-in then
    # hands off rather than asking again, so a fault ends the drive instead of
    # looping.
    step=autodrive-goal-turn/decide
    turns="$root/drive-turns"
    printf 'x\n' >> "$turns"
    if [ "$(wc -l < "$turns")" -eq 1 ]; then
      response="$(j '{action:"merge",args:{head:$h},outcomes:[],why:"Every check passed at this head and merging is allowed, and the task names no required outcome, so I ask for the merge at the head I read."}')"
    else
      response="$(j '{action:"handoff",args:{owner:"operator",decision:"why the merge did not end the drive",text:"The merge requested on the first turn did not end the drive. The operator decides what happens next."},why:"The goal did not end after the merge."}')"
    fi
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

# The branch, as the stub recorded it when the pull request was asked for: the
# branch of `--head`, in either of the two forms gh accepts. The argv file is
# read here only for whether a `pr create` was recorded at all.
if [ -s "$WORK/gh-create.argv" ]; then
  BRANCH="$(cat "$WORK/gh-state/branch" 2>/dev/null)"
fi
# Whole-string, and no leading dash: the value reaches `git fetch` and
# `git worktree add` as an argument, and `grep` would have matched one line of
# a multi-line value and accepted a value Git would read as an option.
[[ $BRANCH =~ ^[A-Za-z0-9._/][A-Za-z0-9._/-]*$ ]] || BRANCH=""
# The run's own head: the head of the worktree Git lists on that branch. The
# build goal (#3129) opens the pull request from the launch checkout, so the
# folder gh was called in is not the run's work tree.
BUILD_WT=""
[ -z "$BRANCH" ] || BUILD_WT="$(gitq -C "$WORK/build-repo" worktree list --porcelain 2>/dev/null \
  | awk -v want="refs/heads/$BRANCH" '/^worktree /{p = substr($0, 10)} /^branch /{if (substr($0, 8) == want) {print p; exit}}')"
LOCAL_HEAD=""
[ -z "$BUILD_WT" ] || LOCAL_HEAD="$(gitq -C "$BUILD_WT" rev-parse HEAD 2>/dev/null)"
ORIGIN_HEAD=""
[ -z "$BRANCH" ] || ORIGIN_HEAD="$(gitq --git-dir="$ORIGIN" rev-parse "refs/heads/$BRANCH" 2>/dev/null)"

if [ -z "$BRANCH" ]; then
  flunk "part1_build_opens_pr_and_pushes_branch: the stub recorded no pr create (build exit $build_rc)"
elif [ -z "$LOCAL_HEAD" ] || [ "$LOCAL_HEAD" != "$ORIGIN_HEAD" ]; then
  flunk "part1_build_opens_pr_and_pushes_branch: pr create for '$BRANCH', but the run's worktree '${BUILD_WT:-none listed}' is at '${LOCAL_HEAD:-nothing}', not the origin's '$ORIGIN_HEAD'"
else
  pass "part1_build_opens_pr_and_pushes_branch - $BRANCH at $ORIGIN_HEAD"
fi

# The build loop's end, from facts the claude stand-in cannot produce: the gh
# stand-in recorded `pr ready 42` and no merge, and the run exited 0. Read
# here, before part 2 runs, so only the build can have made these calls.
if [ "$build_rc" -ne 0 ]; then
  flunk "part1_build_loop_ends_ready_without_merge: the build exited $build_rc"
elif ! awk '$1 == "pr" && $2 == "ready" && $3 == "42" { found = 1 } END { exit !found }' "$WORK/gh-calls"; then
  flunk "part1_build_loop_ends_ready_without_merge: the build never asked for 'pr ready 42'"
elif [ -s "$WORK/gh-merge.argv" ]; then
  flunk "part1_build_loop_ends_ready_without_merge: the build asked for a merge: $(cat "$WORK/gh-merge.argv")"
else
  pass "part1_build_loop_ends_ready_without_merge"
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
    -w "$DRIVE_REPO" --no-detach -f json \
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
# The drive's verdict: with `-f json` the run prints its report,
# `autodrive_outcome`, whose `stop_reason` is the drive's own word for how it
# ended (#3129: the goal loop's end, read by report.rs). The wording of any
# sentence beside it is the product's and is not read here.
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
