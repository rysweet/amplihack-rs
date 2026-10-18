#!/usr/bin/env bash
# Tests for scripts/check-recipe-runner-pin.sh and its workflow (issue #1593).
#
# Contract under test:
#   check-recipe-runner-pin.sh --event <pull_request|merge_group|push> \
#       --base <sha> --head <sha>
#   run from the root of an amplihack checkout. --base is
#   pull_request.base.sha, merge_group.base_sha or the push event's `before`;
#   --head is pull_request.head.sha, merge_group.head_sha or github.sha.
#   RECIPE_RUNNER_REPO_URL replaces the runner repository URL (tests only).
#   GITHUB_STEP_SUMMARY, when set, receives the override record.
#   Exit 0 = pass, 1 = policy failure, 2 = usage or environment error.
#
# Every case runs offline: the runner repository is a local fixture reached
# over file://, and the amplihack repository is a local fixture with a
# claude-plugin/recipe-runner.rev file.
#
# Fixture runner history:   A --- B --- C   (main)
#                            \
#                             D             (side)
# C stands in for 4d5187e and B for b4a82e7 (the AC1 rollback).
#
# Run: bash tests/recipe_runner_pin_ancestry_test.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECK="$REPO_ROOT/scripts/check-recipe-runner-pin.sh"
WORKFLOW="$REPO_ROOT/.github/workflows/recipe-runner-pin.yml"
CI_WORKFLOW="$REPO_ROOT/.github/workflows/ci.yml"

pass=0
fail=0
TMPROOT=""

cleanup() { [ -n "$TMPROOT" ] && rm -rf "$TMPROOT"; }
trap cleanup EXIT

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    fail=$((fail + 1))
}

TMPROOT="$(mktemp -d)"

# Git with a fixed identity, no signing and no user or system configuration,
# so a developer's global hooks or signing setup cannot change the fixtures.
fgit() {
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
        GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.invalid \
        GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.invalid \
        git -c commit.gpgsign=false -c init.defaultBranch=main "$@"
}

# ---------------------------------------------------------------------------
# Runner fixture
# ---------------------------------------------------------------------------
RUNNER="$TMPROOT/runner"
mkdir -p "$RUNNER"
fgit -C "$RUNNER" init -q
fgit -C "$RUNNER" config uploadpack.allowFilter true
fgit -C "$RUNNER" config uploadpack.allowAnySHA1InWant true
fgit -C "$RUNNER" commit -q --allow-empty -m A
SHA_A="$(fgit -C "$RUNNER" rev-parse HEAD)"
fgit -C "$RUNNER" commit -q --allow-empty -m B
SHA_B="$(fgit -C "$RUNNER" rev-parse HEAD)"
fgit -C "$RUNNER" commit -q --allow-empty -m C
SHA_C="$(fgit -C "$RUNNER" rev-parse HEAD)"
fgit -C "$RUNNER" checkout -q -b side "$SHA_A"
fgit -C "$RUNNER" commit -q --allow-empty -m D
SHA_D="$(fgit -C "$RUNNER" rev-parse HEAD)"
fgit -C "$RUNNER" checkout -q main
RUNNER_URL="file://$RUNNER"
# A valid-looking SHA that exists nowhere in the runner fixture.
SHA_MISSING="0123456789abcdef0123456789abcdef01234567"
ZEROS="0000000000000000000000000000000000000000"

if [ ! -f "$CHECK" ]; then
    record_fail "scripts/check-recipe-runner-pin.sh exists"
fi

# ---------------------------------------------------------------------------
# Amplihack fixture helpers
# ---------------------------------------------------------------------------
case_n=0
AMP=""

