#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #1518: auto-drive must read
# merge-ready criterion 6 (reviews and approvals) from what GitHub reports, not
# from an empty reviewDecision.
#
# Runs the SHIPPED amplifier-bundle/tools/autodrive_platform_facts.sh, as a
# black box, against a stand-in `gh` that answers the way GitHub does: the
# pull request through `gh pr view --json ... --jq ...`, branch protection and
# rulesets through `gh api`, and an HTTP error as a JSON body on stdout, a
# message on stderr and exit 1. A token without admin rights gets 404 from the
# protection endpoint (measured on rysweet/amplihack-rs), so a 404 must never
# read as "0 approvals required".
#
# Launched by the gadugi `execute` action with no arguments; prints one PASS or
# FAIL line per case and ALL_CASES_PASSED when every case holds. Needs bash,
# jq and coreutils, nothing else.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TOOL="$REPO_ROOT/amplifier-bundle/tools/autodrive_platform_facts.sh"
[ -f "$TOOL" ] || { echo "FAIL: missing $TOOL"; echo "SCENARIO_FAILED"; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "FAIL: jq is required"; echo "SCENARIO_FAILED"; exit 1; }
JQ_DIR="$(dirname "$(command -v jq)")"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/home"
HEAD_SHA="0123456789abcdef0123456789abcdef01234567"

cat > "$WORK/bin/gh" <<'GH'
#!/usr/bin/env bash
JQ=""; prev=""; for a in "$@"; do [ "$prev" = "--jq" ] && JQ="$a"; prev="$a"; done
out() { if [ -n "$JQ" ]; then printf '%s' "$1" | jq -r "$JQ"; else printf '%s\n' "$1"; fi; }
http404() { printf '{"message":"%s","documentation_url":"https://docs.github.com/rest","status":"404"}' "$1"; echo "gh: $1 (HTTP 404)" >&2; exit 1; }
case "$1 ${2:-}" in
  "pr view")
    PASSED='[{"__typename":"CheckRun","name":"Test","status":"COMPLETED","conclusion":"SUCCESS"}]'
    out "$(jq -cn --arg m "${PF_MSTATE:-CLEAN}" --arg d "${PF_DECISION-}" --arg h "$PF_HEAD" \
      --argjson c "${PF_CHECKS:-$PASSED}" \
      '{state:"OPEN",isDraft:false,mergeable:"MERGEABLE",mergeStateStatus:$m,
        reviewDecision:(if $d == "" then null else $d end),headRefOid:$h,statusCheckRollup:$c,baseRefName:"main"}')"
    exit 0 ;;
  "api graphql") # one page of review threads, through the tool's own --jq
    out "$(jq -cn --argjson n "${PF_THREAD_NODES:-[]}" \
      '{data:{repository:{pullRequest:{reviewThreads:{pageInfo:{hasNextPage:false,endCursor:null},nodes:$n}}}}}')"
    exit 0 ;;
