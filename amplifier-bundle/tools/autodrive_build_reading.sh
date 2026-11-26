#!/usr/bin/env bash
# autodrive_build_reading.sh <turn_dir>: the build goal's fresh reading of the
# live state (#3129 PR 2). Called by autodrive_build_goal.sh `read`.
#
# Writes <turn_dir>/goal.json, schema 1: `summary` (at most 600 bytes),
# `subject`, `guidance_target` (the run's pull request URL, present only once
# one exists), `worktree` (the run's worktree when it is present and valid,
# otherwise the empty private folder <state>/no-worktree, so the agent never
# works in the launch checkout by accident), `untrusted` (the JSON paths that
# carry third-party text), and these sections, each with its own `error`
# (null when the read worked; a failed read is null data with its reason,
# never an absence):
#   run            branch, base, worktree, launch checkout, local head, pushed
#                  head, commits not pushed, uncommitted changes, a merge in
#                  progress, identity_check, the state folder and checks file
#   prs            how the run's pull request was found (state, branch,
#                  context, none) and each open candidate on the run branch
#   pr, checks, reviews, repository, base
#                  with an own pull request: taken whole from PR 1's
#                  autodrive_pr_reading.sh. Before one exists, `base` is
#                  computed here (commits behind, conflicting files, files
#                  changed on both sides, the strict policy)
#   issue          number and source (tracking_issue_reference), and each claim
#                  with its relation: run-branch, recorded-pr, context-pr or
#                  other-branch
#   host_inputs    workflow_worktree_host_inputs.sh facts
#   checks_passed  the checks the agent reported in <state>/checks.json at the
#                  current local head only (reported_by agent; malformed
#                  entries counted in `ignored`)
#   pr_text        the intended title and body, and whether the live text
#                  equals them after normalisation
#   context        the context keys the agent may need; never no_merge or
#                  finalization_handoff, which a build ignores
#
# Only refs are fetched; nothing here changes a worktree or GitHub. The one
# write is a cache in the state folder: an own pull request found by branch is
# recorded in pr.json, so a later `final` reads it. Exits 75 and 79 pass up;
# every other failure is recorded and the read exits 0.
# docs/reference/build-goal-loop.md#the-reading.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 2
# shellcheck source=autodrive_build_lib.sh
. "$HERE/autodrive_build_lib.sh" || exit 2
# shellcheck source=workflow_gh_retry.sh
. "$HERE/workflow_gh_retry.sh" || exit 2
TD="${1:-}"
build_turn_dir "$TD" || exit 2
STATE="${AUTODRIVE_STATE_DIR:-}"; STATE="${STATE%/}"
# The run's repo_path when it is absolute, else the driver's launch checkout
# (goal contract v1), from which the driver runs this tool; a relative
# repo_path was resolved by the step that started the driver.
LAUNCH="${RECIPE_VAR_repo_path:-}"
case "$LAUNCH" in /*) ;; *) [ -z "${AUTODRIVE_REPO:-}" ] || LAUNCH="$AUTODRIVE_REPO" ;; esac
if [ -z "$LAUNCH" ]; then LAUNCH="."; fi
LAUNCH="$(cd "$LAUNCH" 2>/dev/null && pwd -P)" || LAUNCH="$PWD"
S="$(mktemp -d "$TD/.reading.XXXXXX")" || exit 1
GH_READ_DIR="$S"
trap 'gh_goal_cancel; rm -rf -- "$S"' EXIT
trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM
passup() { case "$1" in 75|79) exit "$1" ;; esac; }
GUIDANCE_SINCE="${AUTODRIVE_GUIDANCE_MARK:-}"
goal_is_entry "$GUIDANCE_SINCE" || GUIDANCE_SINCE=""
GUIDANCE_TARGET="${AUTODRIVE_GUIDANCE_TARGET:-}"
goal_github_pr_repo "$GUIDANCE_TARGET" "${GUIDANCE_TARGET##*/}" >/dev/null || GUIDANCE_TARGET=""
gh_rate_limit_guidance() { goal_read_guidance; }
err() { build_redact_tail < "$1" 2>/dev/null; }
put() { # put <section> <json object or empty> <error>
  if [ -n "$2" ] && printf '%s' "$2" | jq -e 'type == "object"' >/dev/null 2>&1; then
    printf '%s' "$2" | jq -c --arg e "$3" '. + {error:(if $e == "" then null else $e end)}' > "$S/$1.json"
  else
    jq -cn --arg e "${3:-the read gave no usable answer}" '{error:$e}' > "$S/$1.json"
  fi
}

