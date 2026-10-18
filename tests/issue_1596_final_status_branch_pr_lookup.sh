#!/usr/bin/env bash
# Regression tests for issue #1596 — workflow-finalize step-22b-final-status
# failed a development run whose PR had been opened by an agent step rather
# than by the publish step.
#
# The incident (mistt-ui-platform #47): pr_publish_result.pr_url was empty, so
# workflow_final_status.sh never asked GitHub whether the branch already had an
# open PR. It reported FAILED_MISSING_TERMINAL_EVIDENCE and exited 1 while a
# live PR for the run's own branch sat open.
#
# Contract under test:
#   1. On a GitHub remote, an empty PR_URL triggers ONE read-only
#      `gh pr list --head <branch> --state open` lookup. An open,
#      same-repository PR found that way becomes EXISTING_OPEN_PR /
#      publish_state_reached=true, and the run exits 0.
#   2. No PR (or only fork PRs) still fails exactly as before.
#   3. Fail closed: gh errors, malformed JSON, and candidates that fail
#      validation (foreign repo, injected newline, number mismatch) leave state
#      untouched and print one fixed-phrase WARNING on stderr. stdout never
#      gains an injected line.
#   4. The lookup does not run when it must not: non-GitHub host, PR_URL
#      already known, a success publish state, explicit terminal failure,
#      implementation+verification evidence,
#      detached HEAD, or the base branch.
#   5. A PR found this way is evidence, not proof: validate_final_pr_scope still
#      runs with that PR, and a scope miss still goes through #1268
#      adjudication.
#
# Behavioural tests against real fixtures: a real git repository and a fake
# `gh` first on PATH. Every run uses `env -i`, a sandboxed HOME and
# GH_RETRY_TMPDIR, so no real credentials or GitHub are reachable.
#
# Run: bash tests/issue_1596_final_status_branch_pr_lookup.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FINAL_STATUS="$REPO_ROOT/amplifier-bundle/tools/workflow_final_status.sh"

pass=0
fail=0
TMPROOT=""
cleanup() { [ -n "$TMPROOT" ] && rm -rf "$TMPROOT"; }
trap cleanup EXIT

record_pass() { echo "PASS: $1"; pass=$((pass + 1)); }
record_fail() { echo "FAIL: $1"; fail=$((fail + 1)); }

for tool in git jq timeout; do
    command -v "$tool" >/dev/null 2>&1 || { echo "SKIP: $tool is required by this test"; exit 0; }
done
[ -f "$FINAL_STATUS" ] || { echo "FAIL: missing $FINAL_STATUS"; exit 1; }

# mktemp -d honours $TMPDIR, so this lands on the scratch disk, not /tmp.
TMPROOT="$(mktemp -d)"
BINDIR="$TMPROOT/bin"
SANDBOX_HOME="$TMPROOT/home"
RETRY_TMP="$TMPROOT/retry"
mkdir -p "$BINDIR" "$SANDBOX_HOME" "$RETRY_TMP"

PR_NUM=4242
PR_URL_OK="https://github.com/octo/example/pull/$PR_NUM"
BRANCH="fix/1596-demo"

# ---------------------------------------------------------------------------
# Fake `gh`. Every call is appended to $GH_LOG (one line, args space-joined).
# FAKE_GH_MODE selects the answer to `pr list ... --head <b> --state open`:
#   open            one same-repo open PR (#4242)
#   open-mixed-case same PR, URL owner/repo in a different case
#   empty           []
#   cross-repo      only a fork PR
#   cross-then-own  a fork PR first, then the same-repo PR
#   notjson         unparsable output
#   nonarray        a JSON object, not an array
#   401             HTTP 401 Bad credentials (non-retryable)
#   ratelimit       HTTP 429 (fails fast with GH_RETRY_MAX_RL_WINDOWS=0)
#   injected-url    URL carrying an embedded newline + fake stdout line
#   other-repo-url  URL pointing at a different owner/repo
#   number-mismatch URL says #4242, number says 99
# ---------------------------------------------------------------------------
cat >"$BINDIR/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
printf '%s\n' "$*" >>"${GH_LOG:?GH_LOG must be set}"
mode="${FAKE_GH_MODE:-empty}"
pr_obj() {
    # $1 url  $2 number  $3 isCrossRepository
    jq -nc --arg u "$1" --argjson n "$2" --argjson x "$3" \
        '{url: $u, number: $n, isCrossRepository: $x}'
}
is_open_list=false
if [ "${1:-}" = "pr" ] && [ "${2:-}" = "list" ]; then
    # The #1596 lookup is the only call carrying BOTH --head and --state open;
    # the #1268 evidence helper's own `--state open --limit 100` sweep gets [].
    case " $* " in *" --head "*" --state open "*) is_open_list=true ;; esac