# new_amp <initial pin content or "-" for no pin file>
# Creates a fresh amplihack fixture repository with one commit on main.
new_amp() {
    case_n=$((case_n + 1))
    AMP="$TMPROOT/amp$case_n"
    mkdir -p "$AMP/claude-plugin"
    fgit -C "$AMP" init -q
    printf 'fixture\n' >"$AMP/README"
    if [ "$1" != "-" ]; then
        printf '%s\n' "$1" >"$AMP/claude-plugin/recipe-runner.rev"
    fi
    fgit -C "$AMP" add -A
    fgit -C "$AMP" commit -q -m "initial"
}

# set_pin <content> <message file>: commit a new pin. <content> is written
# verbatim followed by a newline; "-" deletes the pin file.
set_pin() {
    if [ "$1" = "-" ]; then
        rm -f "$AMP/claude-plugin/recipe-runner.rev"
    else
        printf '%s\n' "$1" >"$AMP/claude-plugin/recipe-runner.rev"
    fi
    fgit -C "$AMP" add -A
    fgit -C "$AMP" commit -q -F "$2"
}

# touch_other <message file>: commit that does not touch the pin.
touch_other() {
    printf '%s\n' "$RANDOM" >>"$AMP/README"
    fgit -C "$AMP" add -A
    fgit -C "$AMP" commit -q -F "$1"
}

head_sha() { fgit -C "$AMP" rev-parse HEAD; }

msg() {
    # msg <file> <text...>: write a commit message file.
    local f="$1"
    shift
    printf '%s\n' "$@" >"$f"
}

PLAIN="$TMPROOT/msg-plain"
msg "$PLAIN" "chore: plain change"

TRAILER="$TMPROOT/msg-trailer"
msg "$TRAILER" "fix(runner): roll back while #163 is investigated" "" \
    "Recipe-Runner-Rollback: C breaks Windows path quoting; see runner#163"

EMPTY_TRAILER="$TMPROOT/msg-empty-trailer"
msg "$EMPTY_TRAILER" "fix(runner): roll back" "" "Recipe-Runner-Rollback:    "

BODY_TRAILER="$TMPROOT/msg-body-trailer"
msg "$BODY_TRAILER" "fix(runner): roll back" "" \
    "Recipe-Runner-Rollback: written in the body, not the trailer block" "" \
    "This final paragraph is prose, so the line above is not a trailer."

HOSTILE_TRAILER="$TMPROOT/msg-hostile-trailer"
msg "$HOSTILE_TRAILER" "fix(runner): roll back" "" \
    "Recipe-Runner-Rollback: <img src=x> ::error::boom"

LINK_TRAILER="$TMPROOT/msg-link-trailer"
msg "$LINK_TRAILER" "fix(runner): roll back" "" \
    "Recipe-Runner-Rollback: [details](https://attacker.example)"

IMAGE_TRAILER="$TMPROOT/msg-image-trailer"
msg "$IMAGE_TRAILER" "fix(runner): roll back" "" \
    "Recipe-Runner-Rollback: ![x](https://attacker.example/p.png)"

LONG_REASON="$(printf 'x%.0s' $(seq 1 600))"
LONG_TRAILER="$TMPROOT/msg-long-trailer"
msg "$LONG_TRAILER" "fix(runner): roll back" "" "Recipe-Runner-Rollback: $LONG_REASON"

OUT="$TMPROOT/out"
SUMMARY="$TMPROOT/summary"
STATUS=0

# run_check <event> <base> <head> [url]: run the check from the fixture root.
run_check() {
    local url="${4:-$RUNNER_URL}"
    : >"$SUMMARY"
    (
        cd "$AMP" &&
            RECIPE_RUNNER_REPO_URL="$url" GITHUB_STEP_SUMMARY="$SUMMARY" \
                bash "$CHECK" --event "$1" --base "$2" --head "$3"
    ) >"$OUT" 2>&1
    STATUS=$?
}

expect_status() {
    # expect_status <want> <label>
    if [ "$STATUS" -eq "$1" ]; then
        record_pass "$2 (exit $1)"
    else
        record_fail "$2: expected exit $1, got $STATUS"
        sed 's/^/    | /' "$OUT"
    fi
}