# --- identity: the repository, the run branch and its worktree ----------------
REPO_ERR=""; build_repo_slug "$LAUNCH" || REPO_ERR="$BUILD_REPO_ERR"
build_run_branch || :
build_identity_load || :
BRANCH="$BUILD_BRANCH"
WT="${ID_WORKTREE:-$(build_ws_field worktree_path)}"
ID_CHECK=ok
if build_worktree_check "$WT" "$BRANCH" "$LAUNCH"; then WT="$(cd "$WT" && pwd -P)"; WT_OK=true
else ID_CHECK="$BUILD_WT_WHY"; WT_OK=false; fi
[ -n "$BRANCH" ] || ID_CHECK="the run branch is not known from worktree_setup.branch_name or existing_branch"
BASE_NAME="${RECIPE_VAR_base_branch:-}"; build_valid_branch "$BASE_NAME" || BASE_NAME="$ID_BASE"
if ! build_valid_branch "$BASE_NAME"; then
  BASE_NAME="$(git -C "$LAUNCH" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null)"; BASE_NAME="${BASE_NAME#origin/}"
  build_valid_branch "$BASE_NAME" || BASE_NAME=""
fi

# --- the local state of the run branch -------------------------------------------
LOCAL_HEAD=""; HEAD_SRC=none; UNCOMMITTED=null; DIRTY_LIST='[]'; MERGING=null
if [ "$WT_OK" = true ]; then
  LOCAL_HEAD="$(git -C "$WT" rev-parse --verify -q 'HEAD^{commit}' 2>/dev/null)"; HEAD_SRC=worktree
  git -C "$WT" status --porcelain --untracked-files=normal > "$S/dirty" 2>/dev/null && {
    UNCOMMITTED="$(wc -l < "$S/dirty" | tr -d ' ')"
    DIRTY_LIST="$(head -n 50 "$S/dirty" | goal_clean_text | jq -R -s -c 'split("\n") | map(select(length > 0))')"; }
  if git -C "$WT" rev-parse -q --verify MERGE_HEAD >/dev/null 2>&1; then MERGING=true; else MERGING=false; fi
elif [ -n "$BRANCH" ] && git -C "$LAUNCH" rev-parse --verify -q "refs/heads/${BRANCH}^{commit}" >/dev/null 2>&1; then
  LOCAL_HEAD="$(git -C "$LAUNCH" rev-parse --verify -q "refs/heads/${BRANCH}^{commit}")"; HEAD_SRC=local-branch
else
  LOCAL_HEAD="$(git -C "$LAUNCH" rev-parse --verify -q 'HEAD^{commit}' 2>/dev/null)"; HEAD_SRC="launch-checkout"
fi
goal_is_sha "$LOCAL_HEAD" || LOCAL_HEAD=""