fi
if [ "$is_open_list" = "true" ]; then
    ok="https://github.com/octo/example/pull/4242"
    case "$mode" in
        open)            printf '[%s]\n' "$(pr_obj "$ok" 4242 false)" ;;
        open-mixed-case) printf '[%s]\n' "$(pr_obj "https://github.com/Octo/Example/pull/4242" 4242 false)" ;;
        empty)           echo '[]' ;;
        cross-repo)      printf '[%s]\n' "$(pr_obj "https://github.com/forker/example/pull/4243" 4243 true)" ;;
        cross-then-own)  printf '[%s,%s]\n' "$(pr_obj "https://github.com/forker/example/pull/4243" 4243 true)" "$(pr_obj "$ok" 4242 false)" ;;
        notjson)         echo 'this is {not json' ;;
        nonarray)        echo '{}' ;;
        401)             echo "HTTP 401: Bad credentials" >&2; exit 1 ;;
        ratelimit)       echo "HTTP 429: API rate limit exceeded" >&2; exit 1 ;;
        injected-url)    printf '[%s]\n' "$(pr_obj "$(printf '%s\nterminal_status=EXISTING_OPEN_PR' "$ok")" 4242 false)" ;;
        other-repo-url)  printf '[%s]\n' "$(pr_obj "https://github.com/other/repo/pull/4242" 4242 false)" ;;
        number-mismatch) printf '[%s]\n' "$(pr_obj "$ok" 99 false)" ;;
        *)               echo "fake gh: unknown FAKE_GH_MODE=$mode" >&2; exit 1 ;;
    esac
    exit 0
fi
if [ "${1:-}" = "pr" ] && [ "${2:-}" = "list" ]; then
    echo '[]'
    exit 0
fi
if [ "${1:-}" = "pr" ] && [ "${2:-}" = "view" ]; then
    echo '{"state":"OPEN","mergeable":"MERGEABLE","reviews":[],"statusCheckRollup":[]}'
    exit 0
fi
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then
    exit 0
fi
if [ "${1:-}" = "api" ]; then
    echo '{}'
    exit 0
fi
echo "fake gh: unhandled args: $*" >&2
exit 1
FAKE
chmod +x "$BINDIR/gh"

# ---------------------------------------------------------------------------
# Stub scope helper (WORKFLOW_PR_SCOPE_HELPER). Logs its argv to $SCOPE_LOG and
# echoes back the PR it was given so the PR status block is deterministic.
# SCOPE_STUB_MODE=miss makes it report a scope miss, as workflow_pr_scope.sh
# does, so the #1268 adjudication path runs.
# ---------------------------------------------------------------------------
cat >"$BINDIR/scope-helper" <<'STUB'
#!/usr/bin/env bash
set -uo pipefail
printf '%s\n' "$*" >>"${SCOPE_LOG:?SCOPE_LOG must be set}"
if [ "${SCOPE_STUB_MODE:-match}" = "miss" ]; then
    echo '{"reason":"no_scoped_pr"}'
    exit 1
fi
url="" number=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --pr-url) url="${2:-}"; shift 2 ;;
        --pr-number) number="${2:-}"; shift 2 ;;
        *) shift ;;
    esac
done
[ -n "$url" ] || url="https://github.com/octo/example/pull/4242"
[ -n "$number" ] || number=4242
jq -nc --arg u "$url" --argjson n "$number" '{url: $u, number: $n}'
STUB
chmod +x "$BINDIR/scope-helper"