expect_out() {
    # expect_out <fixed string> <label>
    if grep -qF -- "$1" "$OUT"; then
        record_pass "$2"
    else
        record_fail "$2: output lacks '$1'"
        sed 's/^/    | /' "$OUT"
    fi
}

expect_out_ci() {
    # expect_out_ci <fixed string> <label>: case-insensitive.
    if grep -qiF -- "$1" "$OUT"; then
        record_pass "$2"
    else
        record_fail "$2: output lacks '$1' (any case)"
        sed 's/^/    | /' "$OUT"
    fi
}

short() { printf '%s' "${1:0:7}"; }

# ---------------------------------------------------------------------------
# AC1: rollback C -> B on a pull request fails and names both SHAs.
# ---------------------------------------------------------------------------
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC1 pull_request rollback to an ancestor fails"
expect_out "$(short "$SHA_B")" "AC1 message names the new pin"
expect_out "$(short "$SHA_C")" "AC1 message names the base pin"
expect_out_ci "is an ancestor of" "AC1 message states the relationship"
expect_out "Recipe-Runner-Rollback:" "AC1 message gives the trailer syntax"

# ---------------------------------------------------------------------------
# AC2: the same rollback with a trailer passes and records the reason.
# ---------------------------------------------------------------------------
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 0 "AC2 rollback with Recipe-Runner-Rollback trailer passes"
if grep -qF "C breaks Windows path quoting" "$SUMMARY"; then
    record_pass "AC2 reason appears in GITHUB_STEP_SUMMARY"
else
    record_fail "AC2 reason missing from GITHUB_STEP_SUMMARY"
    sed 's/^/    | /' "$SUMMARY"
fi
if grep -qF "$(short "$SHA_B")" "$SUMMARY" && grep -qF "$(short "$SHA_C")" "$SUMMARY"; then
    record_pass "AC2 step summary names both SHAs"
else
    record_fail "AC2 step summary does not name both SHAs"
fi
expect_out "C breaks Windows path quoting" "AC2 reason appears in the log"

# The trailer may sit on any commit in the range, not only the pin commit.
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$PLAIN"
touch_other "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 0 "AC2 trailer on a later commit in the range counts"

# Empty or whitespace-only reasons are not an override.
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$EMPTY_TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC2 whitespace-only trailer reason fails"

# A Recipe-Runner-Rollback line outside the final paragraph is not a trailer.
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$BODY_TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC2 Recipe-Runner-Rollback line outside the trailer block fails"

# A trailer on a commit before the baseline is outside the range.
new_amp "$SHA_C"
touch_other "$TRAILER"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC2 trailer before the baseline does not count"

# ---------------------------------------------------------------------------
# AC3: passing cases.
# ---------------------------------------------------------------------------
# Forward move to a descendant.
new_amp "$SHA_B"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_C" "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 0 "AC3 forward move to a descendant passes"

# Equal: the PR moves the pin to the value main already has.
new_amp "$SHA_A"
MB="$(head_sha)"
set_pin "$SHA_C" "$PLAIN"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr "$MB"
set_pin "$SHA_C" "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 0 "AC3 pin equal to the base pin passes"

# Unchanged pin: the runner repository is never contacted.
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
touch_other "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)" "file://$TMPROOT/nonexistent-runner"
expect_status 0 "AC3 unchanged pin passes with an unreachable runner repository"
expect_out_ci "pin unchanged" "AC3 unchanged pin is reported as unchanged"

# A PR that does not touch the pin while main's pin moved forward. Compared
# with main's tip this looks like C -> B; compared with the merge-base it is
# unchanged, which is the rule.
new_amp "$SHA_B"
MB="$(head_sha)"
set_pin "$SHA_C" "$PLAIN"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr "$MB"
touch_other "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)" "file://$TMPROOT/nonexistent-runner"
expect_status 0 "AC3 PR not touching the pin passes after main's pin moved on"

