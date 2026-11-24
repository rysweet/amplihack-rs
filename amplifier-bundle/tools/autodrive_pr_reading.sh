#!/usr/bin/env bash
# autodrive_pr_reading.sh <turn_dir>: the auto-drive goal's fresh reading of
# a pull request (#3129). Called by autodrive_pr_goal.sh `read`.
#
# Writes <turn_dir>/goal.json, schema 1, with a one-line `summary`, `subject`,
# `guidance_target` (the PR URL), `worktree`, and these sections, each with
# its own `error` (null when the read worked):
#   repo             the PR's linked worktree (autodrive_pr_worktree.sh
#                    resolve), the current branch, whether it is clean, local
#                    HEAD, commits not yet pushed
#   merge_authority  from the captured invocation only: allows_merge,
#                    pr_selected, no_merge, dry_run, effective
#   pr               head, base, state, mergeable, mergeStateStatus, draft,
#                    cross_repository, url
#   checks           each check at the head with its state, counts, and
#                    required_known (false when gh rejects --required)
#   reviews          GitHub reviews and Crusty round records, each with the
#                    head it was given at; a round that could not be read is
#                    listed with its own error. Each minor finding carries
#                    its outcome (fixed, filed or open) from the ledger
#   base             merge base, behind_by, conflict_files (git merge-tree
#                    --write-tree), and the files the base changed since the
#                    merge base that the PR also touches
#   repository       allowed merge methods; strict status checks from the
#                    rules endpoint, then classic protection. A policy that
#                    was read wins. When neither endpoint can be read the
#                    policy stays unknown (null), with strict_unreadable
#                    saying why. mergeStateStatus BEHIND is reported beside it
#                    as merge_state_behind, evidence only: it never sets or
#                    replaces the policy (#3141)
#   autodrive_minor_findings        fix_all, judge or file_all (#3160), with
#   autodrive_minor_findings_error  null, or why an unknown value became fix_all
#
# Fetching the base and head refs is allowed (refspec fetches only); nothing
# here changes the worktree. A read that fails records its error and never
# ends the turn. Exit 79 from gh passes up unchanged; any other failure exits
# 0 with the error in its section. docs/reference/autodrive-goal-loop.md.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 2
# shellcheck source=autodrive_goal_lib.sh
. "$HERE/autodrive_goal_lib.sh" || exit 2
AMPLIHACK_BIN="${AMPLIHACK_BIN:-amplihack}"
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat GIT_TERMINAL_PROMPT=0
umask 077
TD="${1:-}"
goal_check_turn_dir "$TD" || exit 2
PR="${RECIPE_VAR_pr_number:-}"; STATE="${AUTODRIVE_STATE_DIR:-}"; REPO="${AUTODRIVE_REPO:-$PWD}"
S="$(mktemp -d "$TD/.reading.XXXXXX")" || exit 1
trap 'rm -rf -- "$S"' EXIT
g() { git -c core.fsmonitor=false -C "$WORK" "$@"; }
err() { goal_clean_line 300 < "$1" 2>/dev/null; }
put() { # put <section> <json object or empty> <error>
  if [ -n "$2" ] && printf '%s' "$2" | jq -e 'type == "object"' >/dev/null 2>&1; then
    printf '%s' "$2" | jq -c --arg e "$3" '. + {error:(if $e == "" then null else $e end)}' > "$S/$1.json"
  else
    jq -cn --arg e "${3:-the read gave no usable answer}" '{error:$e}' > "$S/$1.json"
  fi
}
gh79() { [ "$1" -ne 79 ] || exit 79; }