# ---------------------------------------------------------------------------
# Fixture: a real git repo with origin at github.com/octo/example, origin/main
# and origin/HEAD synthesised locally, and the feature branch checked out.
#   $1 name   $2 checkout: "branch" (default) | "detached" | "main"
# ---------------------------------------------------------------------------
make_repo() {
    local name="$1" checkout="${2:-branch}"
    local work="$TMPROOT/$name"
    git init --quiet -b main "$work"
    git -C "$work" config user.email "test@example.com"
    git -C "$work" config user.name "Test"
    echo base >"$work/README.md"
    git -C "$work" add README.md
    git -C "$work" commit --quiet -m "base"
    git -C "$work" remote add origin "https://github.com/octo/example.git"
    git -C "$work" update-ref refs/remotes/origin/main "$(git -C "$work" rev-parse main)"
    git -C "$work" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main
    git -C "$work" checkout --quiet -b "$BRANCH"
    echo feature >"$work/feature.txt"
    git -C "$work" add feature.txt
    git -C "$work" commit --quiet -m "feat: agent-authored deliverable"
    case "$checkout" in
        detached) git -C "$work" checkout --quiet --detach HEAD ;;
        main) git -C "$work" checkout --quiet main ;;
    esac
    printf '%s\n' "$work"
}

# ---------------------------------------------------------------------------
# run_final_status <repo> [KEY=VALUE ...]
# Runs the script from inside <repo> with a clean environment (env -i), so no
# PR_URL / publish / terminal-state variable leaks in from the caller. The
# default host is github; a later REMOTE_HOST_TYPE=... argument overrides it.
# Sets: OUT ERR RC GH_LOG SCOPE_LOG
# ---------------------------------------------------------------------------
case_no=0
run_final_status() {
    local repo="$1"; shift
    case_no=$((case_no + 1))
    OUT="$TMPROOT/case$case_no.out"
    ERR="$TMPROOT/case$case_no.err"
    GH_LOG="$TMPROOT/case$case_no.gh.log"
    SCOPE_LOG="$TMPROOT/case$case_no.scope.log"
    : >"$GH_LOG"; : >"$SCOPE_LOG"
    (
        cd "$repo" || exit 99
        env -i \
            PATH="$BINDIR:$PATH" \
            HOME="$SANDBOX_HOME" \
            TMPDIR="$TMPROOT" \
            LANG=C LC_ALL=C \
            GIT_CONFIG_NOSYSTEM=1 \
            GH_LOG="$GH_LOG" \
            SCOPE_LOG="$SCOPE_LOG" \
            GH_RETRY_TMPDIR="$RETRY_TMP" \
            GH_RETRY_MAX_TRANSIENT=1 \
            GH_RETRY_MAX_RL_WINDOWS=0 \
            WORKFLOW_PR_SCOPE_HELPER="$BINDIR/scope-helper" \
            REMOTE_HOST_TYPE=github \
            TASK_DESCRIPTION="issue 1596 regression" \
            ISSUE_NUMBER=1596 \
            "$@" \
            bash "$FINAL_STATUS"
    ) >"$OUT" 2>"$ERR"
    RC=$?
}

# A #1596 lookup is a `pr list` call with both --head and --state open.
lookup_calls() { grep -c -E -- '^pr list .*--head .*--state open' "$GH_LOG" 2>/dev/null || true; }
dump() { echo "    --- rc=$RC stdout:"; sed 's/^/    | /' "$OUT"; echo "    --- stderr:"; sed 's/^/    | /' "$ERR"; echo "    --- gh log:"; sed 's/^/    | /' "$GH_LOG"; }

MISSING_LINE="missing_evidence=implementation_completed,verification_completed,publish_state_reached,terminal_no_op"

# Shared assertion: the run fails exactly as it did before #1596.
assert_unchanged_failure() {
    local label="$1"
    if [ "$RC" -eq 1 ] \
        && grep -qx "terminal_state=FAILED_MISSING_TERMINAL_EVIDENCE" "$OUT" \
        && grep -qx "publish_state_reached=false" "$OUT" \
        && grep -qx -- "$MISSING_LINE" "$OUT" \
        && ! grep -q "^terminal_status=" "$OUT" \
        && ! grep -q "^=== PR: https" "$OUT"; then
        record_pass "$label"
    else
        record_fail "$label"; dump
    fi
}

