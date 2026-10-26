#!/usr/bin/env bash
# Terminal-state evidence collector and adjudicator (issue #1268).
#
# `workflow_pr_scope.sh` answers a genuinely semantic question — "is this the PR
# this workflow was supposed to produce?" — with string matching over head/base
# refs. When it misses, `workflow_final_status.sh` used to exit non-zero and the
# whole run was reported as a failure. That happened AFTER a PR had been
# created, reviewed, quality-audited and merged: the run's real deliverable was
# in `main`, and two follow-up PRs it had opened were abandoned, one rotting
# into a conflicted state with nobody to rebase it. The gate meant to prove
# success destroyed it instead.
#
# The scope match stays as a cheap SIGNAL. It is no longer the arbiter. When it
# misses, this helper looks at what actually happened — is there a PR, did it
# merge, is the work in the base branch — and renders one of three verdicts:
#
#   SUCCESS    the work demonstrably landed or a live PR carries it  -> exit 0
#   UNCERTAIN  the evidence could not be read; nothing is claimed and
#              every outstanding artifact is enumerated for the caller -> exit 0
#   FAILED     the evidence WAS readable and shows the work did not
#              land and no PR carries it                              -> exit 1
#
# It is deliberately not a rubber stamp. `hard_negative` marks the case where
# the evidence positively shows failure; in that state no agent judgement, and
# no caller-supplied verdict, can lift the verdict to SUCCESS. And a SUCCESS
# asserted from outside is admitted ONLY when the collected evidence carries at
# least one positive artifact fact (a merged PR, an open PR, or work already in
# base). Judgement can always DOWNGRADE — that direction is never dangerous.
#
# Usage:
#   workflow_terminal_evidence.sh collect [--repo O/R] [--head REF] [--base REF]
#       [--head-sha SHA] [--pr-url URL] [--pr-number N] [--created-after TIME]
#   workflow_terminal_evidence.sh collect-for-step        (recipe wrapper for collect)
#   workflow_terminal_evidence.sh verdict-token          (recipe wrapper: prose -> token)
#   workflow_terminal_evidence.sh adjudicate [--evidence JSON | --evidence-file F | -]
#       [--agent-verdict TOKEN] [--scope-reason REASON] [--report-only]
#
# Exit codes:
#   0  SUCCESS or UNCERTAIN (adjudicate); evidence emitted (collect)
#   1  FAILED (adjudicate)
#   2  usage error or missing required tooling

# Deliberately no `-e`: this helper must always terminate with a structured
# answer on stdout rather than dying mid-probe and leaving the caller guessing.
set -uo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  sed -n '/^# Usage:/,/^#   2  usage/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
}

if ! command -v jq >/dev/null 2>&1; then
  echo '{"ok":false,"reason":"missing_jq"}'
  echo "ERROR: jq is required by workflow_terminal_evidence.sh" >&2
  exit 2
fi

MODE="${1:-}"
[ "$#" -gt 0 ] && shift

REPO=""
HEAD_REF=""
BASE_REF=""
HEAD_SHA=""
PR_URL=""
PR_NUMBER=""
CREATED_AFTER=""
EVIDENCE=""
EVIDENCE_FILE=""
AGENT_VERDICT=""
SCOPE_REASON=""
REPORT_ONLY="false"
READ_STDIN="false"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 ;;
    --head) HEAD_REF="${2:-}"; shift 2 ;;
    --base) BASE_REF="${2:-}"; shift 2 ;;
    --head-sha) HEAD_SHA="${2:-}"; shift 2 ;;
    --pr-url) PR_URL="${2:-}"; shift 2 ;;
    --pr-number) PR_NUMBER="${2:-}"; shift 2 ;;
    --created-after) CREATED_AFTER="${2:-}"; shift 2 ;;
    --evidence) EVIDENCE="${2:-}"; shift 2 ;;
    --evidence-file) EVIDENCE_FILE="${2:-}"; shift 2 ;;
    --agent-verdict) AGENT_VERDICT="${2:-}"; shift 2 ;;
    --scope-reason) SCOPE_REASON="${2:-}"; shift 2 ;;
    --report-only) REPORT_ONLY="true"; shift ;;
    -) READ_STDIN="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "ERROR: unrecognised argument: $1" >&2; usage; exit 2 ;;
  esac
done