# Stale branch: the PR was cut when the pin was A and moves it forward to B,
# but main has since moved to C. The pin changed relative to the merge-base,
# so it is compared with main's tip, where B is an ancestor of C. Without a
# trailer this is a rollback of main and fails; with one it passes.
if fgit -C "$RUNNER" merge-base --is-ancestor "$SHA_A" "$SHA_B" &&
    fgit -C "$RUNNER" merge-base --is-ancestor "$SHA_B" "$SHA_C" &&
    ! fgit -C "$RUNNER" merge-base --is-ancestor "$SHA_C" "$SHA_B"; then
    record_pass "fixture: A, B and C form a linear chain"
else
    record_fail "fixture: A, B and C do not form a linear chain"
fi

new_amp "$SHA_A"
MB="$(head_sha)"
set_pin "$SHA_C" "$PLAIN"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr "$MB"
set_pin "$SHA_B" "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "stale branch moving the pin forward to a revision behind main's tip fails"
expect_out "$(printf '%s' "$SHA_C" | cut -c1-7)" "stale branch failure names main's pin"

new_amp "$SHA_A"
MB="$(head_sha)"
set_pin "$SHA_C" "$PLAIN"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr "$MB"
set_pin "$SHA_B" "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 0 "stale branch with a Recipe-Runner-Rollback trailer passes"

# The same stale branch reaching main through the merge queue also fails.
new_amp "$SHA_A"
MB="$(head_sha)"
set_pin "$SHA_C" "$PLAIN"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr "$MB"
set_pin "$SHA_B" "$PLAIN"
fgit -C "$AMP" checkout -q main
fgit -C "$AMP" merge -q --no-ff --no-edit -X theirs pr
run_check merge_group "$BASE" "$(head_sha)"
expect_status 1 "merge_group of a stale branch that moves the pin behind main fails"

# push: forward passes, rollback fails, rollback with trailer passes.
new_amp "$SHA_B"
BEFORE="$(head_sha)"
set_pin "$SHA_C" "$PLAIN"
run_check push "$BEFORE" "$(head_sha)"
expect_status 0 "AC3 push with a forward move passes"

new_amp "$SHA_C"
BEFORE="$(head_sha)"
set_pin "$SHA_B" "$PLAIN"
run_check push "$BEFORE" "$(head_sha)"
expect_status 1 "push rollback without a trailer fails"

new_amp "$SHA_C"
BEFORE="$(head_sha)"
set_pin "$SHA_B" "$TRAILER"
run_check push "$BEFORE" "$(head_sha)"
expect_status 0 "push rollback with a trailer passes"

# push with an all-zero before SHA has no baseline.
new_amp "$SHA_B"
run_check push "$ZEROS" "$(head_sha)"
expect_status 0 "push with an all-zero before passes"
expect_out_ci "no comparable base pin" "all-zero before is reported"

# merge_group: base_sha..head_sha.
new_amp "$SHA_C"
BASE="$(head_sha)"
set_pin "$SHA_B" "$PLAIN"
run_check merge_group "$BASE" "$(head_sha)"
expect_status 1 "merge_group rollback without a trailer fails"

new_amp "$SHA_B"
BASE="$(head_sha)"
set_pin "$SHA_C" "$PLAIN"
run_check merge_group "$BASE" "$(head_sha)"
expect_status 0 "merge_group forward move passes"

# An absent or malformed target pin passes with a notice.
new_amp "-"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 0 "absent base pin passes"
expect_out_ci "no comparable base pin" "absent base pin is reported"

new_amp "not-a-sha"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 0 "malformed base pin passes"
expect_out_ci "no comparable base pin" "malformed base pin is reported"

# ---------------------------------------------------------------------------
# AC4: failures that the override cannot rescue.
# ---------------------------------------------------------------------------
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_D" "$PLAIN"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC4 diverged pin without a trailer fails"
expect_out_ci "diverged" "AC4 diverged failure names the relationship"
expect_out "$(short "$SHA_D")" "AC4 diverged failure names the new pin"