# The run branch and the base on origin: one ls-remote, only when the run has
# a branch of its own. Pushed head and commits not pushed come from it.
PUSHED=""; BASE_SHA=""; AHEAD=null; RUN_ERR=""
if [ -n "$BRANCH" ] && [ "$HEAD_SRC" != launch-checkout ]; then
  refs=("refs/heads/${BRANCH}"); [ -z "$BASE_NAME" ] || refs+=("refs/heads/${BASE_NAME}")
  if git -C "$LAUNCH" ls-remote origin "${refs[@]}" > "$S/ls" 2>"$S/ls.err"; then
    PUSHED="$(awk -v r="refs/heads/${BRANCH}" '$2 == r {print $1}' "$S/ls")"
    [ -z "$BASE_NAME" ] || BASE_SHA="$(awk -v r="refs/heads/${BASE_NAME}" '$2 == r {print $1}' "$S/ls")"
    if goal_is_sha "$PUSHED" && [ -n "$LOCAL_HEAD" ] && git -C "$LAUNCH" cat-file -e "${PUSHED}^{commit}" 2>/dev/null; then
      AHEAD="$(git -C "$LAUNCH" rev-list --count "${PUSHED}..${LOCAL_HEAD}" 2>/dev/null || echo null)"
    fi
  else
    RUN_ERR="origin could not be asked for the run branch (git ls-remote): $(err "$S/ls.err")"
  fi
fi
[ -n "$REPO_ERR" ] && RUN_ERR="${RUN_ERR:+$RUN_ERR; }${REPO_ERR}"
[ -z "$STATE" ] || { mkdir -p -- "$STATE/no-worktree" 2>/dev/null && chmod 700 "$STATE/no-worktree" 2>/dev/null; }
put run "$(jq -cn --arg b "$BRANCH" --arg bs "$BUILD_BRANCH_SOURCE" --arg base "$BASE_NAME" --arg wt "$WT" --argjson wok "$WT_OK" \
  --arg launch "$LAUNCH" --arg lh "$LOCAL_HEAD" --arg hs "$HEAD_SRC" --arg ph "$PUSHED" --argjson ahead "$AHEAD" \
  --argjson unc "$UNCOMMITTED" --argjson dl "$DIRTY_LIST" --argjson m "$MERGING" --arg idc "$ID_CHECK" --arg st "$STATE" --arg repo "$BUILD_REPO" '
  {branch:(if $b == "" then null else $b end), branch_source:$bs, base:(if $base == "" then null else $base end),
   worktree:{path:(if $wt == "" then null else $wt end), present:$wok}, launch_checkout:$launch, repository:(if $repo == "" then null else $repo end),
   local_head:(if $lh == "" then null else $lh end), local_head_from:$hs, pushed_head:(if $ph == "" then null else $ph end),
   commits_not_pushed:$ahead, uncommitted_changes:$unc, uncommitted_files:$dl, merge_in_progress:$m, identity_check:$idc,
   state_dir:$st, checks_file:($st + "/checks.json")}')" "$RUN_ERR"

# --- the run's pull request ------------------------------------------------------
PRS_ERR=""
if [ -z "$BRANCH" ]; then PRS_ERR="the run branch is not known, so no pull request was looked up by branch"
elif [ -n "$REPO_ERR" ]; then PRS_ERR="$REPO_ERR"; fi
OWN_PR=""; OWN_HOW=none
if [ -z "$REPO_ERR" ]; then
  build_own_pr gh_goal_read graphql false; rc=$?; passup "$rc"
  [ "$PR_LIST_FAILED" = false ] || PRS_ERR="$PR_LIST_ERR"
fi
# Cache only: an own pull request found by branch is recorded for `final`.
if [ "$OWN_HOW" = branch ] && [ -n "$STATE" ] && [ ! -e "$STATE/pr.json" ]; then
  jq -cn --argjson n "$OWN_PR" --arg b "$BRANCH" --arg u "$(printf '%s' "$BUILD_CANDIDATES" | jq -r --argjson n "$OWN_PR" '.[] | select(.number == $n) | .url // empty')" \
    --arg t "$(date -u +%Y-%m-%dT%H:%M:%SZ)" '{schema_version:1, number:$n, url:$u, how:"branch", branch:$b, time:$t}' | goal_atomic_write "$STATE/pr.json" || :
