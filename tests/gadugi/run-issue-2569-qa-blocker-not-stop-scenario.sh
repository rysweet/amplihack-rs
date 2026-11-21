#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2569: merge-ready
# criterion 1 is a recorded judgement. A QA measurement that cannot be
# completed is recorded as a named blocker the judgement carries on from, and
# it never ends the round; and the change set is measured against the PR's own
# base, never a guessed one.
#
# Black box, all shipped code, run unchanged: the real `amplihack` binary and
# the pinned recipe runner execute the shipped `autodrive-merge-evidence`
# recipe, including its bash step `step-02a-qa-inputs` and the measurement tool
# and helpers that step runs from this checkout's amplifier-bundle.
#
# What is stood in for, and nothing else, in one private PATH directory:
#   gh      answers one read-only call, the form the shipped base lookup makes:
#           `pr view <digits> --repo github.com/<owner>/<name> --json
#           baseRefName --jq .baseRefName`. It records every call and exits 96
#           on any other, so a write call or a lookup in another form fails the
#           case that made it.
#   claude  stands for the recipe's three agent steps. It answers `--version`,
#           records one line for each call that carried a prompt (never the
#           prompt), prints one fixed JSON object and takes no action.
#   npm     records its call and exits 0, so a Node repository's suite command
#           passes and a blocker in the record cannot come from the suite.
# No `gadugi-test` is on that PATH, and a set-up check confirms it.
#
# Cases (each runs the recipe once, in a fresh scratch Git repository):
#   A  a Node repository with no scenario directory: the record names a
#      blocker, not a pass, and the round carries on.
#   B  where the change set is measured against, in four legs: 1 the recipe
#      supplies the base; 2 the lookup of the PR supplies it; 3 nothing
#      answers, so no base and no change set are recorded; 4 a hostile base is
#      rejected and no substitution reaches a shell.
#   C  a scenario directory the operator named that does not exist, whatever
#      bytes its name holds, is a recorded state and the record stays one
#      clean JSON line.
# In every run the round must end normally: exit status 0 and all three agent
# steps called. The evidence folder is 0700 and the record file 0600.
#
# The two trees are kept apart. AMPLIHACK_HOME is this checkout, which ships
# the trusted tools and guidance. The repository under review, passed as
# `repo_path`, is a scratch repository under the scratch root. The measurement
# step treats a tools directory that lies inside the worktree under review as
# no trust root, so the two must not overlap, and this script checks that.
#
# Isolation is by construction: umask 077, one 0700 root, `env -i` carrying
# only the variables named in scenario_env, HOME and the agent configuration
# folders inside the root, a PATH holding only the stand-ins and links to
# named host tools, no Git system or global configuration, no prompts and only
# the file transport. The real ~/.claude and ~/.amplihack are never read or
# written, and no token, key or socket of the caller reaches a command. The
# script starts no background process and uses no timer, polling, retry or
# trace option.
#
# A missing binary, runner or host tool is a failure that names it, never a
# skip. CI's Install Smoke Test job runs this script directly against the
# binary it installs. It is not run on a shared host: run the static checks
# and read the CI result instead (docs/howto/run-the-qa-blocker-scenario.md).
#
# Run by hand: bash tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh
# Set AMPLIHACK_2569_QA_BIN to an amplihack binary to skip the build, and
# RECIPE_RUNNER_RS_PATH (or put recipe-runner-rs on PATH) for the runner. Point
# TMPDIR at a private folder you own. On success it prints ALL_CASES_PASSED and
# exits 0; otherwise one FAIL line for each failed assertion, then
# SCENARIO_FAILED and exit 1.
#
# No `-e` on purpose: every case must run and report, so an early exit would
# hide the failures after the first one. Set-up is guarded with `|| die`.
set -uo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
VERDICT=""
FAILED=0