esac
if [ "${1:-}" = api ]; then
  path=""; for a in "$@"; do case "$a" in repos/*) path="$a" ;; esac; done
  case "$path" in
    */protection/required_pull_request_reviews)
      [ "${PF_CLASSIC:-404}" = 404 ] && http404 "Not Found"
      out "{\"url\":\"u\",${PF_CLASSIC_FLAGS:+${PF_CLASSIC_FLAGS},}\"required_approving_review_count\":${PF_CLASSIC}}"; exit 0 ;;
    */rules/branches/*) out "${PF_RULES:-[]}"; exit 0 ;;
    */branches/*) out "{\"name\":\"main\",\"protected\":${PF_PROTECTED:-true}}"; exit 0 ;;
  esac
fi
exit 1
GH
chmod +x "$WORK/bin/gh"

fail=0
N=0
# check <token> <want approval_status> <want approval_source> <want required_approvals> <why> [PF_VAR=value ...]
check() {
  local token="$1" status="$2" source="$3" req="$4" why="$5" out got
  shift 5
  N=$((N + 1))
  out="$(env -i HOME="$WORK/home" PATH="$WORK/bin:$JQ_DIR:/usr/bin:/bin" PF_HEAD="$HEAD_SHA" "$@" \
    bash "$TOOL" 42 2>"$WORK/err.$N")"
  got="$(printf '%s' "$out" | jq -r '[.approval_status, .approval_source, .required_approvals] | join(" ")' 2>/dev/null)"
  if [ "$got" = "$status $source $req" ] && [ "$(printf '%s' "$out" | jq -r .head_sha 2>/dev/null)" = "$HEAD_SHA" ]; then
    echo "PASS: $token — $why"
  else
    echo "FAIL: $token — $why (got [$got], want [$status $source $req]; out=$out; err=$(tr '\n' ' ' < "$WORK/err.$N"))"
    fail=1
  fi
}

FAILING='[{"__typename":"CheckRun","name":"Test","status":"COMPLETED","conclusion":"FAILURE"}]'
RULE2='[{"type":"deletion"},{"type":"pull_request","parameters":{"required_approving_review_count":2}}]'

check approval_count_0_is_met MET required-count 0 \
  "branch protection requires 0 approvals: an empty reviewDecision is met (#1518)" \
  PF_CLASSIC=0 PF_DECISION= PF_MSTATE=CLEAN
check approval_count_1_empty_decision_is_not_met NOT_MET required-count 1 \
  "1 approval required and an empty reviewDecision: not met, whatever the merge state says" \
  PF_CLASSIC=1 PF_DECISION= PF_MSTATE=CLEAN
check approval_count_1_approved_is_met MET review-decision 1 \
  "1 approval required and GitHub says APPROVED" \
  PF_CLASSIC=1 PF_DECISION=APPROVED PF_MSTATE=CLEAN
check approval_404_clean_is_met_by_merge_state MET merge-state "" \
  "a non-admin 404 is never read as 0; CLEAN merge state is GitHub saying no required review blocks" \
  PF_CLASSIC=404 PF_PROTECTED=true PF_DECISION= PF_MSTATE=CLEAN
check approval_404_blocked_is_unreadable UNREADABLE unreadable "" \
  "a 404 with BLOCKED and every check passed leaves only rules a person satisfies" \
  PF_CLASSIC=404 PF_PROTECTED=true PF_DECISION= PF_MSTATE=BLOCKED
check approval_404_blocked_failing_check_is_pending PENDING other-blockers "" \
  "a failing check hides whether a review is missing; clear it first" \
  PF_CLASSIC=404 PF_PROTECTED=true PF_DECISION= PF_MSTATE=BLOCKED PF_CHECKS="$FAILING"
check approval_404_blocked_outdated_thread_is_pending PENDING other-blockers "" \
  "an unresolved thread that went outdated is still unresolved; an agent resolves it before a person is asked" \
  PF_CLASSIC=404 PF_PROTECTED=true PF_DECISION= PF_MSTATE=BLOCKED \
  PF_THREAD_NODES='[{"isResolved":true,"isOutdated":false},{"isResolved":false,"isOutdated":true}]'
check approval_count_0_with_code_owner_rule_is_not_zero UNREADABLE unreadable "" \
  "a count of 0 beside a code-owner rule is unknown, never 0" \
  PF_CLASSIC=0 PF_CLASSIC_FLAGS='"require_code_owner_reviews":true' PF_DECISION= PF_MSTATE=BLOCKED
check approval_ruleset_count_2_is_not_met NOT_MET required-count 2 \
  "a ruleset requiring 2 approvals is read with read access when protection returns 404" \
  PF_CLASSIC=404 PF_PROTECTED=true PF_RULES="$RULE2" PF_DECISION= PF_MSTATE=CLEAN

if [ "$fail" -eq 0 ]; then
  echo "criterion 6 measured from GitHub's own answers"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