new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_MISSING" "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC4 SHA missing from the runner repository fails despite a trailer"
expect_out "$(short "$SHA_MISSING")" "AC4 missing-SHA failure names the SHA"

new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "${SHA_B:0:39}" "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC4 malformed (39-char) head pin fails despite a trailer"

new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
printf '%s' "$SHA_B" >"$AMP/claude-plugin/recipe-runner.rev"
fgit -C "$AMP" add -A
fgit -C "$AMP" commit -q -F "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC4 head pin without a trailing newline fails"

new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
printf '%s\n%s\n' "$SHA_B" "$SHA_C" >"$AMP/claude-plugin/recipe-runner.rev"
fgit -C "$AMP" add -A
fgit -C "$AMP" commit -q -F "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC4 head pin with two lines fails"

new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "-" "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 1 "AC4 missing head pin fails despite a trailer"

new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)" "file://$TMPROOT/nonexistent-runner"
expect_status 1 "AC4 unreachable runner repository fails despite a trailer"

# ---------------------------------------------------------------------------
# Usage errors (exit 2).
# ---------------------------------------------------------------------------
new_amp "$SHA_C"
GOOD="$(head_sha)"
MARKER="$TMPROOT/upload-pack-ran"

run_usage() {
    (cd "$AMP" && RECIPE_RUNNER_REPO_URL="$RUNNER_URL" bash "$CHECK" "$@") >"$OUT" 2>&1
    STATUS=$?
}

run_usage --event pull_request --base "$GOOD" --head "--upload-pack=touch $MARKER"
expect_status 2 "option-shaped head SHA is rejected"
if [ -e "$MARKER" ]; then
    record_fail "option-shaped head SHA ran a command"
else
    record_pass "option-shaped head SHA ran no command"
fi

run_usage --event pull_request --base "$GOOD" --head "${GOOD:0:39}"
expect_status 2 "39-character head SHA is rejected"

UPPER="$(printf '%s' "$GOOD" | tr '[:lower:]' '[:upper:]')"
run_usage --event pull_request --base "$GOOD" --head "$UPPER"
expect_status 2 "uppercase head SHA is rejected"

run_usage --event pull_request --base "${GOOD:0:39}" --head "$GOOD"
expect_status 2 "39-character base SHA is rejected"

run_usage --event pull_request_target --base "$GOOD" --head "$GOOD"
expect_status 2 "unknown event is rejected"

run_usage --event push --base "$GOOD" --head "$GOOD" --surprise
expect_status 2 "unknown argument is rejected"

run_usage --event push --base "$GOOD"
expect_status 2 "missing --head is rejected"

# ---------------------------------------------------------------------------
# Hostile inputs.
# ---------------------------------------------------------------------------
# An ext:: URL must never run a command.
EXT_MARKER="$TMPROOT/ext-ran"
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$TRAILER"
run_check pull_request "$BASE" "$(head_sha)" "ext::sh -c touch% $EXT_MARKER"
if [ "$STATUS" -ne 0 ]; then
    record_pass "ext:: runner URL does not pass the check"
else
    record_fail "ext:: runner URL passed the check"
fi
if [ -e "$EXT_MARKER" ]; then
    record_fail "ext:: runner URL ran a command"
else
    record_pass "ext:: runner URL ran no command"
fi

# Trailer text is escaped in the summary and cannot inject workflow commands.
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$HOSTILE_TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 0 "hostile trailer still counts as an override"
if grep -qF "<img src=x>" "$SUMMARY"; then
    record_fail "hostile trailer HTML reached the step summary unescaped"
    sed 's/^/    | /' "$SUMMARY"
else
    record_pass "hostile trailer HTML is escaped in the step summary"
fi
if grep -qF "&lt;img src=x&gt;" "$SUMMARY"; then
    record_pass "hostile trailer HTML appears as entities in the step summary"