# --- output ------------------------------------------------------------------
# A value shown in a line is printable ASCII, cut to 120 bytes. The record's own
# text is already sanitised by the tool; this keeps a failing case from carrying
# a control byte into a log. Neither the environment nor a stand-in's log is
# ever printed, and the record's `qa_guidance` is never shown.
show() {
  if [ -z "$1" ]; then printf '(empty)'; return; fi
  printf '%s' "$1" | LC_ALL=C tr -c '[:graph:] ' '?' | LC_ALL=C cut -c1-120
}
pass() { printf 'PASS: %s - %s\n' "$1" "$(show "$2")"; }
flunk() { printf 'FAIL: %s\n' "$1"; FAILED=1; }
eq() { # label expected observed
  if [ "$3" = "$2" ]; then pass "$1" "$3"; else flunk "$1: expected $(show "$2"), observed $(show "$3")"; fi
}
in_set() { # label observed allowed...
  local label="$1" got="$2" w
  shift 2
  for w in "$@"; do
    if [ "$got" = "$w" ]; then pass "$label" "$got"; return 0; fi
  done
  flunk "$label: expected one of $*, observed $(show "$got")"
}
# The verdict token, and its status as the return value. It does not exit, so
# the script's own last command is the verdict and the EXIT trap sees its status.
verdict() {
  VERDICT=1
  if [ "$FAILED" -eq 0 ]; then echo "ALL_CASES_PASSED"; return 0; fi
  echo "SCENARIO_FAILED"; return 1
}
die() {
  echo "FAIL: $*"
  VERDICT=1
  echo "SCENARIO_FAILED"
  exit 1
}

# --- the scratch root --------------------------------------------------------
HOST_TMP="$(cd "${TMPDIR:-/tmp}" 2>/dev/null && pwd -P)" || die "set-up: TMPDIR is not a folder"
WORK="$(mktemp -d "$HOST_TMP/issue-2569-qa.XXXXXX")" || die "set-up: cannot create the scratch root"

on_exit() {
  local rc=$?
  if [ "$rc" -ne 0 ] && [ -z "$VERDICT" ]; then echo "SCENARIO_FAILED"; fi
  # Only the folder this script made: under the temporary folder it was made in,
  # never empty, never the root, never the invoking user's home. No glob is
  # expanded inside it.
  case "$WORK" in
    ""|/) ;;
    "${HOME:-/nonexistent}"|"${HOME:-/nonexistent}"/) ;;
    "$HOST_TMP"/issue-2569-qa.*) rm -rf -- "$WORK" ;;
  esac
  exit "$rc"
}
trap on_exit EXIT
chmod 700 "$WORK" || die "set-up: cannot make the scratch root private"
WORK="$(cd "$WORK" && pwd -P)" || die "set-up: cannot resolve the scratch root"
readonly WORK

# --- A: the binary and the runner under test ---------------------------------
BIN="${AMPLIHACK_2569_QA_BIN:-}"
if [ -z "$BIN" ]; then
  BIN="$(cd "$REPO_ROOT" && cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/cargo-build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')" \
    || { tail -n 40 "$WORK/cargo-build.log"; die "set-up: cargo build -p amplihack --bin amplihack failed"; }