fi
put prs "$(jq -cn --arg how "$OWN_HOW" --arg own "$OWN_PR" --argjson c "$BUILD_CANDIDATES" --arg x "$CROSS_PR" \
  '{how_found:$how, own:(if $own == "" then null else ($own | tonumber) end), candidates:$c,
    cross_repository_on_branch:(if $x == "" then null else ($x | tonumber) end)}')" "$PRS_ERR"

# --- with an own pull request: PR 1's reading of it ------------------------------
URL=""; PR_STATE=""; PR_DRAFT=""
if [ -n "$OWN_PR" ]; then
  mv -f -- "$TD/goal.json" "$S/goal.before" 2>/dev/null
  RECIPE_VAR_pr_number="$OWN_PR" AUTODRIVE_STATE_DIR="$STATE" AUTODRIVE_REPO="$LAUNCH" AUTODRIVE_INVOCATION_FILE="" \
    AUTODRIVE_GUIDANCE_MARK="$GUIDANCE_SINCE" AUTODRIVE_GUIDANCE_TARGET="$GUIDANCE_TARGET" \
    gh_goal_command bash "$HERE/autodrive_pr_reading.sh" "$TD" >"$S/pr-reading.log" 2>&1; rc=$?
  passup "$rc"
  if [ "$rc" -eq 0 ] && goal_json_file "$TD/goal.json" 1048576; then
    for sec in pr checks reviews repository base; do jq -c --arg s "$sec" '.[$s] // {error:"absent"}' "$TD/goal.json" > "$S/$sec.json"; done
    URL="$(jq -r '.url // empty | strings' "$S/pr.json")"; PR_STATE="$(jq -r '.pr.state // empty | strings' "$TD/goal.json")"
    PR_DRAFT="$(jq -r '.pr.draft | tostring' "$TD/goal.json")"
  else
    for sec in pr checks reviews repository base; do put "$sec" "" "the reading of pull request #${OWN_PR} failed (exit ${rc}): $(err "$S/pr-reading.log")"; done
  fi
  rm -f -- "$TD/goal.json"
  [ -n "$URL" ] || URL="$(printf '%s' "$BUILD_CANDIDATES" | jq -r --argjson n "$OWN_PR" '.[] | select(.number == $n) | .url // empty')"
  if goal_github_pr_repo "$URL" "$OWN_PR" >/dev/null; then GUIDANCE_TARGET="$URL"; fi