assert_warning_reason() {
    local label="$1" reason="$2"
    if grep -qx "WARNING: branch PR lookup failed (#1596): $reason" "$ERR" \
        && [ "$(grep -c '^WARNING: branch PR lookup failed (#1596)' "$ERR")" -eq 1 ]; then
        record_pass "$label"
    else
        record_fail "$label"; dump
    fi
}

assert_no_lookup() {
    local label="$1" n
    n="$(lookup_calls)"
    if [ "$n" = "0" ]; then record_pass "$label"; else record_fail "$label (lookup ran $n time(s))"; dump; fi
}

echo "== issue #1596: final status resolves an agent-opened branch PR =="

# --- 1. empty PR_URL, open PR for the branch -> success ---------------------
repo="$(make_repo c1)"
run_final_status "$repo" FAKE_GH_MODE=open
if [ "$RC" -eq 0 ] \
    && grep -qx "terminal_success=true" "$OUT" \
    && grep -qx "terminal_state=EXISTING_OPEN_PR" "$OUT" \
    && grep -qx "publish_state_reached=true" "$OUT" \
    && grep -qx "terminal_status=EXISTING_OPEN_PR" "$OUT" \
    && grep -qxF "=== PR: $PR_URL_OK ===" "$OUT" \
    && grep -qx "All 23 workflow steps completed successfully." "$OUT" \
    && ! grep -q "^missing_evidence=" "$OUT"; then
    record_pass "1: empty PR_URL + open branch PR -> EXISTING_OPEN_PR, exit 0"
else
    record_fail "1: empty PR_URL + open branch PR -> EXISTING_OPEN_PR, exit 0"; dump
fi
if [ "$(lookup_calls)" = "1" ] \
    && grep -E -- '^pr list .*--head .*--state open' "$GH_LOG" | grep -qF -- "--head $BRANCH" \
    && grep -E -- '^pr list .*--head .*--state open' "$GH_LOG" | grep -qF -- "--repo octo/example"; then
    record_pass "1a: exactly one lookup, scoped to --repo octo/example --head $BRANCH"
else
    record_fail "1a: exactly one lookup, scoped to --repo octo/example --head $BRANCH"; dump
fi
if grep -qxF "INFO: resolved open PR $PR_URL_OK for branch $BRANCH (previous publish state: ''); treating as EXISTING_OPEN_PR (#1596)" "$ERR"; then
    record_pass "1b: one INFO line on stderr in the documented format"
else
    record_fail "1b: one INFO line on stderr in the documented format"; dump
fi
if grep -qF -- "--pr-url $PR_URL_OK" "$SCOPE_LOG" && grep -qF -- "--pr-number $PR_NUM" "$SCOPE_LOG"; then
    record_pass "1c: validate_final_pr_scope still runs against the resolved PR (evidence, not proof)"
else
    record_fail "1c: validate_final_pr_scope still runs against the resolved PR (evidence, not proof)"; echo "    scope log:"; sed 's/^/    | /' "$SCOPE_LOG"
fi
if ! grep -q "INFO: resolved open PR" "$OUT" && ! grep -qF "(#1596)" "$OUT"; then
    record_pass "1d: lookup messages never go to stdout"
else
    record_fail "1d: lookup messages never go to stdout"; dump
fi

# --- 2. empty PR_URL, no PR -> fails exactly as now -------------------------
repo="$(make_repo c2)"
run_final_status "$repo" FAKE_GH_MODE=empty
assert_unchanged_failure "2: empty PR_URL + no open PR -> FAILED_MISSING_TERMINAL_EVIDENCE, exit 1"
if ! grep -q "#1596" "$ERR"; then
    record_pass "2a: a clean [] adds no #1596 output"
else
    record_fail "2a: a clean [] adds no #1596 output"; dump
fi