fi
case "$BIN" in /*) ;; *) BIN="$PWD/$BIN" ;; esac
{ [ -f "$BIN" ] && [ -x "$BIN" ]; } || die "set-up: no executable amplihack binary"

# Resolved once, here, and passed to every command as an absolute path: HOME and
# PATH inside a command are not the caller's.
RUNNER="${RECIPE_RUNNER_RS_PATH:-}"
[ -n "$RUNNER" ] || RUNNER="$(command -v recipe-runner-rs 2>/dev/null || true)"
[ -n "$RUNNER" ] || die "set-up: recipe-runner-rs is required (set RECIPE_RUNNER_RS_PATH or put it on PATH)"
case "$RUNNER" in /*) ;; *) RUNNER="$PWD/$RUNNER" ;; esac
{ [ -f "$RUNNER" ] && [ -x "$RUNNER" ]; } || die "set-up: recipe-runner-rs is not an executable file"

# --- B: isolation ------------------------------------------------------------
BINDIR="$WORK/bin"
private_dir() {
  mkdir -p -- "$1" || return 1
  chmod 700 "$1" || return 1
  [ "$(stat -c %a -- "$1")" = 700 ]
}
for d in "$BINDIR" "$WORK/home" "$WORK/home/.config" "$WORK/home/.claude" \
    "$WORK/home/.codex" "$WORK/home/profiles"; do
  private_dir "$d" || die "set-up: cannot create a 0700 folder"
done

# Host tools the shipped steps and helpers call. The first group is required;
# the second is linked when the host has it. Only a path is linked: a shell
# builtin or function has no file to link. No `gadugi-test` is linked.
REQUIRED_TOOLS=(bash sh env git jq cat grep sed awk sort tr head tail cut dirname
  basename mkdir rm cp mv ln chmod date stat mktemp uniq wc find xargs touch
  readlink realpath uname paste id)
OPTIONAL_TOOLS=(rmdir hostname nproc diff cmp tar gzip egrep fgrep tty od base64 expr seq)
for t in "${REQUIRED_TOOLS[@]}"; do
  p="$(command -v "$t" 2>/dev/null || true)"
  case "$p" in
    /*) ln -sf -- "$p" "$BINDIR/$t" ;;
    *) die "set-up: host tool missing: $t" ;;
  esac
done
for t in "${OPTIONAL_TOOLS[@]}"; do
  p="$(command -v "$t" 2>/dev/null || true)"
  case "$p" in /*) [ -e "$BINDIR/$t" ] || ln -sf -- "$p" "$BINDIR/$t" ;; esac
done
ln -sf -- "$BIN" "$BINDIR/amplihack"
ln -sf -- "$RUNNER" "$BINDIR/recipe-runner-rs"

# One wrapper for every command that runs the product or Git: env -i with an
# explicit allowlist. Each label owns a TMPDIR, a folder where the stand-ins
# record their calls, and a work root, all created by begin_case. Variables for
# one command only (EXTRA_ENV) come after the allowlist.
scenario_env() {
  local label="$1"; shift
  env -i \
    HOME="$WORK/home" \
    PATH="$BINDIR" \
    TERM=dumb LANG=C TZ=UTC \
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
    AMPLIHACK_SKIP_REFLECTION=1 AMPLIHACK_SKIP_POWER_STEERING=1 AMPLIHACK_USAGE_LIMIT_WAIT=0 \
    NO_UPDATE_NOTIFIER=1 AMPLIHACK_SKIP_AUTO_INSTALL=1 AMPLIHACK_NO_UPDATE_CHECK=1 \
    AMPLIHACK_NO_FRESHNESS_CHECK=1 RECIPE_RUNNER_NO_UPDATE_CHECK=1 \
    RECIPE_RUNNER_RS_PATH="$RUNNER" \
    RECORD_DIR="$WORK/rec-$label" QA_GH_BASE=qa-base \
    "$@"
}
gitq() { scenario_env setup git "$@"; }

begin_case() { # label
  local d
  for d in "$WORK/tmp-$1" "$WORK/rec-$1" "$WORK/work-$1" "$WORK/ev-$1"; do
    private_dir "$d" || die "set-up: cannot create a 0700 folder for $1"
  done
}
begin_case setup

# --- C: the stand-ins --------------------------------------------------------
# Written from quoted heredocs: nothing is interpolated into their source. Data
# reaches them through the environment only.
cat > "$BINDIR/gh" <<'GH_STUB'
#!/bin/bash
set -u
printf '%s\n' "$*" >> "${RECORD_DIR:?}/gh-calls"
ok=0
if [ "$#" -eq 9 ] && [ "$1" = pr ] && [ "$2" = view ] && [ "$4" = --repo ] \
    && [ "$6" = --json ] && [ "$7" = baseRefName ] && [ "$8" = --jq ] && [ "$9" = .baseRefName ]; then
  case "$3" in
    ''|*[!0-9]*) ;;
    *) case "$5" in github.com/*/*) ok=1 ;; esac ;;
  esac
fi
if [ "$ok" = 1 ]; then
  printf '%s\n' "${QA_GH_BASE:-}"
  exit 0
fi
echo "gh stand-in: call not answered" >&2
exit 96
GH_STUB