else
  for sec in pr checks reviews; do put "$sec" "" "the run has no pull request yet"; done
  put repository "" "read from the pull request once it exists"
  # The base before a pull request exists: fetched by refspec, compared locally.
  BE=""; BJ=""
  if [ -z "$BASE_NAME" ] || [ -z "$LOCAL_HEAD" ] || [ "$HEAD_SRC" = launch-checkout ]; then BE="the base or the run branch's local head is not known"
  elif [ -n "$RUN_ERR" ] && [ -z "$BASE_SHA" ]; then BE="$RUN_ERR"
  elif ! goal_is_sha "$BASE_SHA"; then BE="origin has no branch ${BASE_NAME}"
  else
    OB="refs/remotes/origin/${BASE_NAME}"
    if ! git -C "$LAUNCH" cat-file -e "${BASE_SHA}^{commit}" 2>/dev/null; then
      git -C "$LAUNCH" fetch --quiet --no-tags origin "+refs/heads/${BASE_NAME}:${OB}" 2>"$S/fetch.err" || BE="fetching the base failed: $(err "$S/fetch.err")"
    fi
    if [ -z "$BE" ] && MB="$(git -C "$LAUNCH" merge-base "$LOCAL_HEAD" "$BASE_SHA" 2>"$S/mb.err")"; then
      git -C "$LAUNCH" merge-tree --write-tree --name-only --no-messages "$BASE_SHA" "$LOCAL_HEAD" > "$S/mt" 2>"$S/mt.err"; mrc=$?
      [ "$mrc" -le 1 ] || BE="git merge-tree failed (exit ${mrc}): $(err "$S/mt.err")"
      if [ "$mrc" -eq 1 ]; then tail -n +2 "$S/mt" | grep . | sort -u | head -n 200 > "$S/conflicts"; else : > "$S/conflicts"; fi
      git -C "$LAUNCH" diff --name-only --no-renames "$MB" "$BASE_SHA" 2>/dev/null | sort -u > "$S/based"
      git -C "$LAUNCH" diff --name-only --no-renames "$MB" "$LOCAL_HEAD" 2>/dev/null | sort -u > "$S/mine"
      comm -12 "$S/based" "$S/mine" | head -n 200 > "$S/overlap"
      STRICT=null; SWHY=""
      if [ -n "$BUILD_REPO_NAME" ]; then
        base_path="$(jq -nr --arg ref "$BASE_NAME" '$ref | @uri')"
        gh_goal_read core true api --include "repos/${BUILD_REPO_NAME}/rules/branches/${base_path}"; rc=$?; passup "$rc"
        if [ "$rc" -eq 0 ] && jq -e 'type == "array"' "$GH_READ_BODY" >/dev/null 2>&1; then
          if jq -e 'any(.[]; .type == "required_status_checks" and .parameters.strict_required_status_checks_policy == true)' "$GH_READ_BODY" >/dev/null; then STRICT=true; else STRICT=false; fi
        else SWHY="rules endpoint: ${GH_READ_ERROR:-unreadable response}"; fi
        if [ "$STRICT" != true ]; then
          gh_goal_read core true api --include "repos/${BUILD_REPO_NAME}/branches/${base_path}/protection/required_status_checks"; rc=$?; passup "$rc"
          prot=""
          [ "$rc" -ne 0 ] || prot="$(jq -r '.strict | booleans' "$GH_READ_BODY" 2>/dev/null)"
          case "$prot" in true|false) STRICT="$prot" ;; *) SWHY="${SWHY:+$SWHY; }classic protection: ${GH_READ_ERROR:-unreadable response}" ;; esac
        fi
      fi
      BJ="$(jq -cn --arg b "$BASE_NAME" --arg mb "$MB" --arg bs "$BASE_SHA" \
        --argjson behind "$(git -C "$LAUNCH" rev-list --count "${LOCAL_HEAD}..${BASE_SHA}" 2>/dev/null || echo null)" \
        --argjson ahead "$(git -C "$LAUNCH" rev-list --count "${BASE_SHA}..${LOCAL_HEAD}" 2>/dev/null || echo null)" \
        --rawfile c "$S/conflicts" --rawfile o "$S/overlap" --argjson strict "$STRICT" --arg sw "$SWHY" \
        '{branch:$b, head:$bs, merge_base:$mb, behind_by:$behind, ahead_by:$ahead,
          conflict_files:($c | split("\n") | map(select(length > 0))), base_changed_my_files:($o | split("\n") | map(select(length > 0))),
          strict_status_checks:$strict, strict_unreadable:(if $strict == null then $sw else null end)}')"
    elif [ -z "$BE" ]; then BE="no merge base between the run branch and ${BASE_NAME}: $(err "$S/mb.err")"; fi
  fi
  put base "$BJ" "$BE"
fi

# --- the issue and its claims -----------------------------------------------------
build_task_text "$S/task"
# shellcheck disable=SC2034  # read by tracking_issue_reference from the caller scope
IREF="$( TASK_DESC="$(cat "$S/task")" EXISTING_ISSUE_NUMBER="${RECIPE_VAR_issue_number:-}"
  # shellcheck source=workflow_issue_tracking.sh
  . "$HERE/workflow_issue_tracking.sh" && tracking_issue_reference 2>/dev/null )" || IREF=""
INUM=""; ISRC=none; IE=""
case "$IREF" in
  none) ;;
  [0-9]*' '*) INUM="${IREF%% *}"; ISRC="${IREF#* }" ;;
  *) IE="tracking_issue_reference gave no answer" ;;