# --- repo: the PR's own worktree, else the run's checkout --------------------
WORK="$REPO"; WT_ERR=""
if goal_is_pr "$PR"; then
  wt="$(bash "$HERE/autodrive_pr_worktree.sh" resolve --repo "$REPO" --pr "$PR" 2>"$S/wt.err")"; rc=$?; gh79 "$rc"
  if [ "$rc" -eq 0 ] && [[ "$wt" == /* ]] && [ -d "$wt" ]; then WORK="$wt"; else WT_ERR="the PR's linked worktree was not resolved, so the run's checkout is used: $(err "$S/wt.err")"; fi
fi

# --- merge authority: the captured invocation only --------------------------
AUTH=""; AUTH_ERR=""
if [ -n "${AUTODRIVE_INVOCATION_FILE:-}" ] && goal_is_pr "$PR"; then
  AUTH="$("$AMPLIHACK_BIN" orch helper autodrive authorization --invocation "$AUTODRIVE_INVOCATION_FILE" --directory "$STATE" --pr "$PR" 2>"$S/auth.err")" \
    || { AUTH=""; AUTH_ERR="authorization-unreadable: $(err "$S/auth.err")"; }
else
  AUTH_ERR="authorization-unreadable: no captured invocation record or no pull request number"
fi
# The run's context can narrow the captured authority, never grant it.
CTX_OFF=false
{ [ "${RECIPE_VAR_no_merge:-}" = false ] && [ "${RECIPE_VAR_autodrive_dry_run:-}" = false ]; } || CTX_OFF=true
if [ -n "$AUTH" ]; then
  put merge_authority "$(printf '%s' "$AUTH" | jq -c --argjson off "$CTX_OFF" '{allows_merge, pr_selected:(.pr_selected // false), no_merge:(.no_merge // null),
    dry_run:(.dry_run // null), narrowed_by_context:$off, effective:((.allows_merge == true) and (.pr_selected == true) and ($off | not))}' 2>/dev/null)" ""
else
  put merge_authority '{"allows_merge":false,"pr_selected":false,"effective":false}' "$AUTH_ERR"
fi

# --- the pull request -------------------------------------------------------
HEAD=""; BASE=""; BRANCH=""; URL=""; MSS=""; PR_READ=false
if goal_is_pr "$PR"; then
  gh pr view "$PR" --json number,state,headRefOid,headRefName,baseRefName,mergeable,mergeStateStatus,isDraft,isCrossRepository,url,reviews \
    > "$S/pr.raw" 2>"$S/pr.err"; rc=$?; gh79 "$rc"
  if [ "$rc" -eq 0 ] && jq -e 'type == "object"' "$S/pr.raw" >/dev/null 2>&1; then
    PR_READ=true
    put pr "$(jq -c '{number, state, head:.headRefOid, head_branch:.headRefName, base:.baseRefName, mergeable,
      merge_state:.mergeStateStatus, draft:.isDraft, cross_repository:.isCrossRepository, url}' "$S/pr.raw")" ""
    HEAD="$(jq -r '.headRefOid // empty | strings' "$S/pr.raw")"; BASE="$(jq -r '.baseRefName // empty | strings' "$S/pr.raw")"
    BRANCH="$(jq -r '.headRefName // empty | strings' "$S/pr.raw")"; URL="$(jq -r '.url // empty | strings' "$S/pr.raw")"
    MSS="$(jq -r '.mergeStateStatus // empty | strings' "$S/pr.raw")"
  else
    put pr "" "gh pr view failed (exit ${rc}): $(err "$S/pr.err")"
  fi
else
  put pr "" "no pull request number was given (pr_number)"
fi
goal_is_sha "$HEAD" || HEAD=""

# --- checks at the head -----------------------------------------------------
parse_checks() { jq -R -s -c 'split("\n") | map(select(length > 0) | split("\t") | {name:.[0], state:(.[1] // ""), elapsed:(.[2] // ""), link:(.[3] // "")}) | .[:200]'; }
if goal_is_pr "$PR"; then
  KNOWN=true
  gh pr checks "$PR" --required > "$S/checks.raw" 2>"$S/checks.err"; rc=$?; gh79 "$rc"
  if [ "$rc" -ne 0 ] && [ "$rc" -ne 8 ] && [ ! -s "$S/checks.raw" ] && grep -qiE 'required|unknown flag' "$S/checks.err"; then
    KNOWN=false
    gh pr checks "$PR" > "$S/checks.raw" 2>"$S/checks.err"; rc=$?; gh79 "$rc"
  fi
  if [ "$rc" -eq 0 ] || [ "$rc" -eq 1 ] || [ "$rc" -eq 8 ]; then
    CE=""; [ -s "$S/checks.raw" ] || CE="$(err "$S/checks.err")"
    put checks "$(parse_checks < "$S/checks.raw" | jq -c --argjson k "$KNOWN" --argjson rc "$rc" '{required_known:$k, gh_exit:$rc, checks:.,
      counts:{total:length, passed:(map(select(.state == "pass")) | length), failing:(map(select(.state == "fail" or .state == "cancel")) | length),
        pending:(map(select(.state == "pending")) | length), skipped:(map(select(.state == "skipping")) | length)}}')" "$CE"
  else
    put checks "" "gh pr checks failed (exit ${rc}): $(err "$S/checks.err")"
  fi
else
  put checks "" "no pull request number"
fi

# --- reviews: GitHub's and the Crusty rounds' --------------------------------
# One line per round, read once with --slurpfile. The list never goes through
# argv, where Linux caps one argument at 128 KiB: a long run's rounds would
# pass that, and the whole section would be lost. A round that cannot be read
# is listed by name with its error, so it never reads as "no verdict".
#
# #3160: each minor finding shows its outcome from the run's outcome ledger,
# joined on the name minor:<id> for display only: fixed or filed when that
# entry is met with that treatment, else open. A ledger that cannot be read
# shows every finding open; the rest of the reading is kept. The ledger is
# indexed by name once here (the last entry of a name wins), so each round
# looks its findings up instead of scanning the whole ledger per finding.
if [ -n "$STATE" ] && goal_json_file "$STATE/outcomes.json" 524288 \
  && jq -c '.outcomes | if type == "array" then . else [] end
    | reduce (.[] | objects | select(.name | type == "string")) as $x ({}; .[$x.name] = $x)' \
    "$STATE/outcomes.json" > "$S/ledger.json" 2>/dev/null; then :
else echo '{}' > "$S/ledger.json"; fi
MINOR_JQ='($led[0] // {}) as $L
  | [.[] | objects | {id, summary}] | .[:50]
  | map(. as $m
    | (if $m.id == null then null else ($m.id | tostring | .[0:64]) end) as $id
    | (if $id == null then null else $L["minor:" + $id] end) as $e
    | (if $e.status == "met" and ($e.treatment == "fixed" or $e.treatment == "filed") then $e.treatment else "open" end) as $o
    | {id:$id, summary:$m.summary, outcome:$o}
      + (if ($e.evidence | type) == "string" and $e.evidence != "" then {evidence:($e.evidence | .[0:300])} else {} end)
      + (if ($e.reason | type) == "string" and $e.reason != "" then {reason:($e.reason | .[0:300])} else {} end))'
: > "$S/crusty.jsonl"
round_err() { jq -cn --arg n "${1##*/}" --arg e "$2" '{record:$n, error:$e}' >> "$S/crusty.jsonl"; }
for f in "$STATE"/crusty-round-*.json; do
  [ -e "$f" ] || [ -L "$f" ] || continue
  goal_json_file "$f" 65536 || { round_err "$f" "the round record is not a regular JSON object file of at most 64 KiB"; continue; }
  MINOR='[]'; ME=""
  if [ -f "$f.minor" ] && [ ! -L "$f.minor" ]; then
    MINOR="$(head -c 65536 -- "$f.minor" | jq -c -s --slurpfile led "$S/ledger.json" "$MINOR_JQ" 2>/dev/null)" && [ -n "$MINOR" ] \
      || { MINOR='[]'; ME="the round's minor findings could not be read"; }
  elif [ -e "$f.minor" ] || [ -L "$f.minor" ]; then
    ME="the round's minor findings could not be read"
  fi
  jq -c --arg n "${f##*/}" --argjson m "$MINOR" --arg me "$ME" '{record:$n, verdict:.crusty_verdict, head:.head_sha, reviewed_head:.reviewed_head_sha,
    concerns:.concern_count, minor_findings:$m} + (if $me == "" then {} else {error:$me} end)' "$f" >> "$S/crusty.jsonl" 2>/dev/null \
    || round_err "$f" "the round record could not be read"