# --- 3. gh auth failure -> fail closed with one WARNING ---------------------
repo="$(make_repo c3)"
run_final_status "$repo" FAKE_GH_MODE=401
assert_unchanged_failure "3: gh 401 on lookup -> unchanged failure"
assert_warning_reason "3a: gh 401 -> 'gh failed' WARNING" "gh failed"
if ! grep -q "Bad credentials" "$OUT"; then
    record_pass "3b: raw gh error never reaches stdout"
else
    record_fail "3b: raw gh error never reaches stdout"; dump
fi

# --- 3c. rate limit -> fails fast, closed -----------------------------------
repo="$(make_repo c3c)"
run_final_status "$repo" FAKE_GH_MODE=ratelimit
assert_unchanged_failure "3c: gh rate limit on lookup -> unchanged failure"
assert_warning_reason "3d: rate limit -> 'gh failed' WARNING" "gh failed"

# --- 4. malformed JSON / non-array -> fail closed ---------------------------
repo="$(make_repo c4)"
run_final_status "$repo" FAKE_GH_MODE=notjson
assert_unchanged_failure "4: malformed JSON -> unchanged failure"
assert_warning_reason "4a: malformed JSON -> 'malformed JSON' WARNING" "malformed JSON"

repo="$(make_repo c4b)"
run_final_status "$repo" FAKE_GH_MODE=nonarray
assert_unchanged_failure "4b: non-array JSON ({}) -> unchanged failure"
assert_warning_reason "4c: non-array JSON -> 'malformed JSON' WARNING" "malformed JSON"

# --- 5. only a fork PR -> no match ------------------------------------------
repo="$(make_repo c5)"
run_final_status "$repo" FAKE_GH_MODE=cross-repo
assert_unchanged_failure "5: only a cross-repository (fork) PR -> unchanged failure"

repo="$(make_repo c5b)"
run_final_status "$repo" FAKE_GH_MODE=cross-then-own
if [ "$RC" -eq 0 ] && grep -qx "terminal_status=EXISTING_OPEN_PR" "$OUT" && grep -qxF "=== PR: $PR_URL_OK ===" "$OUT"; then
    record_pass "5b: fork PR skipped, same-repo PR on the branch selected"
else
    record_fail "5b: fork PR skipped, same-repo PR on the branch selected"; dump
fi

# --- 6. non-GitHub host -> no lookup ----------------------------------------
repo="$(make_repo c6)"
run_final_status "$repo" FAKE_GH_MODE=open REMOTE_HOST_TYPE=other
if [ "$RC" -eq 1 ] && grep -qx "terminal_state=FAILED_MISSING_TERMINAL_EVIDENCE" "$OUT" && ! grep -qF -- "--head" "$GH_LOG"; then
    record_pass "6: REMOTE_HOST_TYPE=other -> no gh --head lookup, exit 1"
else
    record_fail "6: REMOTE_HOST_TYPE=other -> no gh --head lookup, exit 1"; dump
fi

repo="$(make_repo c6b)"
run_final_status "$repo" FAKE_GH_MODE=open REMOTE_HOST_TYPE=azdo
if [ "$RC" -eq 1 ] && [ "$(lookup_calls)" = "0" ]; then
    record_pass "6b: REMOTE_HOST_TYPE=azdo -> no lookup (Azure DevOps out of scope)"
else
    record_fail "6b: REMOTE_HOST_TYPE=azdo -> no lookup (Azure DevOps out of scope)"; dump
fi

# --- 7. PR_URL already known -> no lookup -----------------------------------
repo="$(make_repo c7)"
run_final_status "$repo" FAKE_GH_MODE=open PR_URL="$PR_URL_OK" PR_NUMBER="$PR_NUM"
if [ "$RC" -eq 0 ] && [ "$(lookup_calls)" = "0" ] && ! grep -q "#1596" "$ERR"; then
    record_pass "7: PR_URL preset -> no '--state open' lookup"
else
    record_fail "7: PR_URL preset -> no '--state open' lookup"; dump
fi

repo="$(make_repo c7b)"
run_final_status "$repo" FAKE_GH_MODE=open PR_PUBLISH_RESULT_PR_URL="$PR_URL_OK"
assert_no_lookup "7b: PR_PUBLISH_RESULT_PR_URL preset -> no lookup"

