#!/usr/bin/env bash
# TDD tests for issue #1089 — merge-flow docs must NOT recommend disabling
# strict branch protection.
#
# Background (verified): docs/reference/merge-flow.md previously recommended
# turning strict up-to-date branch protection OFF via a
# `gh api ... required_status_checks -F strict=false` command block. Autonomous
# agent sessions read this doc and one used it to actually disable protection on
# `main`. The repository owner has REJECTED disabling strict protection.
#
# Owner decision (authoritative):
#   - Strict up-to-date branch protection on `main` stays ON, with all 7
#     required checks (Lint & Format, Test, Install Smoke Test, and the four
#     Build <target> legs).
#   - PRs merge SERIALLY, each preceded by an up-to-date CI run, then squash.
#   - No admin merges (`gh pr merge --admin`), no `--no-verify`.
#   - A GitHub merge queue is organization-only and unavailable on this
#     user-owned repo — a fact to document, but strict-off is REJECTED.
#
# Contract for docs/reference/merge-flow.md:
#   1. Documents strict up-to-date protection + all 7 required checks staying ON
#      as intentional and to not be turned off.
#   2. Explains the merge queue is unavailable because it is organization-only
#      and this repo is user-owned, keeping the GitHub merge-queue docs link.
#   3. Describes the serial flow: green AND up to date with `main` before
#      merging; update + re-run CI if `main` advanced; then squash-merge.
#   4. Contains NO `strict=false` command block / recommendation.
#   5. Contains NO `strict=true` rollback block, NO `--auto` /
#      `allow_auto_merge` auto-merge guidance.
#   6. Forbids `--admin` and `--no-verify`.
#   7. Contains NO "(be honest about it)" / "be honest" phrasing.
#   8. Keeps the Home > Reference > Merge Flow breadcrumb and valid internal
#      links (ci-pipeline.md, ../index.md).
#
# Contract for docs/index.md:
#   9. The Merge Flow link summary matches the corrected content and contains no
#      strict-off ("strict=false", "disable"/"turn off" protection) language.
#
# Invariant:
#  10. This is a docs-only change — no branch-protection settings changed from
#      code, no source/workflow files modified.
#
# Run: bash tests/issue_1089_merge_flow_docs_test.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOCS="$REPO_ROOT/docs"
MERGE_FLOW="$DOCS/reference/merge-flow.md"
INDEX="$DOCS/index.md"
CI_PIPELINE="$DOCS/reference/ci-pipeline.md"

pass=0
fail=0

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    if [ $# -gt 1 ] && [ -n "$2" ]; then
        printf '      %s\n' "$2"
    fi
    fail=$((fail + 1))
}

# assert_true DESC CMD...  -> passes when CMD exits 0
assert_true() {
    local desc="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        record_pass "$desc"
    else
        record_fail "$desc" "expected success: $*"
    fi
}

# assert_false DESC CMD...  -> passes when CMD exits non-zero
assert_false() {
    local desc="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        record_fail "$desc" "expected no match: $*"
    else
        record_pass "$desc"
    fi
}

echo "=== issue #1089 merge-flow docs TDD tests ==="
echo "Merge flow: $MERGE_FLOW"
echo "Index:      $INDEX"
echo

# ---------------------------------------------------------------------------
# 0. Files exist
# ---------------------------------------------------------------------------
assert_true "docs/reference/merge-flow.md exists" test -f "$MERGE_FLOW"
assert_true "docs/index.md exists" test -f "$INDEX"

# ---------------------------------------------------------------------------
# 1. Strict up-to-date protection + 7 checks stay ON, intentionally
# ---------------------------------------------------------------------------
assert_true "documents 7 required status checks" \
    grep -Eiq "7 required status checks" "$MERGE_FLOW"
assert_true "documents up-to-date (strict) requirement staying on" \
    grep -Eiq "up[ -]to[ -]date" "$MERGE_FLOW"
assert_true "states strict protection stays on / intentional / must not be turned off" \
    grep -Eiq "must not be turned off|stays \*\*on\*\*|intentional" "$MERGE_FLOW"

# ---------------------------------------------------------------------------
# 2. Merge queue unavailable: organization-only, user-owned repo, link kept
# ---------------------------------------------------------------------------
assert_true "explains merge queue is organization-only" \
    grep -Eiq "organization" "$MERGE_FLOW"
assert_true "explains this repo is user/personal-owned" \
    grep -Eiq "personal user account|user-owned|personal account" "$MERGE_FLOW"
assert_true "keeps GitHub merge-queue docs link" \
    grep -Fq "managing-a-merge-queue" "$MERGE_FLOW"

# ---------------------------------------------------------------------------
# 3. Serial green-and-up-to-date squash-merge flow
# ---------------------------------------------------------------------------
assert_true "documents serial / one-at-a-time merging" \
    grep -Eiq "serial|one at a time" "$MERGE_FLOW"
assert_true "documents squash-merge command" \
    grep -Eq "gh pr merge <number> --squash" "$MERGE_FLOW"
assert_true "documents re-running CI when main advances" \
    grep -Eiq "update the branch|run again|re-?run" "$MERGE_FLOW"

# ---------------------------------------------------------------------------
# 4. NO strict=false recommendation / command block
# ---------------------------------------------------------------------------
assert_false "no strict=false anywhere in merge-flow.md" \
    grep -Eiq "strict=false|strict[[:space:]]*=[[:space:]]*false" "$MERGE_FLOW"