done
# GitHub's reviews come from the pull request read: when that failed they are
# unknown, not none, and the section says so.
RVE="the pull request's GitHub reviews could not be read"
if [ "$PR_READ" = true ] && RV="$(jq -cn --slurpfile cr "$S/crusty.jsonl" --slurpfile pr "$S/pr.raw" '{github:([($pr[0].reviews // [])[] | {author:(.author.login // null),
    state, submitted_at:(.submittedAt // null), head:(.commit.oid // null)}]), crusty:$cr}' 2>/dev/null)"; then RVE=""
else RV="$(jq -cn --slurpfile cr "$S/crusty.jsonl" '{github:[], crusty:$cr}')"; fi
put reviews "$RV" "$RVE"

# --- the base: fetch, merge base, conflicts, overlapping files ---------------
ref_ok() { case "$1" in ''|-*|*..*) return 1 ;; esac; git check-ref-format "refs/heads/$1" >/dev/null 2>&1; }
BE=""; BJ=""
if [ -z "$HEAD" ] || [ -z "$BASE" ]; then BE="the pull request's head or base could not be read"
elif ! ref_ok "$BASE"; then BE="the base branch name was refused (it starts with '-', holds '..', or fails git check-ref-format); git was not run with it"
elif ! g fetch --quiet --no-tags origin "refs/heads/$BASE:refs/remotes/origin/$BASE" 2>"$S/fetch.err"; then BE="fetching the base failed: $(err "$S/fetch.err")"
else
  OB="refs/remotes/origin/$BASE"
  if ! g cat-file -e "$HEAD^{commit}" 2>/dev/null && ref_ok "$BRANCH"; then
    g fetch --quiet --no-tags origin "refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" 2>/dev/null || :
  fi
  if ! MB="$(g merge-base "$HEAD" "$OB" 2>"$S/mb.err")"; then BE="no merge base between the head and ${BASE}: $(err "$S/mb.err")"
  else
    g merge-tree --write-tree --name-only --no-messages "$OB" "$HEAD" > "$S/mt" 2>"$S/mt.err"; mrc=$?
    [ "$mrc" -le 1 ] || BE="git merge-tree failed (exit ${mrc}): $(err "$S/mt.err")"
    [ "$mrc" -eq 1 ] && tail -n +2 "$S/mt" | grep . | sort -u | head -n 200 > "$S/conflicts" || : > "$S/conflicts"
    # --no-renames: a renamed file is listed under both its paths, so a base
    # rename of a file the PR touches still overlaps. Rename detection is
    # also about 13 times slower over a long range.
    g diff --name-only --no-renames "$MB" "$OB" | sort -u > "$S/based" 2>/dev/null
    g diff --name-only --no-renames "$MB" "$HEAD" | sort -u > "$S/prfiles" 2>/dev/null
    comm -12 "$S/based" "$S/prfiles" | head -n 200 > "$S/overlap"
    BJ="$(jq -cn --arg b "$BASE" --arg mb "$MB" --argjson behind "$(g rev-list --count "$HEAD..$OB" 2>/dev/null || echo null)" \
      --argjson ahead "$(g rev-list --count "$OB..$HEAD" 2>/dev/null || echo null)" \
      --rawfile c "$S/conflicts" --rawfile o "$S/overlap" \
      '{branch:$b, merge_base:$mb, behind_by:$behind, ahead_by:$ahead, conflict_files:($c | split("\n") | map(select(length > 0))),
        base_changed_pr_files:($o | split("\n") | map(select(length > 0)))}')"
  fi