cat > "$BINDIR/claude" <<'CLAUDE_STUB'
#!/bin/bash
set -u
# The version the product checks before it launches an agent step.
if [ "${1-}" = "--version" ]; then echo "2.1.319 (Claude Code)"; exit 0; fi
input="$(cat)"
# One line for each call that carried a prompt. The prompt is never kept.
if [ -n "$input" ]; then printf 'call\n' >> "${RECORD_DIR:?}/agent-calls"; fi
printf '%s\n' '{"status":"ok","summary":"stand-in agent, no action taken"}'
CLAUDE_STUB

cat > "$BINDIR/npm" <<'NPM_STUB'
#!/bin/bash
set -u
printf '%s\n' "$*" >> "${RECORD_DIR:?}/npm-calls"
exit 0
NPM_STUB
chmod 700 "$BINDIR/gh" "$BINDIR/claude" "$BINDIR/npm"

# --- D: helpers --------------------------------------------------------------
# The three agent steps of the recipe: step-01-sync-with-base,
# step-02-qa-team-scenarios and step-03-wait-for-ci. step-02a-qa-inputs is a
# bash step and does not call the agent. The count is of calls, never of prompt
# text: one fixed answer serves all three, so it shows that the round reached its
# last agent step and cannot show which prompt each call carried.
EXPECTED_AGENT_CALLS=3
ORIGIN_URL="https://github.com/qa-owner/qa-repo.git"

