#!/usr/bin/env bash
# test-gh-rate-limit-backoff.sh — focused unit tests for the shared
# GitHub/az rate-limit-aware retry helper (amplifier-bundle/tools/workflow_gh_retry.sh).
#
# Covers the motivating production failures where an exhausted GraphQL quota
# (0/5000, reset up to ~1h later) made `gh` calls fail the workflow closed:
#   * workflow_publish_pr.sh  — `gh pr list` retried 3x then refused.
#   * workflow_pr_scope.sh    — scoped `gh pr list` -> pr_metadata_unavailable.
#   * workflow-prep step-03   — `gh issue create` aborted on rate limit.
#
# Contracts under test:
#   1. classify_gh_error distinguishes auth | rate_limit | transient | other.
#   2. Reset epoch is read from `gh api rate_limit` (non-budget-consuming) and,
#      as a fallback, from Retry-After / X-RateLimit-Reset headers.
#   3. gh_pr_exists_rest maps the REST /pulls payload into the `gh --json` shape.
#   4. The retry driver: (a) waits for the authoritative reset on a rate limit,
#      (b) serves READ existence via REST when GraphQL is exhausted but core has
#      budget, (c) never retries auth errors, (d) keeps the generic transient
#      short-backoff at exactly 3 attempts.
#
# Usage: bash amplifier-bundle/recipes/tests/test-gh-rate-limit-backoff.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
LIB="${REPO_ROOT}/amplifier-bundle/tools/workflow_gh_retry.sh"

[ -f "$LIB" ] || { echo "HARNESS ERROR: missing $LIB" >&2; exit 2; }
command -v jq >/dev/null 2>&1 || { echo "HARNESS ERROR: jq is required" >&2; exit 2; }

WORK="${REPO_ROOT}/.gh-rate-limit-backoff-test-${$}"
rm -rf "$WORK"
mkdir -p "$WORK"
export GH_RETRY_TMPDIR="$WORK"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

PASS_COUNT=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS: $*"; }

# shellcheck source=/dev/null
. "$LIB"

# --- 1. classifier ---------------------------------------------------------
classify_str() { local f="${WORK}/classify-${PASS_COUNT}-${RANDOM}.stderr"; printf '%s' "$1" > "$f"; classify_gh_error "$f"; rm -f "$f"; }

expect_class() {
  local got; got="$(classify_str "$2")"
  [ "$got" = "$3" ] || fail "classify '$1': expected '$3' got '$got'"
  ok "classify $1 -> $3"
}

expect_class "rate-limit-primary"   "GraphQL: API rate limit already exceeded for user ID 1." "rate_limit"
expect_class "rate-limit-secondary" "You have exceeded a secondary rate limit. Please wait."  "rate_limit"
expect_class "rate-limit-429"       "HTTP 429 Too Many Requests"                              "rate_limit"
expect_class "rate-limit-403-bare"  "HTTP 403: Forbidden"                                     "rate_limit"
expect_class "permission-403"       "HTTP 403: Resource not accessible by integration"        "other"
expect_class "auth-bad-credentials" "HTTP 401: Bad credentials"                               "auth"
expect_class "auth-requires"        "gh: authentication failed - requires authentication"     "auth"
expect_class "transient-503"        "HTTP 503 temporary GitHub API failure"                   "transient"
expect_class "transient-502"        "HTTP 502 temporary GitHub API failure"                   "transient"
expect_class "transient-network"    "connection reset by peer"                                "transient"
expect_class "empty"                ""                                                        "other"
expect_class "unrelated"            "some other unrelated failure"                            "other"

# --- 2. reset epoch from headers ------------------------------------------
hdr="${WORK}/headers.stderr"
printf 'X-RateLimit-Reset: 2000000000\n' > "$hdr"
[ "$(gh_reset_epoch_from_stderr "$hdr")" = "2000000000" ] || fail "X-RateLimit-Reset epoch not parsed"
ok "reset epoch parsed from X-RateLimit-Reset header"
now="$(date +%s)"; printf 'Retry-After: 30\n' > "$hdr"
ra="$(gh_reset_epoch_from_stderr "$hdr")"
[ "$ra" -ge "$((now + 29))" ] && [ "$ra" -le "$((now + 35))" ] || fail "Retry-After not converted to epoch (got $ra)"
ok "reset epoch derived from Retry-After header"
rm -f "$hdr"