fi
put base "$BJ" "$BE"

# --- the repository's settings ----------------------------------------------
OWNER=""; NAME=""
if [[ "$URL" =~ ^https://github\.com/([A-Za-z0-9_-]+)/([A-Za-z0-9._-]+)/pull/[0-9]+$ ]]; then OWNER="${BASH_REMATCH[1]}"; NAME="${BASH_REMATCH[2]}"; fi
RE=""; RJ='{}'
if [ -z "$OWNER" ]; then RE="the repository could not be named from the PR URL"
else
  gh api "repos/$OWNER/$NAME" --jq '{merge:.allow_merge_commit, squash:.allow_squash_merge, rebase:.allow_rebase_merge}' > "$S/methods" 2>"$S/methods.err"; rc=$?; gh79 "$rc"
  M="$(jq -c 'select(type == "object")' "$S/methods" 2>/dev/null)"; [ -n "$M" ] || { M=null; RE="the allowed merge methods could not be read: $(err "$S/methods.err")"; }
  STRICT=null; SRC=""; WHY=""; RULES_READ=false
  if ref_ok "$BASE"; then
    gh api "repos/$OWNER/$NAME/rules/branches/$BASE" > "$S/rules" 2>"$S/rules.err"; rc=$?; gh79 "$rc"
    if [ "$rc" -eq 0 ] && jq -e 'type == "array"' "$S/rules" >/dev/null 2>&1; then
      RULES_READ=true
      if jq -e 'any(.[]; .type == "required_status_checks" and .parameters.strict_required_status_checks_policy == true)' "$S/rules" >/dev/null; then STRICT=true; SRC=rules; fi
    else WHY="rules endpoint: $(err "$S/rules.err")"; fi
    if [ "$STRICT" != true ]; then
      gh api "repos/$OWNER/$NAME/branches/$BASE/protection/required_status_checks" --jq .strict > "$S/prot" 2>"$S/prot.err"; rc=$?; gh79 "$rc"
      case "$rc:$(tr -d '[:space:]' < "$S/prot")" in
        0:true) STRICT=true; SRC=protection ;;
        0:false) STRICT=false; SRC=protection ;;
        *) WHY="${WHY:+$WHY; }classic protection: $(err "$S/prot.err")" ;;
      esac
      if [ "$STRICT" = null ] && [ "$RULES_READ" = true ]; then STRICT=false; SRC="rules (no strict status check rule)"; fi
    fi
  else WHY="the base name was refused"; fi
  # #3141: BEHIND is evidence beside the policy, never the policy. GitHub can
  # report BEHIND under a policy that does not require an up-to-date branch,
  # so it neither overrides a policy that was read nor fills in one that was
  # not. null: the pull request itself could not be read.
  BEHIND_EV=null
  [ "$PR_READ" != true ] || { if [ "$MSS" = BEHIND ]; then BEHIND_EV=true; else BEHIND_EV=false; fi; }
  RJ="$(jq -cn --argjson m "$M" --argjson s "$STRICT" --arg src "$SRC" --arg why "$WHY" --argjson behind "$BEHIND_EV" \
    '{merge_methods:$m, strict_status_checks:$s, strict_source:(if $src == "" then null else $src end), strict_unreadable:(if $s == null then $why else null end),
      merge_state_behind:$behind}')"
