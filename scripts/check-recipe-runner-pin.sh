#!/usr/bin/env bash
# Fail a change that moves claude-plugin/recipe-runner.rev backwards (issue #1593).
#
# Usage, from the root of an amplihack checkout with full history:
#   check-recipe-runner-pin.sh --event <pull_request|merge_group|push> \
#       --base <sha> --head <sha>
#
#   --base   pull_request.base.sha, merge_group.base_sha or the push `before`
#   --head   pull_request.head.sha, merge_group.head_sha or github.sha
#
# The new pin (at --head) must equal, or descend from, the pin it replaces in
# the runner repository. For a pull request two commits are used:
#   - the baseline, merge-base(base, head), decides whether the change touched
#     the pin and is where the trailer search starts, so a pull request that
#     did not touch the pin is not flagged after main's pin moves on;
#   - the comparison commit, the tip of the base branch (--base), supplies the
#     old pin, so a stale branch cannot move the pin behind main's.
# For a merge group or a push, --base serves as both.
#
# An intentional rollback, or a move to a diverged commit, passes only when a
# commit in the range carries a non-empty `Recipe-Runner-Rollback: <reason>`
# trailer in its final paragraph. The trailer is an audit record; review
# remains the control. A malformed head pin, an unknown SHA or an unreachable
# runner repository fail whatever the trailer says.
#
# RECIPE_RUNNER_REPO_URL replaces the runner repository URL (tests only).
# GITHUB_STEP_SUMMARY, when set, receives the outcome.
#
# Exit status: 0 pass, 1 policy failure, 2 usage or environment error.
# See docs/reference/ci-pipeline.md#recipe-runner-pin-ancestry.
set -euo pipefail
export LC_ALL=C

PIN_PATH="claude-plugin/recipe-runner.rev"
TRAILER_KEY="Recipe-Runner-Rollback"
REASON_LIMIT=500
ZEROS="0000000000000000000000000000000000000000"
runner_url="${RECIPE_RUNNER_REPO_URL:-https://github.com/rysweet/amplihack-recipe-runner}"

# Every line is prefixed, so nothing this script prints, including text from
# commit messages, can start a GitHub workflow command ('::').
say() { printf 'recipe-runner-pin: %s\n' "$*"; }
usage() {
    say "error: $*" >&2
    say "usage: check-recipe-runner-pin.sh --event <pull_request|merge_group|push> --base <sha> --head <sha>" >&2
    exit 2
}
fail() {
    say "FAIL: $*" >&2
    summary "### Recipe runner pin ancestry: failed" "" "$(html_escape "$*")"
    exit 1
}
summary() {
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
        printf '%s\n' "$@" >>"$GITHUB_STEP_SUMMARY"
    fi
}
is_sha() { [[ "$1" =~ ^[0-9a-f]{40}$ ]]; }
html_escape() {
    # Replacements are quoted: bash 5.2+ expands an unquoted & in the
    # replacement to the matched text (patsub_replacement). & goes first.
    local s="$1"
    s="${s//&/"&amp;"}"
    s="${s//</"&lt;"}"
    s="${s//>/"&gt;"}"
    s="${s//\`/"&#96;"}"
    s="${s//\[/"&#91;"}"
    s="${s//\]/"&#93;"}"
    s="${s//\(/"&#40;"}"
    s="${s//\)/"&#41;"}"
    s="${s//!/"&#33;"}"
    printf '%s' "$s"
}

# A git command that cannot prompt, run hooks or fsmonitor, use stored
# credentials, or reach a transport other than https and local files.
hgit() {
    env -u GIT_ASKPASS -u SSH_ASKPASS -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE \
        -u GIT_CONFIG_PARAMETERS -u GIT_CONFIG_COUNT \
        GIT_TERMINAL_PROMPT=0 \
        git -c protocol.allow=never -c protocol.https.allow=always \
        -c protocol.file.allow=always -c credential.helper= \
        -c core.fsmonitor=false -c core.hooksPath=/dev/null \
        -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=30 "$@"
}

event=""
base=""
head=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --event | --base | --head)
            [[ $# -ge 2 ]] || usage "$1 needs a value"
            case "$1" in
                --event) event="$2" ;;
                --base) base="$2" ;;
                --head) head="$2" ;;
            esac
            shift 2
            ;;
        *) usage "unknown argument" ;;
    esac
done
case "$event" in
    pull_request | merge_group | push) ;;
    *) usage "--event must be pull_request, merge_group or push" ;;
esac
is_sha "$base" || usage "--base must be a 40-character lowercase SHA"
is_sha "$head" || usage "--head must be a 40-character lowercase SHA"
case "$runner_url" in
    https://* | file://*) ;;
    *) usage "RECIPE_RUNNER_REPO_URL must be an https:// or file:// URL" ;;
esac