else
    record_fail "hostile trailer HTML is not entity-encoded in the step summary"
    sed 's/^/    | /' "$SUMMARY"
fi
if grep -q '^::' "$OUT"; then
    record_fail "a log line starts with '::' (workflow command injection)"
    sed 's/^/    | /' "$OUT"
else
    record_pass "no log line starts with '::'"
fi

# Markdown links and images in a reason are not rendered in the summary.
for case in "link:$LINK_TRAILER" "image:$IMAGE_TRAILER"; do
    kind="${case%%:*}"
    new_amp "$SHA_C"
    BASE="$(head_sha)"
    fgit -C "$AMP" checkout -q -b pr
    set_pin "$SHA_B" "${case#*:}"
    run_check pull_request "$BASE" "$(head_sha)"
    expect_status 0 "Markdown $kind trailer still counts as an override"
    if grep -qF "](" "$SUMMARY"; then
        record_fail "Markdown $kind in a trailer reached the step summary"
        sed 's/^/    | /' "$SUMMARY"
    else
        record_pass "Markdown $kind in a trailer is escaped in the step summary"
    fi
done

# Reasons are capped at 500 bytes.
new_amp "$SHA_C"
BASE="$(head_sha)"
fgit -C "$AMP" checkout -q -b pr
set_pin "$SHA_B" "$LONG_TRAILER"
run_check pull_request "$BASE" "$(head_sha)"
expect_status 0 "long trailer still counts as an override"
if grep -qF "$LONG_REASON" "$SUMMARY" "$OUT"; then
    record_fail "600-byte reason was not capped"
else
    record_pass "600-byte reason is capped"
fi

# The temporary bare repository is removed.
LEFTOVER_TMP="$TMPROOT/tmpdir"
mkdir -p "$LEFTOVER_TMP"
new_amp "$SHA_B"
BASE="$(head_sha)"
set_pin "$SHA_C" "$PLAIN"
(
    cd "$AMP" &&
        TMPDIR="$LEFTOVER_TMP" RECIPE_RUNNER_REPO_URL="$RUNNER_URL" \
            bash "$CHECK" --event push --base "$BASE" --head "$(fgit -C "$AMP" rev-parse HEAD)"
) >"$OUT" 2>&1
STATUS=$?
expect_status 0 "forward move with a private TMPDIR passes"
if [ -z "$(ls -A "$LEFTOVER_TMP")" ]; then
    record_pass "temporary repository is removed"
else
    record_fail "temporary files left in TMPDIR: $(ls -A "$LEFTOVER_TMP")"
fi

# ---------------------------------------------------------------------------
# AC5: workflow wiring.
# ---------------------------------------------------------------------------
wf_has() {
    # wf_has <extended regex> <label>
    if [ -f "$WORKFLOW" ] && grep -qE -- "$1" "$WORKFLOW"; then
        record_pass "$2"
    else
        record_fail "$2"
    fi
}

wf_lacks() {
    if [ -f "$WORKFLOW" ] && ! grep -qE -- "$1" "$WORKFLOW"; then
        record_pass "$2"
    else
        record_fail "$2"
    fi
}

wf_has '^  pull_request:' "workflow triggers on pull_request"
wf_has '^  merge_group:' "workflow triggers on merge_group"
wf_has '^  push:' "workflow triggers on push"
wf_lacks 'pull_request_target' "workflow never uses pull_request_target"
wf_has '^  recipe-runner-pin:' "job id is recipe-runner-pin"
wf_has '^    name: Recipe runner pin ancestry$' "job name is 'Recipe runner pin ancestry'"
wf_has '^name: Recipe runner pin$' "workflow name is 'Recipe runner pin'"
wf_has 'fetch-depth: 0' "checkout uses fetch-depth: 0"
wf_has 'persist-credentials: false' "checkout uses persist-credentials: false"
wf_has 'contents: read' "permissions are contents: read"
wf_lacks 'secrets\.' "workflow uses no secrets"
wf_has 'timeout-minutes: 5' "job timeout is 5 minutes"
wf_has 'group: .*github\.workflow.*github\.event_name.*github\.ref' \
    "concurrency group includes workflow, event name and ref"
