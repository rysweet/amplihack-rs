#!/usr/bin/env bash
# test-issue-2033-qa-git-env.sh — regression test for #2033.
#
# An auto-drive merge-readiness phase ran another repository's tests with
# GIT_DIR, GIT_WORK_TREE and similar variables inherited. Git reads those
# before the working directory, so the tests' own `git add` / `git commit`
# landed in the repository the variables named (the real PR worktree), not in
# the checkout under test.
#
# This runs amplifier-bundle/tools/autodrive_qa_evidence.sh, unmodified, on a
# caller repository while every repository-selecting git variable points at a
# decoy repository. The suite command commits with no `-C`, so a failed
# isolation commits into the decoy. Cases:
#   1. clean environment: git_env_cleared and run_env_cleared are `none`;
#   2. all 12 git names at the decoy, all 5 run-identity names set: the same
#      exit status and qa_status as case 1, head_sha is the caller's, the
#      decoy is byte-for-byte unchanged, the caller gains one commit, and the
#      evidence names all 12 and all 5 variables;
#   3. git-repo-env.sh missing beside the tool: exit 1 by name, no evidence;
#   4. git-repo-env.sh a symlink: the same as case 3.
# Only exit statuses and before-and-after snapshots are compared: no sleep,
# timeout or clock read.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-2033-qa-git-env.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
TOOL="${TOOLS}/autodrive_qa_evidence.sh"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }
[[ -f "$TOOL" ]] || harness_error "tool not found: $TOOL"
command -v python3 >/dev/null 2>&1 || harness_error "python3 is required to read the JSON"
field_of() { python3 -c 'import json,sys; print(json.loads(sys.stdin.buffer.read().decode("utf-8")).get(sys.argv[1], "<missing>"))' "$1"; }

GIT_NAMES="GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR GIT_DIR"
GIT_NAMES="$GIT_NAMES GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_INDEX_FILE GIT_INDEX_VERSION GIT_NAMESPACE"
GIT_NAMES="$GIT_NAMES GIT_OBJECT_DIRECTORY GIT_PREFIX GIT_QUARANTINE_PATH GIT_WORK_TREE"
RUN_NAMES="AMPLIHACK_RECIPE_RUN_ID AMPLIHACK_RUN_DIR AMPLIHACK_RUN_ID AMPLIHACK_SESSION_TREE_DIR AMPLIHACK_TREE_ID"

# Run from a git hook, an inherited GIT_DIR would hijack `git init` below: the
# bug under test biting the test. Scrub it before any fixture exists.
# shellcheck disable=SC2086 # word-splitting the name lists is intended
unset $GIT_NAMES $RUN_NAMES
for v in $(compgen -e | grep '^GIT_CONFIG' || true); do unset "$v"; done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2033.XXXXXX")"
[[ -n "$WORK" && -d "$WORK" && "$WORK" != "/" ]] || harness_error "mktemp -d failed"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf -- "$WORK"' EXIT

# Never the real HOME, system or global git config: no core.hooksPath,
# fsmonitor or ~/.amplihack can be read.
export HOME="$WORK/home" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
mkdir -p "$HOME"
ID_ENV=(GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid)

PASS_COUNT=0
FAIL_COUNT=0
CASES_PASSED=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

# make_repo <dir>: a repository with one tracked file and one commit.
make_repo() {
    mkdir -p "$1"
    : > "$1/Cargo.toml"
    printf '%s\n' "$(basename "$1")" > "$1/README"
    ( cd "$1" && git init -q . && git add -A && env "${ID_ENV[@]}" git commit -q -m init ) >/dev/null 2>&1 \
        || harness_error "could not create $1"
    [[ -d "$1/.git" ]] || harness_error "git init was hijacked for $1"
}
CALLER="$WORK/caller"
DECOY="$WORK/decoy"
make_repo "$CALLER"
make_repo "$DECOY"