# --- mock gh helper --------------------------------------------------------
# make_gh <dir> <<'BODY' ... BODY   installs a PATH-shim gh; logs to $GH_LOG.
make_gh() {
  local dir="$1"; mkdir -p "${dir}/bin"
  cat > "${dir}/bin/gh"
  chmod +x "${dir}/bin/gh"
}

# --- 3. gh_pr_exists_rest shape -------------------------------------------
d3="${WORK}/rest"; make_gh "$d3" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
printf 'call %s\n' "$*" >> "${GH_LOG:?}"
case "$2" in
  repos/example/repo/pulls\?*)
    printf '[{"number":42,"title":"t","body":"issue #7","state":"open","created_at":"2026-01-01T00:00:00Z","merged_at":null,"html_url":"https://github.com/example/repo/pull/42","head":{"ref":"feat/x","sha":"abc","repo":{"name":"repo","full_name":"example/repo","owner":{"login":"example"}}},"base":{"ref":"main","repo":{"full_name":"example/repo"}}}]\n' ;;
  *) printf '[]\n' ;;
esac
SHIM
(
  export PATH="${d3}/bin:${PATH}" GH_LOG="${d3}/log"; : > "$GH_LOG"
  out="$(gh_pr_exists_rest example/repo feat/x)"
  printf '%s' "$out" | jq -e '.[0].number == 42 and .[0].state == "OPEN" and .[0].headRefName == "feat/x" and .[0].baseRefName == "main" and .[0].isCrossRepository == false' >/dev/null \
    || { echo "REST shape mismatch: $out" >&2; exit 1; }
) || fail "gh_pr_exists_rest must map REST /pulls into the gh --json shape"
ok "gh_pr_exists_rest maps REST /pulls into gh --json shape"

# --- 4a. rate-limit waits for authoritative reset then retries -------------
d4a="${WORK}/wait"; make_gh "$d4a" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
log="${GH_LOG:?}"; printf 'call %s\n' "$*" >> "$log"
case "${1:-}-${2:-}" in
  api-rate_limit)
    now="$(date +%s)"
    printf '{"resources":{"graphql":{"remaining":0,"reset":%s},"core":{"remaining":0,"reset":%s}}}\n' "$now" "$now" ;;
  pr-list)
    n="$(grep -c '^call pr list' "$log")"
    if [ "$n" -le 1 ]; then echo "GraphQL: API rate limit already exceeded" >&2; exit 1; fi
    printf '[]\n' ;;
  *) exit 1 ;;
esac
SHIM
(
  set +e
  export PATH="${d4a}/bin:${PATH}" GH_LOG="${d4a}/log"; : > "$GH_LOG"
  rc=0
  GH_RETRY_REST_FALLBACK="" _gh_retry_core "pr list" pr list --repo example/repo --head feat/x >/dev/null 2>&1 || rc=$?
  [ "$rc" -eq 0 ] || { echo "rc=$rc" >&2; cat "$GH_LOG" >&2; exit 1; }
  grep -q '^call api rate_limit' "$GH_LOG" || { echo "rate_limit endpoint not consulted" >&2; exit 1; }
  [ "$(grep -c '^call pr list' "$GH_LOG")" -eq 2 ] || { echo "expected 2 pr list attempts" >&2; cat "$GH_LOG" >&2; exit 1; }
) || fail "rate-limit must wait for the authoritative reset (gh api rate_limit) then retry"
ok "rate-limit waits for authoritative reset then retries"