fi
put repository "$RJ" "$RE"

# --- the worktree's own state -------------------------------------------------
LB="$(g symbolic-ref --short -q HEAD 2>/dev/null)"; LH="$(g rev-parse --verify -q HEAD 2>/dev/null)"
DIRTY="$(g status --porcelain --untracked-files=normal 2>/dev/null | head -c 4096)"
UNPUSHED=null
if [ -n "$LB" ] && g rev-parse --verify -q "refs/remotes/origin/$LB" >/dev/null; then UNPUSHED="$(g rev-list --count "refs/remotes/origin/$LB..HEAD" 2>/dev/null || echo null)"; fi
put repo "$(jq -cn --arg w "$WORK" --arg b "$LB" --arg h "$LH" --arg d "$DIRTY" --argjson u "$UNPUSHED" \
  '{worktree:$w, branch:$b, local_head:$h, clean:($d == ""), unpushed_commits:$u}')" "$WT_ERR"

# --- the setting for minor findings (#3160) ---------------------------------
# fix_all when empty or absent (drives started on an earlier release). An
# unknown value is reported, never refused here: fix_all applies, and the
# error quotes the cleaned value. The top-level recipes refuse it at the start.
MF="${RECIPE_VAR_autodrive_minor_findings:-}"
MF="${MF#"${MF%%[![:space:]]*}"}"; MF="${MF%"${MF##*[![:space:]]}"}"
MF_ERR=""
case "$MF" in
  ""|fix_all) MF=fix_all ;;
  judge|file_all) ;;
  *) MF_ERR="autodrive_minor_findings must be fix_all, judge or file_all (got '$(printf '%s' "$MF" | LC_ALL=C tr -cd '[:print:]' | head -c 80)'); fix_all applies"
     MF=fix_all ;;
esac

# --- goal.json ----------------------------------------------------------------
jq -n --slurpfile repo "$S/repo.json" --slurpfile ma "$S/merge_authority.json" --slurpfile pr "$S/pr.json" \
  --slurpfile checks "$S/checks.json" --slurpfile reviews "$S/reviews.json" --slurpfile base "$S/base.json" \
  --slurpfile repository "$S/repository.json" --arg prn "$PR" --arg url "$URL" --arg work "$WORK" --arg mf "$MF" --arg mferr "$MF_ERR" '
  ($pr[0]) as $p | ($checks[0].counts // {}) as $c | ($base[0]) as $b |
  {schema_version:1, subject:("pull request #" + $prn), guidance_target:(if ($url | test("^https://github\\.com/[^/]+/[^/]+/pull/[0-9]+$")) then $url else null end),
   worktree:$work, repo:$repo[0], merge_authority:$ma[0], pr:$p, checks:$checks[0], reviews:$reviews[0], base:$b, repository:$repository[0],
   autodrive_minor_findings:$mf, autodrive_minor_findings_error:(if $mferr == "" then null else $mferr end),
   summary:((if $p.error then "PR #" + $prn + ": the pull request could not be read (" + $p.error + ")" else
     "PR #" + $prn + " " + ($p.state // "?") + " at " + (($p.head // "?")[0:12]) + " (" + ($p.head_branch // "?") + " into " + ($p.base // "?") + "), mergeStateStatus " + ($p.merge_state // "?")
     + "; checks: " + (if $checks[0].error then "unreadable" else "\($c.passed // 0) passed, \($c.failing // 0) failing, \($c.pending // 0) pending" end)
     + "; base: " + (if $b.error then "unreadable" else "\($b.behind_by // "?") behind, \($b.conflict_files | length) conflicting files" end)
     + "; merging " + (if $ma[0].effective then "allowed" else "off" end) end) + "; minor findings: " + $mf)}' | goal_atomic_write "$TD/goal.json"