# snapshot <repo> [objects]: everything a stray `git add` / `git commit` changes.
snapshot() {
    ( cd "$1" && {
        sha256sum .git/index
        cat .git/HEAD
        git rev-parse HEAD
        git for-each-ref
        git ls-files -s
        find . -path ./.git -prune -o -type f -print | LC_ALL=C sort | xargs sha256sum
        [[ "${2:-}" != objects ]] || find .git/objects -type f | LC_ALL=C sort
    } ) 2>&1
}
commits() { git -C "$1" rev-list --count HEAD; }

SUITE='echo changed >> README && git add -A && git commit --allow-empty -q -m x'

# run_tool <tool> <evidence> [NAME=value]... — sets RC and OUT (the last line).
run_tool() {
    local tool="$1" ev="$2" stdout
    shift 2
    set +e
    stdout="$(env -i HOME="$HOME" TMPDIR="$WORK" PATH="/usr/bin:/bin" \
        GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null "${ID_ENV[@]}" \
        REPO_PATH="$CALLER" AUTODRIVE_ROUND_LABEL=round-1 AUTODRIVE_QA_EVIDENCE="$ev" \
        AUTODRIVE_QA_COMMANDS="$SUITE" "$@" bash "$tool" 2>"$ev.err")"
    RC=$?
    set -e
    OUT="${stdout##*$'\n'}"
}

echo "== #2033: auto-drive QA runs repository tests with the git env cleared =="

# --- Case 1: clean environment -------------------------------------------
ok=1
before_caller="$(commits "$CALLER")"
run_tool "$TOOL" "$WORK/ev1.json"
CLEAN_RC="$RC"
CLEAN_STATUS="$(printf '%s' "$OUT" | field_of qa_status 2>/dev/null || echo '<invalid>')"
[[ "$CLEAN_RC" == 0 ]] || { ok=0; fail clean "exit $CLEAN_RC: $(tail -n 3 "$WORK/ev1.json.err" | tr '\n' '|')"; }
[[ "$CLEAN_STATUS" == PASS ]] || { ok=0; fail clean "qa_status=$CLEAN_STATUS (the suite should commit and pass)"; }
[[ "$(commits "$CALLER")" == $((before_caller + 1)) ]] || { ok=0; fail clean "the caller did not gain one commit"; }
for f in git_env_cleared run_env_cleared; do
    got="$(printf '%s' "$OUT" | field_of "$f" 2>/dev/null || echo '<invalid>')"
    [[ "$got" == none ]] || { ok=0; fail clean "$f=$got, want none"; }
done
[[ "$ok" == 1 ]] && { pass clean "clean run records none and none"; CASES_PASSED=$((CASES_PASSED + 1)); }

# --- Case 2: every git name at the decoy, every run name set ---------------
ok=1
DECOY_ENV=(
    "GIT_ALTERNATE_OBJECT_DIRECTORIES=$DECOY/.git/objects"
    "GIT_CEILING_DIRECTORIES=$WORK"
    "GIT_COMMON_DIR=$DECOY/.git"
    "GIT_DIR=$DECOY/.git"
    "GIT_DISCOVERY_ACROSS_FILESYSTEM=1"
    "GIT_INDEX_FILE=$DECOY/.git/index"
    "GIT_INDEX_VERSION=4"
    "GIT_NAMESPACE=decoy"
    "GIT_OBJECT_DIRECTORY=$DECOY/.git/objects"
    "GIT_PREFIX=sub/"
    "GIT_QUARANTINE_PATH=$DECOY/.git/objects/incoming-decoy"
    "GIT_WORK_TREE=$DECOY"
    "AMPLIHACK_RECIPE_RUN_ID=outer-recipe-run"
    "AMPLIHACK_RUN_DIR=$WORK/outer-run"
    "AMPLIHACK_RUN_ID=outer-run-id"
    "AMPLIHACK_SESSION_TREE_DIR=$WORK/outer-tree"
    "AMPLIHACK_TREE_ID=outer-tree-id"
)
decoy_before="$(snapshot "$DECOY" objects)"
caller_head="$(git -C "$CALLER" rev-parse HEAD)"
before_caller="$(commits "$CALLER")"
run_tool "$TOOL" "$WORK/ev2.json" "${DECOY_ENV[@]}"
status="$(printf '%s' "$OUT" | field_of qa_status 2>/dev/null || echo '<invalid>')"
[[ "$RC" == "$CLEAN_RC" ]] || { ok=0; fail decoy "exit $RC, the clean run exited $CLEAN_RC"; }
[[ "$status" == "$CLEAN_STATUS" ]] || { ok=0; fail decoy "qa_status=$status, the clean run gave $CLEAN_STATUS"; }
head_sha="$(printf '%s' "$OUT" | field_of head_sha 2>/dev/null || echo '<invalid>')"
[[ "$head_sha" == "$caller_head" ]] || { ok=0; fail decoy "head_sha=$head_sha, want the caller's $caller_head"; }
decoy_after="$(snapshot "$DECOY" objects)"
[[ "$decoy_before" == "$decoy_after" ]] \
    || { ok=0; fail decoy "the decoy repository changed: $(diff <(printf '%s\n' "$decoy_before") <(printf '%s\n' "$decoy_after") | head -n 6 | tr '\n' '|')"; }