# --- shared gh plumbing ----------------------------------------------------
# Evidence reads are best-effort by design: an unreadable GitHub is UNCERTAIN,
# never a fabricated verdict in either direction. A rate limit must not stall
# the final reporting step for a reset window that can be an hour out, so the
# rate-limit wait is pinned off exactly as the display read in
# workflow_final_status.sh does.
GH_RETRY_HELPER="${WORKFLOW_GH_RETRY_HELPER:-${SCRIPT_DIR}/workflow_gh_retry.sh}"
if [ -f "$GH_RETRY_HELPER" ]; then
  # shellcheck source=/dev/null
  . "$GH_RETRY_HELPER"
fi

gh_read() {
  local label="$1"; shift
  if declare -F _gh_retry_core >/dev/null 2>&1; then
    GH_RETRY_MAX_RL_WINDOWS=0 _gh_retry_core "$label" "$@"
  else
    gh "$@"
  fi
}

parse_github_repo_identity() {
  local url="$1" path owner repo
  case "$url" in
    git@github.com:*) path="${url#git@github.com:}" ;;
    ssh://git@github.com/*) path="${url#ssh://git@github.com/}" ;;
    https://*@github.com/*|http://*@github.com/*) path="${url#*://}"; path="${path#*@github.com/}" ;;
    https://github.com/*) path="${url#https://github.com/}" ;;
    http://github.com/*) path="${url#http://github.com/}" ;;
    *) return 1 ;;
  esac
  path="${path%%\?*}"; path="${path%%#*}"; path="${path%.git}"
  case "$path" in */*) ;; *) return 1 ;; esac
  owner="${path%%/*}"; repo="${path#*/}"; repo="${repo%%/*}"
  [ -n "$owner" ] && [ -n "$repo" ] || return 1
  printf '%s/%s\n' "$owner" "$repo"
}

resolve_base_ref() {
  local candidate
  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null 2>&1; then
    printf '%s\n' "${candidate#origin/}"; return 0
  fi
  for candidate in origin/main origin/master origin/develop; do
    if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null 2>&1; then
      printf '%s\n' "${candidate#origin/}"; return 0
    fi
  done
  return 1
}

PR_FIELDS="number,state,title,url,headRefName,baseRefName,createdAt,isCrossRepository"

collect() {
  local prs_for_head="[]" explicit_pr="null" open_all="[]"
  local evidence_readable="true" unreadable_reason=""
  local work_in_base="unknown" raw rc

  if [ -z "$REPO" ]; then
    REPO="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" 2>/dev/null || true)"
  fi
  [ -n "$HEAD_REF" ] || HEAD_REF="$(git branch --show-current 2>/dev/null || true)"
  [ -n "$HEAD_SHA" ] || HEAD_SHA="$(git rev-parse --verify HEAD 2>/dev/null || true)"
  [ -n "$BASE_REF" ] || BASE_REF="$(resolve_base_ref 2>/dev/null || true)"

  # --- git-side evidence: did the work already land in the base branch? -----
  # Ancestry catches a merge commit / fast-forward; an empty diff catches a
  # squash-merge, where the base contains the CONTENT but none of the SHAs.
  # Neither is string matching, and both are facts about the repository.
  if [ -n "$BASE_REF" ] && command -v git >/dev/null 2>&1; then
    local base_commit=""
    for candidate in "refs/remotes/origin/${BASE_REF}" "$BASE_REF" "origin/${BASE_REF}"; do
      if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null 2>&1; then
        base_commit="$candidate"; break
      fi
    done
    if [ -n "$base_commit" ] && [ -n "$HEAD_SHA" ]; then
      if git merge-base --is-ancestor "$HEAD_SHA" "$base_commit" >/dev/null 2>&1; then
        work_in_base="true"
      elif git diff --quiet "${base_commit}..${HEAD_SHA}" >/dev/null 2>&1; then
        work_in_base="true"
      else
        work_in_base="false"
      fi
    fi
  fi

  # --- GitHub-side evidence -------------------------------------------------
  if ! command -v gh >/dev/null 2>&1; then
    evidence_readable="false"; unreadable_reason="gh CLI is not available"
  elif [ -z "$REPO" ]; then
    evidence_readable="false"; unreadable_reason="could not resolve an owner/repo identity"
  elif [ -z "$HEAD_REF" ]; then
    evidence_readable="false"; unreadable_reason="could not resolve the head branch"
  else
    # Every PR ever opened FROM this head branch, in any state. `--state all` is
    # the whole point: the incident's PR was MERGED, so an open-only query could
    # never see it.
    raw="$(gh_read "terminal evidence pr list" pr list --repo "$REPO" --head "$HEAD_REF" --state all --json "$PR_FIELDS" 2>/dev/null)"
    rc="$?"
    if [ "$rc" -ne 0 ]; then
      evidence_readable="false"; unreadable_reason="unable to list pull requests for head branch '$HEAD_REF'"
    else
      [ -n "${raw//[[:space:]]/}" ] || raw="[]"
      if printf '%s' "$raw" | jq -e 'type == "array"' >/dev/null 2>&1; then
        prs_for_head="$(printf '%s' "$raw" | jq -c '.')"
      else
        evidence_readable="false"; unreadable_reason="pull-request metadata was not a JSON array"
      fi
    fi

    # The explicitly published PR, when the run recorded one. It may live on a
    # different head ref than the current branch (a follow-up rebase, a renamed
    # branch), which is precisely why the list above is not sufficient.
    local explicit_target=""
    if [ -n "$PR_URL" ]; then explicit_target="$PR_URL"
    elif [ -n "$PR_NUMBER" ]; then explicit_target="$PR_NUMBER"
    fi
    if [ -n "$explicit_target" ]; then
      if raw="$(gh_read "terminal evidence pr view" pr view "$explicit_target" --repo "$REPO" --json "$PR_FIELDS" 2>/dev/null)" \
        && printf '%s' "$raw" | jq -e 'type == "object"' >/dev/null 2>&1; then
        explicit_pr="$(printf '%s' "$raw" | jq -c '.')"
      fi
    fi

    # Outstanding open PRs. Acceptance criterion: these are enumerated in the
    # final status WHATEVER the verdict, because the incident's real damage was
    # two live PRs nobody was driving.
    if raw="$(gh_read "terminal evidence open pr list" pr list --repo "$REPO" --state open --limit 100 --json "$PR_FIELDS" 2>/dev/null)" \
      && printf '%s' "$raw" | jq -e 'type == "array"' >/dev/null 2>&1; then
      open_all="$(printf '%s' "$raw" | jq -c '.')"
    fi
  fi

  jq -nc \
    --arg schema_version "1" \
    --arg repo "$REPO" \
    --arg head_ref "$HEAD_REF" \
    --arg base_ref "$BASE_REF" \
    --arg head_sha "$HEAD_SHA" \
    --arg pr_url "$PR_URL" \
    --arg pr_number "$PR_NUMBER" \
    --arg created_after "$CREATED_AFTER" \
    --arg evidence_readable "$evidence_readable" \
    --arg unreadable_reason "$unreadable_reason" \
    --arg work_in_base "$work_in_base" \
    --arg scope_reason "$SCOPE_REASON" \
    --argjson prs_for_head "$prs_for_head" \
    --argjson explicit_pr "$explicit_pr" \
    --argjson open_all "$open_all" '
    def merged: [ .[] | select(((.state // "") | ascii_upcase) == "MERGED") ];
    def open_only: [ .[] | select(((.state // "") | ascii_upcase) == "OPEN") ];
    def closed_unmerged: [ .[] | select(((.state // "") | ascii_upcase) == "CLOSED") ];
    def brief: [ .[] | {number, state, title, url, headRefName, baseRefName, createdAt} ];

    ($prs_for_head + (if $explicit_pr == null then [] else [$explicit_pr] end))
      | unique_by(.number) as $all
    | ($all | merged) as $merged
    | ($all | open_only) as $open
    | ($all | closed_unmerged) as $closed
    # Outstanding = every open PR from this head branch, plus (when the run
    # start time is known) every PR opened during the run. Broad on purpose:
    # this is a REPORT, and under-reporting a live PR is the failure that
    # orphaned #1132/#1133.
    | ( ($open)
        + ( if $created_after == "" then []
            else [ $open_all[] | select((.createdAt // "") >= $created_after) ]
            end )
      ) | unique_by(.number) as $outstanding
    | ($merged | length) as $merged_count
    | ($open | length) as $open_count
    | ($closed | length) as $closed_count
    | (($merged_count > 0) or ($open_count > 0) or ($work_in_base == "true")) as $positive
    | (if $evidence_readable != "true" then
         {v: "UNCERTAIN", r: ("terminal evidence could not be read: " + $unreadable_reason)}
       elif $merged_count > 0 then
         {v: "SUCCESS", r: ("a pull request from head branch \"" + $head_ref + "\" is MERGED: #" + (($merged[0].number // 0) | tostring))}
       elif $work_in_base == "true" then
         {v: "SUCCESS", r: ("the head commit is already contained in base branch \"" + $base_ref + "\"; the work landed")}
       elif $open_count > 0 then
         {v: "SUCCESS", r: ("an OPEN pull request carries this work: #" + (($open[0].number // 0) | tostring))}
       elif ($closed_count > 0) and ($work_in_base == "false") then
         {v: "FAILED", r: "every pull request for this head branch is CLOSED and unmerged, and the work is not in the base branch"}
       elif ($closed_count == 0) and ($open_count == 0) and ($merged_count == 0) and ($work_in_base == "false") then
         {v: "FAILED", r: ("no pull request exists for head branch \"" + $head_ref + "\" and its commits are not in the base branch; the work was never published")}
       else
         {v: "UNCERTAIN", r: "no pull request matched and the base-branch state of the work could not be determined"}
       end) as $mech
    | {
        schema_version: ($schema_version | tonumber),
        repo: $repo,
        head_ref: $head_ref,
        base_ref: $base_ref,
        head_sha: $head_sha,
        published_pr_url: $pr_url,
        published_pr_number: $pr_number,
        created_after: $created_after,
        evidence_readable: $evidence_readable,
        unreadable_reason: $unreadable_reason,
        scope_reason: $scope_reason,
        work_in_base: $work_in_base,
        prs_for_head: ($all | brief),
        merged_pr_count: $merged_count,
        open_pr_count: $open_count,
        closed_unmerged_pr_count: $closed_count,
        positive_artifact: (if $positive then "true" else "false" end),
        outstanding_prs: ($outstanding | brief),
        outstanding_pr_count: ($outstanding | length),
        outstanding_pr_basis: "open PRs on this head branch, plus every PR opened since the run started",
        mechanical_verdict: $mech.v,
        mechanical_reason: $mech.r,
        hard_negative: (if $mech.v == "FAILED" then "true" else "false" end)
      }'
}

# Case-insensitive EXACT-TOKEN equality, never substring: `NOT_MERGED` contains
# `MERGED` and `UNSUCCESSFUL` contains `SUCCESS`, and a `contains` match would
# read both as approval. Anything unrecognised — including the empty string —
# is UNCERTAIN, which never lifts a verdict on its own.
normalise_verdict_token() {
  local t
  t="$(printf '%s' "${1:-}" | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')"
  case "$t" in
    SUCCESS|SUCCEEDED|SUCCESSFUL|PASS|PASSED|LANDED|MERGED|COMPLETE|COMPLETED|ACHIEVED) printf 'SUCCESS' ;;
    FAILED|FAILURE|FAIL|NOT_LANDED|NOTLANDED|UNMERGED|ABANDONED|BLOCKED) printf 'FAILED' ;;
    *) printf 'UNCERTAIN' ;;
  esac
}

adjudicate() {
  local ev
  if [ -n "$EVIDENCE_FILE" ]; then
    ev="$(cat "$EVIDENCE_FILE" 2>/dev/null)"
  elif [ -n "$EVIDENCE" ]; then
    ev="$EVIDENCE"
  elif [ "$READ_STDIN" = "true" ] || [ ! -t 0 ]; then
    ev="$(cat)"
  else
    ev=""
  fi

  if [ -z "${ev//[[:space:]]/}" ] || ! printf '%s' "$ev" | jq -e 'type == "object"' >/dev/null 2>&1; then
    # No structured evidence at all. This is not a licence to pass: nothing was
    # proven, so the honest answer is UNCERTAIN with the artifacts we do know.
    jq -nc --arg reason "terminal evidence was absent or unparseable" \
           --arg pr_url "$PR_URL" --arg pr_number "$PR_NUMBER" '
      {terminal_verdict:"UNCERTAIN", verdict_source:"missing_evidence",
       terminal_success_proven:"false", mechanical_verdict:"UNCERTAIN",
       agent_verdict:"", reason:$reason,
       required_next_action:"Re-run terminal evidence collection; do not close the run until the outstanding pull requests below are driven to a terminal state.",
       outstanding_prs: (if $pr_url == "" and $pr_number == "" then [] else [{url:$pr_url, number:$pr_number}] end),
       outstanding_pr_count: (if $pr_url == "" and $pr_number == "" then 0 else 1 end)}'
    echo "WARNING: workflow_terminal_evidence.sh: UNCERTAIN — terminal evidence was absent or unparseable." >&2
    return 0
  fi

  field() { printf '%s' "$ev" | jq -r --arg f "$1" '.[$f] // ""'; }

  local mech positive hard agent final source reason
  mech="$(field mechanical_verdict)"
  positive="$(field positive_artifact)"
  hard="$(field hard_negative)"
  case "$mech" in SUCCESS|UNCERTAIN|FAILED) ;; *) mech="UNCERTAIN" ;; esac

  agent=""
  if [ -n "${AGENT_VERDICT//[[:space:]]/}" ]; then
    agent="$(normalise_verdict_token "$AGENT_VERDICT")"
  fi

  if [ "$hard" = "true" ] || [ "$mech" = "FAILED" ]; then
    # HARD NEGATIVE. The evidence was readable and it shows the work did not
    # land and no PR carries it. Judgement is not consulted and cannot lift it —
    # this is the guard that stops the adjudicator becoming a rubber stamp.
    final="FAILED"; source="deterministic_hard_negative"
    reason="$(field mechanical_reason)"
  elif [ -z "$agent" ]; then
    final="$mech"; source="deterministic_only"
    reason="$(field mechanical_reason)"
  elif [ "$agent" = "FAILED" ]; then
    # Downgrades are always admitted: judgement that says "this merged PR is not
    # ours" costs a false alarm, never a silently orphaned run.
    final="FAILED"; source="agent_downgrade"
    reason="evaluation judged the run failed despite mechanical evidence of ${mech}"
  elif [ "$agent" = "UNCERTAIN" ]; then
    final="UNCERTAIN"; source="agent_uncertain"
    reason="evaluation could not confirm the run's terminal state"
  elif [ "$positive" = "true" ]; then
    final="SUCCESS"; source="agent_confirmed"
    reason="evaluation confirmed the run achieved its goal; $(field mechanical_reason)"
  else
    # An asserted SUCCESS with nothing to stand on is refused. No merged PR, no
    # open PR, no work in base — there is no artifact that could make this true.
    final="UNCERTAIN"; source="agent_success_refused_no_artifact"
    reason="evaluation asserted success but the collected evidence contains no merged PR, no open PR, and no work in the base branch"
  fi

  local proven="false"
  [ "$final" = "SUCCESS" ] && proven="true"

  local next_action
  case "$final" in
    SUCCESS) next_action="None for the primary deliverable. Drive any outstanding pull requests listed below to a terminal state." ;;
    UNCERTAIN) next_action="Terminal success was NOT proven and NOT disproven. Do not discard this run: hand the outstanding pull requests listed below to the goal-seeking loop and re-check." ;;
    *) next_action="Terminal failure is proven from readable evidence. Investigate why the work never reached a pull request or the base branch." ;;
  esac

  printf '%s' "$ev" | jq -c \
    --arg terminal_verdict "$final" \
    --arg verdict_source "$source" \
    --arg terminal_success_proven "$proven" \
    --arg agent_verdict "$agent" \
    --arg reason "$reason" \
    --arg required_next_action "$next_action" '
    {
      terminal_verdict: $terminal_verdict,
      verdict_source: $verdict_source,
      terminal_success_proven: $terminal_success_proven,
      mechanical_verdict: (.mechanical_verdict // "UNCERTAIN"),
      agent_verdict: $agent_verdict,
      reason: $reason,
      required_next_action: $required_next_action,
      scope_reason: (.scope_reason // ""),
      evidence_readable: (.evidence_readable // "false"),
      work_in_base: (.work_in_base // "unknown"),
      merged_pr_count: (.merged_pr_count // 0),
      open_pr_count: (.open_pr_count // 0),
      closed_unmerged_pr_count: (.closed_unmerged_pr_count // 0),
      outstanding_prs: (.outstanding_prs // []),
      outstanding_pr_count: (.outstanding_pr_count // 0)
    }'

  # Human-readable half. The incident's error text — `no_scoped_pr` — told the
  # operator nothing about the PR that HAD merged or the two left dangling.
  {
    echo "=== TERMINAL STATE ADJUDICATION (issue #1268) ==="
    echo "verdict=${final} (source=${source})"
    echo "reason: ${reason}"
    [ -n "$(field scope_reason)" ] && echo "mechanical scope signal: $(field scope_reason) (a signal, not the verdict)"
    echo "merged_pr_count=$(field merged_pr_count) open_pr_count=$(field open_pr_count) closed_unmerged_pr_count=$(field closed_unmerged_pr_count) work_in_base=$(field work_in_base)"
    echo "outstanding_pr_count=$(field outstanding_pr_count)"
    printf '%s' "$ev" | jq -r '(.outstanding_prs // [])[] | "  OUTSTANDING PR #\(.number) [\(.state)] \(.url) — \(.title)"'
    echo "required_next_action: ${next_action}"
  } >&2

  case "$final" in
    SUCCESS) return 0 ;;
    UNCERTAIN) return 0 ;;
    *) [ "$REPORT_ONLY" = "true" ] && return 0; return 1 ;;
  esac
}

# Read field <field> of the JSON object in the base variable RECIPE_VAR_<key>
# (#1561). The runner writes RECIPE_VAR_<key> for every declared key, but a
# nested name only for fields the current object has, so a nested name can
# hold a parent run's value and is never read. Prints nothing for an unset or
# empty key, a value that is not a JSON object, or a missing or null field;
# `false` and `0` print as values. Callers treat empty as "not reported".
rv_field() {
  local key="$1" field="$2" var
  [[ "$key" =~ ^[A-Za-z0-9_]+$ && "$field" =~ ^[A-Za-z0-9_]+$ ]] || return 0
  var="RECIPE_VAR_${key}"
  printf '%s' "${!var:-}" \
    | jq -r --arg f "$field" 'if type=="object" and has($f) and .[$f] != null then .[$f] | tostring else empty end' 2>/dev/null \
    || true
}

# Recipe-facing wrappers (issue #1268). These keep the recipe steps to a couple
# of lines each: the env-variable precedence and the fail-soft fallbacks live
# here, next to the logic they feed, instead of being duplicated across recipes.
collect_for_step() {
  local pr_url pr_number
  pr_url="${RECIPE_VAR_pr_url:-$(rv_field pr_publish_result pr_url)}"
  pr_number="${RECIPE_VAR_pr_number:-$(rv_field pr_publish_result pr_number)}"
  [ "$pr_url" = "''" ] && pr_url=""
  PR_URL="$pr_url"
  case "$pr_number" in ''|"''"|*[!0-9]*) PR_NUMBER="" ;; *) PR_NUMBER="$pr_number" ;; esac
  CREATED_AFTER="${CREATED_AFTER:-${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-${TASK_STARTED_AT:-}}}}"
  SCOPE_REASON="${SCOPE_REASON:-pre-collected for the terminal-status gate}"
  collect
}

# Extract the single verdict token from the adjudicator agent's prose. The last
# explicit line wins; anything unrecognised — including no narrative at all,
# when the agent step was skipped — stays empty, which the adjudicator treats
# as "no judgement offered" and which therefore lifts nothing on its own.
verdict_token() {
  local narrative token
  narrative="${RECIPE_VAR_terminal_adjudication_narrative:-}"
  token="$(printf '%s\n' "$narrative" \
    | grep -Eo '^[[:space:]]*TERMINAL_VERDICT:[[:space:]]*[A-Za-z_]+' \
    | tail -1 \
    | sed -E 's/^[[:space:]]*TERMINAL_VERDICT:[[:space:]]*//' \
    | tr '[:lower:]' '[:upper:]' || true)"
  case "$token" in SUCCESS|FAILED|UNCERTAIN) ;; *) token="" ;; esac
  jq -nc --arg agent_verdict "$token" '{agent_verdict: $agent_verdict}'
}

case "$MODE" in
  collect) collect ;;
  collect-for-step) collect_for_step ;;
  verdict-token) verdict_token ;;
  adjudicate) adjudicate ;;
  *)
    echo "ERROR: workflow_terminal_evidence.sh requires mode: collect, collect-for-step, verdict-token, or adjudicate" >&2
    usage
    exit 2
    ;;
esac