# --- 8. publish already reached a success state -> no lookup ----------------
for state in FOLLOWUP_CREATED EXISTING_OPEN_PR MERGED; do
    repo="$(make_repo "c8-$state")"
    run_final_status "$repo" FAKE_GH_MODE=open PR_PUBLISH_RESULT_STATE="$state"
    assert_no_lookup "8: PR_PUBLISH_RESULT_STATE=$state -> no lookup"
done

repo="$(make_repo c8b)"
run_final_status "$repo" FAKE_GH_MODE=open PUBLISH_STATE_REACHED=true
assert_no_lookup "8b: PUBLISH_STATE_REACHED=true -> no lookup"

# --- 9. detached HEAD -> no lookup ------------------------------------------
repo="$(make_repo c9 detached)"
run_final_status "$repo" FAKE_GH_MODE=open
assert_no_lookup "9: detached HEAD -> no lookup"
assert_unchanged_failure "9a: detached HEAD -> unchanged failure"

# --- 9b. on the base branch -> no lookup ------------------------------------
repo="$(make_repo c9b main)"
run_final_status "$repo" FAKE_GH_MODE=open
assert_no_lookup "9b: current branch is the base branch (main) -> no lookup"
assert_unchanged_failure "9c: base branch -> unchanged failure"

# --- 10. non-success publish state + open PR -> EXISTING_OPEN_PR (D1) -------
for state in unknown create-new-pr; do
    repo="$(make_repo "c10-$state")"
    run_final_status "$repo" FAKE_GH_MODE=open PR_PUBLISH_RESULT_STATE="$state"
    if [ "$RC" -eq 0 ] \
        && grep -qx "terminal_state=EXISTING_OPEN_PR" "$OUT" \
        && grep -qx "terminal_status=EXISTING_OPEN_PR" "$OUT" \
        && grep -qF "(previous publish state: '$state'); treating as EXISTING_OPEN_PR (#1596)" "$ERR"; then
        record_pass "10: PR_PUBLISH_RESULT_STATE=$state + open PR -> EXISTING_OPEN_PR, exit 0"
    else
        record_fail "10: PR_PUBLISH_RESULT_STATE=$state + open PR -> EXISTING_OPEN_PR, exit 0"; dump
    fi
done

# --- 10b. explicit PUBLISH_STATE_REACHED=false does not block the lookup (D2)
repo="$(make_repo c10b)"
run_final_status "$repo" FAKE_GH_MODE=open PUBLISH_STATE_REACHED=false
if [ "$RC" -eq 0 ] && grep -qx "publish_state_reached=true" "$OUT" && grep -qx "terminal_status=EXISTING_OPEN_PR" "$OUT"; then
    record_pass "10b: explicit PUBLISH_STATE_REACHED=false + open PR -> EXISTING_OPEN_PR"
else
    record_fail "10b: explicit PUBLISH_STATE_REACHED=false + open PR -> EXISTING_OPEN_PR"; dump
fi

# --- 10c. origin owner/repo compared case-insensitively ---------------------
repo="$(make_repo c10c)"
run_final_status "$repo" FAKE_GH_MODE=open-mixed-case
if [ "$RC" -eq 0 ] && grep -qx "terminal_status=EXISTING_OPEN_PR" "$OUT"; then
    record_pass "10c: PR URL owner/repo differing only in case is accepted"
else
    record_fail "10c: PR URL owner/repo differing only in case is accepted"; dump
fi

# --- 11. explicit terminal failure wins; no lookup --------------------------
repo="$(make_repo c11)"
run_final_status "$repo" FAKE_GH_MODE=open TERMINAL_FAILURE=true
if [ "$RC" -eq 1 ] && grep -qx "terminal_failure=true" "$OUT" && [ "$(lookup_calls)" = "0" ] && ! grep -q "^terminal_status=" "$OUT"; then
    record_pass "11: TERMINAL_FAILURE=true + open PR -> exit 1, no lookup"
else
    record_fail "11: TERMINAL_FAILURE=true + open PR -> exit 1, no lookup"; dump
fi