[[ "$(commits "$CALLER")" == $((before_caller + 1)) ]] || { ok=0; fail decoy "the caller did not gain exactly one commit"; }
got="$(printf '%s' "$OUT" | field_of git_env_cleared 2>/dev/null || echo '<invalid>')"
[[ "$got" == "$GIT_NAMES" ]] || { ok=0; fail decoy "git_env_cleared=<$got>, want <$GIT_NAMES>"; }
got="$(printf '%s' "$OUT" | field_of run_env_cleared 2>/dev/null || echo '<invalid>')"
[[ "$got" == "$RUN_NAMES" ]] || { ok=0; fail decoy "run_env_cleared=<$got>, want <$RUN_NAMES>"; }
[[ "$OUT" != *"$DECOY"* && "$OUT" != *outer-run-id* ]] || { ok=0; fail decoy "the evidence recorded a variable's value, not only its name"; }
[[ "$ok" == 1 ]] && { pass decoy "decoy untouched, caller committed, all 12 and 5 names recorded"; CASES_PASSED=$((CASES_PASSED + 1)); }

# --- Cases 3 and 4: the helper is missing, or a symlink ---------------------
WANT_MSG='ERROR: autodrive-qa-helpers-not-found: autodrive_state.sh, autodrive_trust.sh or git-repo-env.sh is missing'
refusal_case() {
    local id="$1" dir="$WORK/tools-$1" ev="$WORK/ev-$1.json" before
    ok=1
    mkdir -p "$dir"
    cp -R "$TOOLS/." "$dir/"
    rm -f -- "$dir/git-repo-env.sh"
    [[ "$id" != symlink ]] || ln -s "$TOOLS/git-repo-env.sh" "$dir/git-repo-env.sh"
    before="$(snapshot "$CALLER")"
    run_tool "$dir/autodrive_qa_evidence.sh" "$ev"
    [[ "$RC" == 1 ]] || { ok=0; fail "$id" "exit $RC, want 1"; }
    grep -qxF -- "$WANT_MSG" "$ev.err" || { ok=0; fail "$id" "stderr lacks the exact message: $(head -n 3 "$ev.err" | tr '\n' '|')"; }
    [[ ! -e "$ev" ]] || { ok=0; fail "$id" "an evidence file was written"; }
    [[ "$before" == "$(snapshot "$CALLER")" ]] || { ok=0; fail "$id" "the suite ran: the caller changed"; }
    [[ "$ok" == 1 ]] && { pass "$id" "helper $id: refused by name, no evidence, nothing ran"; CASES_PASSED=$((CASES_PASSED + 1)); }
    return 0
}
refusal_case missing
refusal_case symlink

echo
echo "passed checks: $PASS_COUNT, failed checks: $FAIL_COUNT"
if [[ "$FAIL_COUNT" -eq 0 && "$CASES_PASSED" -eq 4 ]]; then
    echo "PASS: 4/4 cases (issue #2033)"
    exit 0
fi
echo "FAIL: ${CASES_PASSED}/4 cases (issue #2033)" >&2
exit 1