# --- 4b. REST fallback when GraphQL exhausted but core has budget ----------
d4b="${WORK}/restfb"; make_gh "$d4b" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
log="${GH_LOG:?}"; printf 'call %s\n' "$*" >> "$log"
case "${1:-}-${2:-}" in
  api-rate_limit)
    now="$(date +%s)"
    printf '{"resources":{"graphql":{"remaining":0,"reset":%s},"core":{"remaining":4000,"reset":%s}}}\n' "$((now+3600))" "$now" ;;
  api-*)
    case "$2" in
      repos/example/repo/pulls\?*)
        printf '[{"number":88,"title":"t","body":"b","state":"open","created_at":"2026-01-01T00:00:00Z","merged_at":null,"html_url":"https://github.com/example/repo/pull/88","head":{"ref":"feat/x","sha":"abc","repo":{"name":"repo","full_name":"example/repo","owner":{"login":"example"}}},"base":{"ref":"main","repo":{"full_name":"example/repo"}}}]\n' ;;
      *) printf '[]\n' ;;
    esac ;;
  pr-list) echo "GraphQL: API rate limit exceeded" >&2; exit 1 ;;
  *) exit 1 ;;
esac
SHIM
(
  set +e
  export PATH="${d4b}/bin:${PATH}" GH_LOG="${d4b}/log"; : > "$GH_LOG"
  _fb() { gh_pr_exists_rest example/repo feat/x; }
  rc=0
  out="$(GH_RETRY_REST_FALLBACK=_fb _gh_retry_core "pr list" pr list --repo example/repo --head feat/x 2>"${d4b}/err")" || rc=$?
  [ "$rc" -eq 0 ] || { echo "rc=$rc" >&2; cat "$GH_LOG" "${d4b}/err" >&2; exit 1; }
  printf '%s' "$out" | jq -e '.[0].number == 88' >/dev/null || { echo "REST fallback output missing: $out" >&2; exit 1; }
  grep -qi 'REST /pulls fallback' "${d4b}/err" || { echo "REST fallback not logged as WARNING" >&2; cat "${d4b}/err" >&2; exit 1; }
  [ "$(grep -c '^call pr list' "$GH_LOG")" -eq 1 ] || { echo "should not block on GraphQL when REST serves" >&2; cat "$GH_LOG" >&2; exit 1; }
) || fail "REST fallback must serve PR-existence when GraphQL exhausted but core has budget"
ok "REST fallback serves PR-existence when GraphQL exhausted but core has budget"

# --- 4c. auth errors are never retried ------------------------------------
d4c="${WORK}/auth"; make_gh "$d4c" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
printf 'call %s\n' "$*" >> "${GH_LOG:?}"
echo "HTTP 401: Bad credentials" >&2
exit 1
SHIM
(
  set +e
  export PATH="${d4c}/bin:${PATH}" GH_LOG="${d4c}/log"; : > "$GH_LOG"
  rc=0
  _gh_retry_core "pr view" pr view 1 >/dev/null 2>&1 || rc=$?
  [ "$rc" -ne 0 ] || { echo "auth must be non-zero" >&2; exit 1; }
  [ "$(grep -c '^call ' "$GH_LOG")" -eq 1 ] || { echo "auth must not retry" >&2; cat "$GH_LOG" >&2; exit 1; }
) || fail "auth errors (401 / Bad credentials) must never be retried"
ok "auth errors are terminal (no retry)"

# --- 4d. generic 5xx keeps the short-backoff 3-attempt behavior ------------
d4d="${WORK}/transient"; make_gh "$d4d" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
printf 'call %s\n' "$*" >> "${GH_LOG:?}"
echo "HTTP 503 temporary GitHub API failure" >&2
exit 1
SHIM
(
  set +e
  export PATH="${d4d}/bin:${PATH}" GH_LOG="${d4d}/log"; : > "$GH_LOG"
  rc=0
  _gh_retry_core "pr list" pr list --repo example/repo --head feat/x >/dev/null 2>&1 || rc=$?
  [ "$rc" -ne 0 ] || { echo "persistent 5xx must fail closed" >&2; exit 1; }
  [ "$(grep -c '^call pr list' "$GH_LOG")" -eq 3 ] || { echo "transient must keep 3 attempts" >&2; cat "$GH_LOG" >&2; exit 1; }
  grep -q '^call api rate_limit' "$GH_LOG" && { echo "transient must not consult rate_limit reset" >&2; exit 1; }
  true
) || fail "generic 5xx/network errors must keep the short-backoff 3-attempt behavior"
ok "generic 5xx keeps short-backoff 3-attempt behavior"

echo "PASS: gh rate-limit backoff helper contracts are covered (${PASS_COUNT} checks)."