wf_has "cancel-in-progress: \\$\\{\\{ *github\\.event_name != 'push' *\\}\\}" \
    "push runs are never cancelled"
wf_has 'scripts/check-recipe-runner-pin\.sh' "workflow runs the check script"
wf_lacks 'uses: [^@]+@v[0-9]' "actions are pinned by SHA, not by tag"

# No ${{ }} inside a run: block; event values reach the step through env:.
if [ -f "$WORKFLOW" ]; then
    run_exprs="$(awk '
        /^[[:space:]]*run:/ { inrun = 1; indent = match($0, /[^ ]/); if ($0 ~ /\$\{\{/) print FNR": "$0; next }
        inrun { cur = match($0, /[^ ]/); if (cur > 0 && cur <= indent) inrun = 0 }
        inrun && /\$\{\{/ { print FNR": "$0 }
    ' "$WORKFLOW")"
    if [ -z "$run_exprs" ]; then
        record_pass "no \${{ }} expression inside a run: block"
    else
        record_fail "\${{ }} expression inside a run: block: $run_exprs"
    fi
fi

# The shell tests are wired into ci.yml's check job.
for t in recipe_runner_pin_ancestry_test.sh install_pinned_runner_downgrade_guard_test.sh \
    runner_ancestry_lib_parity_test.sh; do
    if grep -qF "tests/$t" "$CI_WORKFLOW"; then
        record_pass "ci.yml runs tests/$t"
    else
        record_fail "ci.yml does not run tests/$t"
    fi
done

# Issue references: this work is tracked by #1593. A bare "#161" in the files
# below would be read as an amplihack-rs issue; the runner fixes are written
# as rysweet/amplihack-recipe-runner#161 instead. This file is not scanned
# because it states the rule.
bare_161="$(grep -nE '(^|[^/A-Za-z0-9_-])#161([^0-9a-fA-F]|$)' \
    "$CHECK" "$WORKFLOW" "$CI_WORKFLOW" \
    "$REPO_ROOT/scripts/install-pinned-recipe-runner.sh" \
    "$REPO_ROOT/scripts/lib/runner-ancestry.sh" \
    "$REPO_ROOT/claude-plugin/bin/install-runtime" \
    "$REPO_ROOT/crates/amplihack-cli/src/runner_ancestry.rs" \
    "$REPO_ROOT/crates/amplihack-cli/src/freshness.rs" \
    "$REPO_ROOT/crates/amplihack-cli/src/commands/install/recipe_runner.rs" \
    "$REPO_ROOT/tests/install_pinned_runner_downgrade_guard_test.sh" \
    "$REPO_ROOT/tests/runner_ancestry_lib_parity_test.sh" \
    "$REPO_ROOT/docs/reference/ci-pipeline.md" \
    "$REPO_ROOT/docs/reference/recipe-runner-validation.md" \
    "$REPO_ROOT/docs/reference/environment-variables.md" 2>/dev/null || true)"
if [ -z "$bare_161" ]; then
    record_pass "no bare #161 reference in the pin check, installers, tests or docs"
else
    record_fail "bare #161 references remain (use #1593, or rysweet/amplihack-recipe-runner#161):"
    printf '%s\n' "$bare_161" | sed 's/^/    | /'
fi

# The job name is what branch protection requires; it must not change.
if grep -qE '^[[:space:]]+name: Recipe runner pin ancestry[[:space:]]*$' "$WORKFLOW"; then
    record_pass "workflow job is still named 'Recipe runner pin ancestry'"
else
    record_fail "workflow job name 'Recipe runner pin ancestry' changed or missing"
fi

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