safe_path() { # one absolute line, no quote, backslash, $, backtick, doubled brace or control byte
  case "$1" in /*) ;; *) return 1 ;; esac
  case "$1" in *[\"\\\$\`]* | *'{{'* | *'}}'* | *$'\n'*) return 1 ;; esac
  [ "$(printf '%s' "$1" | LC_ALL=C tr -d '\040-\176\200-\377'; printf x)" = "x" ]
}

# 0 when the folder equals the checkout, lies inside it, or contains it.
overlaps_checkout() {
  local d
  d="$(cd -- "$1" 2>/dev/null && pwd -P)" || return 0
  case "$d" in "$REPO_ROOT"|"$REPO_ROOT"/*) return 0 ;; esac
  case "$REPO_ROOT" in "$d"/*) return 0 ;; esac
  return 1
}

mode_of() { stat -c %a -- "$1" 2>/dev/null; }
count_lines() { if [ -f "$1" ]; then awk 'END { print NR }' "$1"; else echo 0; fi; }
file_text() { if [ -f "$1" ]; then cat -- "$1"; fi; }
count_text() { # file text: the lines of the file that hold the text
  if [ -f "$1" ]; then awk -v pat="$2" 'index($0, pat) { n++ } END { print n + 0 }' "$1"; else echo 0; fi
}
field() { # file name: a field of the record as text, or nothing
  jq -r --arg k "$2" 'if type == "object" then (.[$k] // "") | tostring else "" end' "$1" 2>/dev/null
}
record_state() { # file: readable, missing or not-one-json-object
  [ -f "$1" ] || { printf 'missing'; return; }
  if [ "$(jq -rs 'if (length == 1 and (.[0] | type) == "object") then "ok" else "no" end' "$1" 2>/dev/null)" = ok ]; then
    printf 'readable'
  else
    printf 'not-one-json-object'
  fi
}
check_record() { # prefix file: 0 when the record is one readable JSON object
  local s
  s="$(record_state "$2")"
  eq "$1 record" readable "$s"
  [ "$s" = readable ]
}

# The fixture repositories. Every Git command runs through scenario_env, so the
# isolation applies to set-up as it does to the recipe.
new_node_repo() { # dir
  gitq init -q -b main "$1" \
    && printf '{"name":"qa-fixture","version":"1.0.0","scripts":{"test":"true"}}\n' > "$1/package.json" \
    && gitq -C "$1" add package.json \
    && gitq -C "$1" commit -q -m "base"
}
# The first commit is also refs/remotes/origin/qa-base, set with update-ref and
# never fetched, because the measurement reads the merge base from that ref. The
# second commit adds src/index.js, so a change set measured against qa-base holds
# that one path.
add_change() { # dir
  gitq -C "$1" update-ref refs/remotes/origin/qa-base HEAD \
    && mkdir -p -- "$1/src" \
    && printf 'module.exports = 1;\n' > "$1/src/index.js" \
    && gitq -C "$1" add src/index.js \
    && gitq -C "$1" commit -q -m "change"
}
# The URL is only a string: nothing contacts it. The stand-in agent takes no
# action, the measurement runs no command that reaches a remote, and
# GIT_ALLOW_PROTOCOL=file would stop any other protocol. It exists so that the
# shipped lookup can establish the GitHub repository and reach the gh stand-in.
add_origin() { gitq -C "$1" remote add origin "$ORIGIN_URL"; }
guard_tree() { # dir
  if overlaps_checkout "$1"; then die "set-up: the repository under review overlaps the checkout"; fi
}

EXTRA_ENV=()
EXTRA_CTX=()
RUN_RC=""
# Runs the recipe once. EXTRA_ENV holds variables for this command only and
# EXTRA_CTX holds more `-c key=value` pairs. repo_path is always passed, so the
# recipe never measures the checkout itself.
run_recipe() { # label repo pr
  local label="$1" repo="$2" pr="$3"
  scenario_env "$label" ${EXTRA_ENV[@]+"${EXTRA_ENV[@]}"} \
    "$BINDIR/amplihack" recipe run autodrive-merge-evidence \
    -w "$repo" --no-detach \
    -c "repo_path=$repo" -c "pr_number=$pr" \
    -c "autodrive_qa_evidence=$WORK/ev-$label/qa-evidence.json" \
    ${EXTRA_CTX[@]+"${EXTRA_CTX[@]}"} \
    </dev/null >"$WORK/out-$label" 2>"$WORK/err-$label"
  RUN_RC=$?
}

# What every run must show: the round ended normally, all three agent steps ran,
# and the evidence folder and the record stayed private.
round_continued() { # prefix label
  eq "$1 exit-status" 0 "$RUN_RC"
  eq "$1 agent-calls" "$EXPECTED_AGENT_CALLS" "$(count_lines "$WORK/rec-$2/agent-calls")"
  eq "$1 mode" "dir=700 file=600" "dir=$(mode_of "$WORK/ev-$2") file=$(mode_of "$WORK/ev-$2/qa-evidence.json")"
}

# --- E: set-up checks --------------------------------------------------------
TOOLS_REAL="$(cd "$REPO_ROOT/amplifier-bundle/tools" 2>/dev/null && pwd -P || true)"
SKILL_REAL="$(cd "$TOOLS_REAL/../skills/merge-ready" 2>/dev/null && pwd -P || true)"
UNUSABLE=""
safe_path "$WORK" || UNUSABLE="${UNUSABLE:+$UNUSABLE,}scratch-root"
safe_path "$REPO_ROOT" || UNUSABLE="${UNUSABLE:+$UNUSABLE,}checkout"
safe_path "$TOOLS_REAL" || UNUSABLE="${UNUSABLE:+$UNUSABLE,}tools-dir"
safe_path "$SKILL_REAL" || UNUSABLE="${UNUSABLE:+$UNUSABLE,}skill-dir"
PATHS_STATE=ok
[ -z "$UNUSABLE" ] || PATHS_STATE="unusable: $UNUSABLE"
eq "set-up paths" ok "$PATHS_STATE"
TREES_STATE=disjoint
if overlaps_checkout "$WORK"; then TREES_STATE=overlap; fi
eq "set-up trees" disjoint "$TREES_STATE"
# Where gadugi-test is found, case A would record FAIL with no-scenarios and not
# BLOCKED, so the scenario stops here and names the tool.
eq "set-up gadugi-test-absent" absent \
  "$(scenario_env setup bash -c 'if command -v gadugi-test >/dev/null 2>&1; then echo present; else echo absent; fi')"
if [ "$FAILED" -ne 0 ]; then verdict; exit 1; fi

# --- F: case A, a Node repository with no scenarios --------------------------
begin_case case-a
new_node_repo "$WORK/repo-a" || die "set-up: cannot create the case A repository"
guard_tree "$WORK/repo-a"
EXTRA_ENV=(); EXTRA_CTX=()
run_recipe case-a "$WORK/repo-a" 7
REC_A="$WORK/ev-case-a/qa-evidence.json"
round_continued "case-a" case-a
if check_record "case-a" "$REC_A"; then
  in_set "case-a qa_status" "$(field "$REC_A" qa_status)" FAIL BLOCKED QA_UNRESOLVED
  in_set "case-a qa_reason" "$(field "$REC_A" qa_reason)" qa-command-failed no-scenarios \
    qa-config-invalid qa-command-missing qa-command-not-installed gadugi-test-missing qa-unresolved
  # The measured fact behind the blocker: the tool is absent and the record says so.
  eq "case-a gadugi_status" NOT_INSTALLED "$(field "$REC_A" gadugi_status)"
fi

# --- G: case B, the base the change set is measured against ------------------
# Leg 1: the recipe supplies the base. It is used before any lookup.
begin_case case-b-leg-1
{ new_node_repo "$WORK/repo-b1" && add_change "$WORK/repo-b1" && add_origin "$WORK/repo-b1"; } \
  || die "set-up: cannot create the case B leg 1 repository"
guard_tree "$WORK/repo-b1"
EXTRA_ENV=(); EXTRA_CTX=(-c autodrive_base_branch=qa-base)
run_recipe case-b-leg-1 "$WORK/repo-b1" 7
REC_B1="$WORK/ev-case-b-leg-1/qa-evidence.json"
round_continued "case-b leg-1" case-b-leg-1
if check_record "case-b leg-1" "$REC_B1"; then
  eq "case-b leg-1 qa_base_source" recipe "$(field "$REC_B1" qa_base_source)"
  eq "case-b leg-1 qa_base" qa-base "$(field "$REC_B1" qa_base)"
  eq "case-b leg-1 qa_command_paths" src/index.js "$(field "$REC_B1" qa_command_paths)"
fi
eq "case-b leg-1 gh-calls" "" "$(file_text "$WORK/rec-case-b-leg-1/gh-calls")"

# Leg 2: the recipe supplies none, so the lookup of the PR supplies it. This is
# the one leg that exercises the pr-base source. A lookup in any other form is
# not answered by the stand-in, the record then reads none, and the leg fails
# on the source.
begin_case case-b-leg-2
{ new_node_repo "$WORK/repo-b2" && add_change "$WORK/repo-b2" && add_origin "$WORK/repo-b2"; } \
  || die "set-up: cannot create the case B leg 2 repository"
guard_tree "$WORK/repo-b2"
EXTRA_ENV=(); EXTRA_CTX=()
run_recipe case-b-leg-2 "$WORK/repo-b2" 7
REC_B2="$WORK/ev-case-b-leg-2/qa-evidence.json"
round_continued "case-b leg-2" case-b-leg-2
if check_record "case-b leg-2" "$REC_B2"; then
  eq "case-b leg-2 qa_base_source" pr-base "$(field "$REC_B2" qa_base_source)"
  eq "case-b leg-2 qa_base" qa-base "$(field "$REC_B2" qa_base)"
  eq "case-b leg-2 qa_command_paths" src/index.js "$(field "$REC_B2" qa_command_paths)"
fi
eq "case-b leg-2 gh-calls" \
  "pr view 7 --repo github.com/qa-owner/qa-repo --json baseRefName --jq .baseRefName" \
  "$(file_text "$WORK/rec-case-b-leg-2/gh-calls")"

# Leg 3: nothing answers. The repository has no origin, so the lookup stops
# before it calls gh. No change set is guessed and the literal `main` is never
# substituted. A lookup that reaches gh and is not answered is covered by no leg.
begin_case case-b-leg-3
new_node_repo "$WORK/repo-b3" || die "set-up: cannot create the case B leg 3 repository"
guard_tree "$WORK/repo-b3"
EXTRA_ENV=(); EXTRA_CTX=()
run_recipe case-b-leg-3 "$WORK/repo-b3" 7
REC_B3="$WORK/ev-case-b-leg-3/qa-evidence.json"
round_continued "case-b leg-3" case-b-leg-3
if check_record "case-b leg-3" "$REC_B3"; then
  eq "case-b leg-3 qa_base_source" none "$(field "$REC_B3" qa_base_source)"
  eq "case-b leg-3 qa_base" "" "$(field "$REC_B3" qa_base)"
  eq "case-b leg-3 qa_command_paths" "(unknown)" "$(field "$REC_B3" qa_command_paths)"
  # The changed-path list is not known, and the predicate reads true only for a
  # known, non-empty list.
  eq "case-b leg-3 docs_only" unknown "$(field "$REC_B3" docs_only)"
fi
eq "case-b leg-3 gh-calls" "" "$(file_text "$WORK/rec-case-b-leg-3/gh-calls")"

# Leg 4: a hostile base is rejected, is not replaced by a lookup, and reaches no
# shell. Only the record, which the tool writes, is read for the value: the
# runner may echo the context it was given in its own output.
begin_case case-b-leg-4
{ new_node_repo "$WORK/repo-b4" && add_origin "$WORK/repo-b4"; } \
  || die "set-up: cannot create the case B leg 4 repository"
guard_tree "$WORK/repo-b4"
# shellcheck disable=SC2016 # the command substitution is data and must not run here
HOSTILE_BASE='-HOSTILEBASE2569$(touch injected2569)'
EXTRA_ENV=(); EXTRA_CTX=(-c "autodrive_base_branch=$HOSTILE_BASE")
run_recipe case-b-leg-4 "$WORK/repo-b4" 7
REC_B4="$WORK/ev-case-b-leg-4/qa-evidence.json"
round_continued "case-b leg-4" case-b-leg-4
if check_record "case-b leg-4" "$REC_B4"; then
  eq "case-b leg-4 qa_base_source" none "$(field "$REC_B4" qa_base_source)"
  eq "case-b leg-4 qa_base" "" "$(field "$REC_B4" qa_base)"
  eq "case-b leg-4 record-hostile-text" 0 "$(count_text "$REC_B4" HOSTILEBASE2569)"
fi
eq "case-b leg-4 gh-calls" "" "$(file_text "$WORK/rec-case-b-leg-4/gh-calls")"
eq "case-b leg-4 injected-file" 0 \
  "$(find "$WORK" -maxdepth 8 -name injected2569 2>/dev/null | awk 'END { print NR }')"

# --- H: case C, an operator-named scenario directory that does not exist -----
# The name holds a double quote, a backslash, a tab and an ESC byte. The tool
# consults a scenario directory only where gadugi-test is required, and a Node
# repository with no base requires it, which the first assertion checks.
begin_case case-c
new_node_repo "$WORK/repo-c" || die "set-up: cannot create the case C repository"
guard_tree "$WORK/repo-c"
HOSTILE_DIR="$(printf 'no"such\\dir\tx\033[31m_2569')"
EXTRA_ENV=("AUTODRIVE_QA_SCENARIO_DIR=$HOSTILE_DIR"); EXTRA_CTX=()
run_recipe case-c "$WORK/repo-c" 7
REC_C="$WORK/ev-case-c/qa-evidence.json"
round_continued "case-c" case-c
eq "case-c record-shape" "lines=1 esc=0 readable" \
  "lines=$(count_lines "$REC_C") esc=$(if [ -f "$REC_C" ]; then LC_ALL=C tr -cd '\033' < "$REC_C" | LC_ALL=C wc -c | tr -d ' '; else echo 0; fi) $(record_state "$REC_C")"
if [ "$(record_state "$REC_C")" = readable ]; then
  if [ "$(field "$REC_C" gadugi_required)" = true ]; then
    pass "case-c gadugi_required" true
    eq "case-c gadugi_scenario_dir_state" operator-missing "$(field "$REC_C" gadugi_scenario_dir_state)"
  else
    flunk "case-c gadugi_required: expected true, observed $(show "$(field "$REC_C" gadugi_required)") (the fixture does not require gadugi-test, so the scenario directory is not consulted)"
  fi
fi

verdict