repo="$(make_repo c11b)"
run_final_status "$repo" FAKE_GH_MODE=open IMPLEMENTATION_COMPLETED=maybe
if [ "$RC" -eq 2 ] && grep -qx "terminal_state=FAILED_INVALID_EVIDENCE" "$OUT" && [ "$(lookup_calls)" = "0" ]; then
    record_pass "11b: invalid evidence -> exit 2, no lookup"
else
    record_fail "11b: invalid evidence -> exit 2, no lookup"; dump
fi

# --- 11c. implementation+verification evidence already closes the run ------
repo="$(make_repo c11c)"
run_final_status "$repo" FAKE_GH_MODE=open IMPLEMENTATION_COMPLETED=true VERIFICATION_COMPLETED=true
if [ "$RC" -eq 0 ] && grep -qx "terminal_state=IMPLEMENTED_VERIFIED" "$OUT" \
    && [ "$(lookup_calls)" = "0" ] && [ ! -s "$SCOPE_LOG" ]; then
    record_pass "11c: implementation+verification evidence + open PR -> IMPLEMENTED_VERIFIED, exit 0, no lookup"
else
    record_fail "11c: implementation+verification evidence + open PR -> IMPLEMENTED_VERIFIED, exit 0, no lookup"; dump
fi

# --- 12. hostile / invalid candidates -> fail closed, no stdout injection ---
repo="$(make_repo c12)"
run_final_status "$repo" FAKE_GH_MODE=injected-url
assert_unchanged_failure "12: URL with embedded newline -> unchanged failure"
assert_warning_reason "12a: injected URL -> 'URL failed validation' WARNING" "URL failed validation"
if ! grep -q "^terminal_status=" "$OUT" && ! grep -q "^terminal_status=" "$ERR"; then
    record_pass "12b: injected terminal_status line never appears on stdout or stderr"
else
    record_fail "12b: injected terminal_status line never appears on stdout or stderr"; dump
fi

repo="$(make_repo c12c)"
run_final_status "$repo" FAKE_GH_MODE=other-repo-url
assert_unchanged_failure "12c: URL for another owner/repo -> unchanged failure"
assert_warning_reason "12d: foreign repo URL -> 'URL failed validation' WARNING" "URL failed validation"
if ! grep -q "other/repo" "$ERR" && ! grep -q "other/repo" "$OUT"; then
    record_pass "12e: rejected candidate URL is not echoed anywhere"
else
    record_fail "12e: rejected candidate URL is not echoed anywhere"; dump
fi

repo="$(make_repo c12f)"
run_final_status "$repo" FAKE_GH_MODE=number-mismatch
assert_unchanged_failure "12f: number differs from URL -> unchanged failure"
assert_warning_reason "12g: number mismatch -> 'PR number mismatch' WARNING" "PR number mismatch"

# --- 13. evidence, not proof: a scope miss still goes through #1268 ---------
# With no terminal-evidence helper the #1268 adjudicator reports UNCERTAIN:
# the run is not failed, but success is NOT claimed.
repo="$(make_repo c13)"
run_final_status "$repo" FAKE_GH_MODE=open SCOPE_STUB_MODE=miss \
    WORKFLOW_TERMINAL_EVIDENCE_HELPER="$TMPROOT/does-not-exist.sh"
if [ "$RC" -eq 0 ] \
    && grep -qx "terminal_state=EXISTING_OPEN_PR" "$OUT" \
    && grep -qx "terminal_verdict=UNCERTAIN" "$OUT" \
    && ! grep -q "All 23 workflow steps completed successfully." "$OUT"; then
    record_pass "13: resolved PR + scope miss -> #1268 adjudication runs (UNCERTAIN, no success line)"
else
    record_fail "13: resolved PR + scope miss -> #1268 adjudication runs (UNCERTAIN, no success line)"; dump
fi

# --- 14. lookup does not depend on caller credentials -----------------------
# env -i already strips GH_TOKEN/GITHUB_TOKEN; assert the script never asks gh
# for a token itself.
if ! grep -h "auth token" "$TMPROOT"/case*.gh.log >/dev/null 2>&1; then
    record_pass "14: no case ever ran 'gh auth token'"
else
    record_fail "14: no case ever ran 'gh auth token'"
fi

echo ""
echo "issue #1596 final-status branch PR lookup: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