esac
[[ "$INUM" =~ ^[0-9]{1,10}$ ]] || INUM=""
CLAIMS='[]'; CRES=""
if [ -n "$INUM" ]; then
  args=(--json --issue "$INUM" --repo-path "$LAUNCH"); [ -z "$BRANCH" ] || args+=(--head "$BRANCH")
  # The repository the build writes to (origin as configured, before insteadOf
  # rewriting), so the claims are read from the same place. A host other than
  # github.com is left to the claim check's own reading of origin.
  [ -z "$BUILD_REPO_NAME" ] || [ "$BUILD_REPO" != "$BUILD_REPO_NAME" ] || args+=(--repo "$BUILD_REPO_NAME")
  # TMPDIR is this reading's scratch folder, so the claim check's file for gh's
  # error text stays inside the state folder (S-D3).
  AUTODRIVE_GUIDANCE_MARK="$GUIDANCE_SINCE" AUTODRIVE_GUIDANCE_TARGET="$GUIDANCE_TARGET" \
    gh_goal_command env -u ALLOW_EXISTING_PR -u AMPLIHACK_ALLOW_EXISTING_PR TMPDIR="$S" bash "$HERE/workflow_issue_claim_check.sh" \
    --goal-reading "${args[@]}" > "$S/claim" 2>"$S/claim.err"; rc=$?; passup "$rc"
  line="$(tail -n 1 "$S/claim")"
  if [ "$rc" -eq 0 ] && printf '%s' "$line" | jq -e 'type == "object" and (.claims | type) == "array"' >/dev/null 2>&1; then
    CRES="$(printf '%s' "$line" | jq -r '.result // empty | strings')"
    REC="$(jq -r '.number // empty | tostring' "$STATE/pr.json" 2>/dev/null)"
    CLAIMS="$(printf '%s' "$line" | jq -c --arg b "$BRANCH" --arg rec "$REC" --arg ctx "${RECIPE_VAR_pr_number:-}" '[.claims[] | objects
      | {number, url, state, head:((.head // "") | tostring | gsub("[[:cntrl:]]"; "") | .[0:255]), via, source, own}
      | . + {relation:(if $b != "" and .head == $b then "run-branch" elif ($rec != "" and (.number | tostring) == $rec) then "recorded-pr"
          elif ($ctx != "" and (.number | tostring) == $ctx) then "context-pr" else "other-branch" end)}]')"
    case "$CRES" in unknown|skipped) IE="the claim check gave $(printf '%s' "$line" | jq -r '.reason // "no reason" | tostring' | goal_clean_line 200)" ;; esac
  else
    IE="the claim check gave no answer (exit ${rc}): $(err "$S/claim.err")"
  fi
fi
put issue "$(jq -cn --arg n "$INUM" --arg s "$ISRC" --argjson c "$CLAIMS" --arg r "$CRES" \
  '{number:(if $n == "" then null else ($n | tonumber) end), source:$s, claim_result:(if $r == "" then null else $r end), claims:$c}')" "$IE"

# --- host inputs ------------------------------------------------------------------
HJ=""; HE=""
if [ ! -f "$HERE/workflow_worktree_host_inputs.sh" ]; then HE="workflow_worktree_host_inputs.sh is missing from the bundle"
elif [ "$WT_OK" = true ]; then
  HJ="$(bash "$HERE/workflow_worktree_host_inputs.sh" facts "$LAUNCH" "$WT" 2>"$S/hi.err")" || { HJ=""; HE="the host-input facts could not be read: $(err "$S/hi.err")"; }
else
  HJ="$(bash "$HERE/workflow_worktree_host_inputs.sh" facts "$LAUNCH" "$LAUNCH" 2>"$S/hi.err" | jq -c '{launch, worktree:null, missing:null}' 2>/dev/null)" \
    || { HJ=""; HE="the host-input facts could not be read: $(err "$S/hi.err")"; }
  [ -z "$HJ" ] || HE="the run has no usable worktree (${ID_CHECK}), so only the launch checkout is listed"
fi
put host_inputs "$HJ" "$HE"

# --- checks the agent reported at the current local head ----------------------------
CF="$STATE/checks.json"; CJ=""; CE=""
if [ -z "$LOCAL_HEAD" ]; then CE="the local head is not known"
elif [ ! -e "$CF" ]; then CJ="$(jq -cn '{entries:[], ignored:0}')"
elif [ -L "$CF" ] || [ ! -f "$CF" ] || [ "$(wc -c < "$CF" | tr -d ' ')" -gt 262144 ]; then CE="checks.json is not a regular file of at most 256 KiB"
else
  CJ="$(jq -c --arg h "$LOCAL_HEAD" '(if type == "array" then . elif type == "object" and (.entries | type) == "array" then .entries else null end) as $all
    | if $all == null then error("checks.json is neither an array nor an object with entries") else
      ($all | map(type == "object" and (.check | type) == "string" and (.check | length) > 0 and (.check | length) <= 200
        and (.head | type) == "string" and (.head | test("^([0-9a-f]{40}|[0-9a-f]{64})$")) and (.outcome | type) == "string")) as $ok
      | {entries:[range(0; $all | length) as $i | select($ok[$i]) | $all[$i] | select(.head == $h)
          | {check:(.check | gsub("[[:cntrl:]]"; " ")), head, outcome:(.outcome | .[0:60]),
             uncommitted:(.uncommitted // null), time:(.time // null)}] | .[-100:],
         ignored:($ok | map(select(. | not)) | length)} end' "$CF" 2>"$S/cj.err")" \
    || { CJ=""; CE="checks.json could not be read: $(err "$S/cj.err")"; }
fi
[ -z "$CJ" ] || CJ="$(printf '%s' "$CJ" | jq -c --arg h "$LOCAL_HEAD" --arg f "$CF" '. + {reported_by:"agent", head:$h, file:$f}')"
[ -n "$CJ" ] || CJ="$(jq -cn --arg f "$CF" '{reported_by:"agent", file:$f, entries:null, ignored:null}')"
put checks_passed "$CJ" "$CE"

# --- the intended pull request text, and whether the live text equals it -----------
TJ=""; TE=""
if [ -f "$STATE/pr-text/title" ] && [ -f "$STATE/pr-text/body" ]; then
  EQ=null
  if [ -n "$OWN_PR" ]; then
    gh_goal_read graphql false pr view "$OWN_PR" --json title,body; rc=$?; passup "$rc"
    if [ "$rc" -eq 0 ] && jq -e '(.title | type) == "string" and (.body | type) == "string"' "$GH_READ_BODY" >/dev/null 2>&1; then
      jq -j .title "$GH_READ_BODY" > "$S/lt"; jq -j .body "$GH_READ_BODY" > "$S/lb"
      # shellcheck source=workflow_pr_text.sh
      if . "$HERE/workflow_pr_text.sh" && pr_text_same "$STATE/pr-text/title" "$STATE/pr-text/body" "$S/lt" "$S/lb"; then EQ=true; else EQ=false; fi
    else TE="the live text could not be read: ${GH_READ_ERROR:-unreadable response}"; fi
  fi
  TJ="$(jq -cn --rawfile t "$STATE/pr-text/title" --rawfile b "$STATE/pr-text/body" --argjson eq "$EQ" --arg d "$STATE/pr-text" \
    '{intended:{title:$t, body_excerpt:($b | .[0:2000]), body_bytes:($b | utf8bytelength), folder:$d}, live_equals_intended:$eq}')"
else
  TJ='{"intended":null,"live_equals_intended":null}'
fi
put pr_text "$TJ" "$TE"

# --- context ----------------------------------------------------------------------
put context "$(jq -cn --arg eb "${RECIPE_VAR_existing_branch:-}" --arg ae "${RECIPE_VAR_allow_existing_pr:-}" --arg pn "${RECIPE_VAR_pr_number:-}" \
  --arg in "${RECIPE_VAR_issue_number:-}" --arg bb "${RECIPE_VAR_base_branch:-}" --arg cr "${RECIPE_VAR_closing_reference:-}" \
  --arg sd "${RECIPE_VAR_strict_delivery:-}" --arg an "${RECIPE_VAR_allow_no_op:-}" --arg pr "${RECIPE_VAR_pr_repository:-}" \
  '{existing_branch:$eb, allow_existing_pr:$ae, pr_number:$pn, issue_number:$in, base_branch:$bb, closing_reference:($cr | .[0:500]),
    strict_delivery:$sd, allow_no_op:$an, pr_repository:$pr}')" ""

# --- goal.json -----------------------------------------------------------------------
OUT_WT="$STATE/no-worktree"; [ "$WT_OK" != true ] || OUT_WT="$WT"
GT=""; [[ "$URL" =~ ^https://[A-Za-z0-9.-]+/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+/pull/[0-9]+$ ]] && GT="$URL"
jq -n --slurpfile run "$S/run.json" --slurpfile prs "$S/prs.json" --slurpfile pr "$S/pr.json" --slurpfile checks "$S/checks.json" \
  --slurpfile reviews "$S/reviews.json" --slurpfile repository "$S/repository.json" --slurpfile base "$S/base.json" \
  --slurpfile issue "$S/issue.json" --slurpfile hi "$S/host_inputs.json" --slurpfile cp "$S/checks_passed.json" \
  --slurpfile text "$S/pr_text.json" --slurpfile ctx "$S/context.json" --arg own "$OWN_PR" --arg gt "$GT" --arg wt "$OUT_WT" \
  --arg pst "$PR_STATE" --arg pd "$PR_DRAFT" '
  ($run[0]) as $r | ($issue[0]) as $i | ($hi[0]) as $h | ($checks[0].counts // null) as $c |
  {schema:1, schema_version:1,
   subject:(if $own != "" then "PR #" + $own elif $r.branch then "branch " + $r.branch else "the build run" end),
   worktree:$wt, untrusted:[".prs.candidates", ".pr", ".checks", ".reviews", ".issue.claims", ".pr_text.intended"],
   run:$r, prs:$prs[0], pr:$pr[0], checks:$checks[0], reviews:$reviews[0], repository:$repository[0], base:$base[0],
   issue:$i, host_inputs:$h, checks_passed:$cp[0], pr_text:$text[0], context:$ctx[0],
   summary:((if $r.branch then "branch " + $r.branch else "no run branch known" end)
     + "; worktree " + (if $r.worktree.present then "present" else "missing (" + $r.identity_check + ")" end)
     + "; local head " + (($r.local_head // "unknown")[0:12])
     + (if $r.commits_not_pushed != null then ", \($r.commits_not_pushed) commits not pushed" else "" end)
     + (if $r.uncommitted_changes != null then ", \($r.uncommitted_changes) uncommitted changes" else "" end)
     + (if $own != "" then "; PR #" + $own + " " + (if $pst == "" then "unreadable" else $pst end) + (if $pd == "true" then " draft" elif $pd == "false" then " ready" else "" end)
          + (if $c == null then "" elif $c.total == 0 then ", checks pending (none reported)"
            else ", checks \($c.passed // 0) passed, \($c.failing // 0) failing, \($c.pending // 0) pending" end)
        elif $prs[0].error then "; pull requests unreadable" else "; no pull request yet" end)
     + (if $i.number != null then "; issue #\($i.number) (\($i.source)), \($i.claims | length) claims" else "" end)
     + (if ($h.missing | type) == "array" and ($h.missing | length) > 0 then "; host inputs missing from the worktree: " + ($h.missing | join(", ")) else "" end)
     + "; \(($cp[0].entries // []) | length) checks reported at this head") | .[0:600]}
  + (if $gt == "" then {} else {guidance_target:$gt} end)' | goal_atomic_write "$TD/goal.json"