assert_false "no required_status_checks strict-off api block" \
    grep -Eiq "required_status_checks.*strict" "$MERGE_FLOW"

# ---------------------------------------------------------------------------
# 5. NO strict=true rollback, NO auto-merge guidance
# ---------------------------------------------------------------------------
assert_false "no strict=true rollback block" \
    grep -Eiq "strict=true" "$MERGE_FLOW"
assert_false "no allow_auto_merge guidance" \
    grep -Eiq "allow_auto_merge" "$MERGE_FLOW"
assert_false "no gh pr merge --auto guidance" \
    grep -Eq -- "--auto" "$MERGE_FLOW"

# ---------------------------------------------------------------------------
# 6. Forbids --admin and --no-verify
# ---------------------------------------------------------------------------
assert_true "forbids gh pr merge --admin" \
    grep -Eq -- "--admin" "$MERGE_FLOW"
assert_true "forbids --no-verify" \
    grep -Eq -- "--no-verify" "$MERGE_FLOW"
assert_true "prohibition framed as 'do not' use those flags" \
    grep -Eiq "do \*\*not\*\*|do not use" "$MERGE_FLOW"

# ---------------------------------------------------------------------------
# 7. NO "be honest" phrasing
# ---------------------------------------------------------------------------
assert_false "no 'be honest about it' phrasing" \
    grep -Eiq "be honest" "$MERGE_FLOW"

# ---------------------------------------------------------------------------
# 8. Breadcrumb + valid internal links
# ---------------------------------------------------------------------------
assert_true "keeps Home > Reference > Merge Flow breadcrumb" \
    grep -Fq "[Home](../index.md) > Reference > Merge Flow" "$MERGE_FLOW"
assert_true "ci-pipeline.md link target exists" test -f "$CI_PIPELINE"
assert_true "links to ci-pipeline.md" \
    grep -Fq "ci-pipeline.md" "$MERGE_FLOW"

# Verify every relative .md link in merge-flow.md resolves to a real file.
link_ok=1
missing=""
while IFS= read -r target; do
    # strip anchors / query
    file="${target%%#*}"
    [ -z "$file" ] && continue
    case "$file" in
        http://*|https://*|mailto:*) continue ;;
    esac
    resolved="$(cd "$(dirname "$MERGE_FLOW")" && cd "$(dirname "$file")" 2>/dev/null && pwd)/$(basename "$file")"
    if [ ! -f "$resolved" ]; then
        link_ok=0
        missing="$missing $file"
    fi
done < <(grep -oE '\]\([^)]+\)' "$MERGE_FLOW" | sed -E 's/^\]\(//; s/\)$//')
if [ "$link_ok" -eq 1 ]; then
    record_pass "all relative links in merge-flow.md resolve"
else
    record_fail "all relative links in merge-flow.md resolve" "missing:$missing"
fi

# ---------------------------------------------------------------------------
# 9. index.md summary matches, no strict-off language
# ---------------------------------------------------------------------------
assert_true "index.md links to reference/merge-flow.md" \
    grep -Fq "reference/merge-flow.md" "$INDEX"
INDEX_LINE="$(grep -F "reference/merge-flow.md" "$INDEX" | head -n1)"
if grep -Fq "reference/merge-flow.md" "$INDEX"; then
    if printf '%s' "$INDEX_LINE" | grep -Eiq "strict=false|disabl|turn(ing)? off"; then
        record_fail "index.md merge-flow summary has no strict-off language" \
            "line: $INDEX_LINE"
    else
        record_pass "index.md merge-flow summary has no strict-off language"
    fi
    if printf '%s' "$INDEX_LINE" | grep -Eiq "stays on|up[ -]to[ -]date|serial|squash"; then
        record_pass "index.md merge-flow summary matches corrected content"
    else
        record_fail "index.md merge-flow summary matches corrected content" \
            "line: $INDEX_LINE"
    fi
fi

# ---------------------------------------------------------------------------
# 10. Docs-only invariant vs base branch (no code/settings changes)
# ---------------------------------------------------------------------------
base_ref=""
if git -C "$REPO_ROOT" rev-parse --verify -q origin/main >/dev/null 2>&1; then
    base_ref="origin/main"
elif git -C "$REPO_ROOT" rev-parse --verify -q main >/dev/null 2>&1; then
    base_ref="main"
fi

if [ -n "$base_ref" ]; then
    changed="$(git -C "$REPO_ROOT" diff --name-only "$base_ref"...HEAD 2>/dev/null)"
    # Only docs/** and this test file are permitted to change.
    offending="$(printf '%s\n' "$changed" \
        | grep -vE '^docs/|^tests/issue_1089_merge_flow_docs_test\.sh$' \
        | grep -vE '^$' || true)"
    if [ -z "$offending" ]; then
        record_pass "change is docs-only (no code/settings files modified)"
    else
        record_fail "change is docs-only (no code/settings files modified)" \
            "unexpected changed files: $(printf '%s' "$offending" | tr '\n' ' ')"
    fi
else
    record_pass "base branch unavailable; docs-only diff check skipped"
fi

echo
echo "=== Results: $pass passed, $fail failed ==="
if [ "$fail" -ne 0 ]; then
    exit 1
fi