# pin_at <commit>: print the pin recorded at <commit> if it is exactly 40
# lowercase hex characters and a newline. Returns 1 when the file is absent
# and 3 when it is malformed.
pin_at() {
    local blob
    hgit cat-file -e "$1:$PIN_PATH" 2>/dev/null || return 1
    # The trailing x keeps a final newline that $(...) would strip.
    blob="$(hgit cat-file blob "$1:$PIN_PATH" 2>/dev/null; printf x)"
    blob="${blob%x}"
    if [[ ${#blob} -ne 41 || "${blob:40:1}" != $'\n' ]] || ! is_sha "${blob:0:40}"; then
        return 3
    fi
    printf '%s' "${blob:0:40}"
}

# raw_pin_at <commit>: the pin file's exact bytes, or a marker when absent.
raw_pin_at() {
    local blob
    if hgit cat-file -e "$1:$PIN_PATH" 2>/dev/null; then
        blob="$(hgit cat-file blob "$1:$PIN_PATH" 2>/dev/null; printf x)"
        printf 'present:%s' "$blob"
    else
        printf 'absent'
    fi
}

hgit cat-file -e "$head^{commit}" 2>/dev/null || usage "--head $head is not a commit in this checkout"

# Revisions compared, by event.
range_start="$base"
baseline="$base"
target_ref="$base"
if [[ "$base" == "$ZEROS" ]]; then
    target_ref=""
fi
if [[ -n "$target_ref" ]]; then
    hgit cat-file -e "$base^{commit}" 2>/dev/null || usage "--base $base is not a commit in this checkout"
    if [[ "$event" == pull_request ]]; then
        baseline="$(hgit merge-base "$base" "$head")" || usage "no merge-base between $base and $head"
        range_start="$baseline"
    fi
fi

# 1. An unchanged pin passes without contacting the runner repository.
if [[ -n "$target_ref" && "$(raw_pin_at "$baseline")" == "$(raw_pin_at "$head")" ]]; then
    say "pin unchanged against $baseline; nothing to check"
    exit 0
fi

# 2. The new pin must be well formed; no override applies.
pin_status=0
new_pin="$(pin_at "$head")" || pin_status=$?
case "$pin_status" in
    0) ;;
    1) fail "$PIN_PATH is missing at $head" ;;
    *) fail "$PIN_PATH at $head must be exactly 40 lowercase hex characters and a newline" ;;
esac

# 3. Without a well-formed pin to replace, there is nothing to order against.
old_pin=""
if [[ -n "$target_ref" ]]; then
    old_pin="$(pin_at "$target_ref")" || old_pin=""
fi
if [[ -z "$old_pin" ]]; then
    say "notice: no comparable base pin; new pin $new_pin accepted"
    summary "### Recipe runner pin ancestry: passed" "" \
        "No comparable base pin; new pin \`$new_pin\` accepted."
    exit 0
fi

# 4. Fetch both runner commits into a private bare repository.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
hgit init --quiet --bare "$work/runner.git" >/dev/null 2>&1 ||
    { say "error: could not create a temporary repository" >&2; exit 2; }
for sha in "$new_pin" "$old_pin"; do
    if ! hgit --git-dir="$work/runner.git" fetch --quiet --no-tags \
        --filter=blob:none -- "$runner_url" "$sha" >/dev/null 2>&1; then
        fail "could not fetch runner commit $sha from the runner repository; the SHA does not exist there or the repository is unreachable"
    fi
done

# 5. Classify the new pin against the one it replaces.
relation=""
if [[ "$new_pin" == "$old_pin" ]]; then
    relation=equal
elif hgit --git-dir="$work/runner.git" merge-base --is-ancestor "$old_pin" "$new_pin" 2>/dev/null; then
    relation=descendant
elif hgit --git-dir="$work/runner.git" merge-base --is-ancestor "$new_pin" "$old_pin" 2>/dev/null; then
    relation=ancestor
else
    relation=diverged
fi
case "$relation" in
    equal | descendant)
        say "new pin $new_pin is $relation of base pin $old_pin"
        exit 0
        ;;
    ancestor) description="new pin $new_pin is an ancestor of base pin $old_pin (a rollback)" ;;
    *) description="new pin $new_pin has diverged from base pin $old_pin" ;;
esac

# 6. A rollback or divergence needs a Recipe-Runner-Rollback trailer in range.
range="$range_start..$head"
trailers="$(hgit log --format="%(trailers:key=$TRAILER_KEY,valueonly)" "$range")" ||
    { say "error: could not read commit messages in $range" >&2; exit 2; }
reasons=()
while IFS= read -r line; do
    line="$(printf '%s' "$line" | tr -d '\000-\037\177')"
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    [[ -n "$line" ]] || continue
    reasons+=("${line:0:$REASON_LIMIT}")
done <<EOF
$trailers
EOF

if [[ ${#reasons[@]} -eq 0 ]]; then
    fail "$description. An intentional rollback needs a trailer in the final paragraph of a commit message in $range, for example: '$TRAILER_KEY: <reason>'"
fi

say "override: $description"
summary "### Recipe runner pin ancestry: rollback override" "" \
    "$(html_escape "$description")." "" "Reasons ($TRAILER_KEY trailers):" ""
for reason in "${reasons[@]}"; do
    say "$TRAILER_KEY: $reason"
    summary "- $(html_escape "$reason")"
done
exit 0
