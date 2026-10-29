#!/usr/bin/env bash
# Open-PR signal for resumed-run finalization (issue #1641).
#
# finalization_open_pr_signal <target_dir> <branch> [<pr_ref>]
#
# Reports facts only; classification stays in workflow_finalization_validate.sh.
# On success it exits 0 and prints one compact JSON line:
#   {"pr_number":"<n>","pr_url":"https://github.com/<owner>/<repo>/pull/<n>",
#    "checks":"green|failing|pending|none|unknown","head":"local|descendant"}
# Any missing, ambiguous or unverifiable fact exits 1, prints nothing on
# stdout and one fixed "WARNING: no open-PR signal: <reason>" line on stderr.
#
# The PR head commit (headRefOid) must be the local HEAD ("head":"local") or
# a fast-forward of it ("head":"descendant", issue #1714): the local HEAD is
# an ancestor of the PR head, as when a merge of main was pushed from another
# checkout. Every local commit is then in the PR, so its checks cover this
# run's work. A local HEAD ahead of, behind a rewrite of, or diverged from the
# PR head is no signal. When the PR head commit is not in the local object
# store, one bounded `git fetch origin refs/heads/<branch>` (an empty
# --refmap, so no remote-tracking ref is updated; no FETCH_HEAD, no prompt,
# 60s) is tried; if the commit is still
# missing, or merge-base fails for any reason, there is no signal.
#
# The repo identity comes only from <target_dir>'s remote.origin.url and is
# passed to every gh call with --repo, so a fork checkout never queries
# `upstream` or a set-default repo. gh is read-only (pr view / pr list with
# fixed --json field lists), runs through _gh_retry_core from
# workflow_gh_retry.sh with one attempt, no rate-limit wait and no REST
# fallback, and its stderr is never emitted. The only other network call is
# the bounded git fetch above. Retry scratch files live in a
# private mktemp -d directory that is removed on every return path.
#
# This file only DEFINES functions (and sources the shared parser
# workflow_github_repo.sh); sourcing it has no other side effects.

# parse_github_repo_identity comes from the shared parser next to this file
# (issue #1642). When it is missing the function stays undefined, every call
# fails, and the open-PR signal fails closed; this file is sourced, so it never exits.
_finalization_pr_signal_repo_helper="${WORKFLOW_GITHUB_REPO_HELPER:-$(dirname "${BASH_SOURCE[0]}")/workflow_github_repo.sh}"
if [ -f "$_finalization_pr_signal_repo_helper" ] && [ -r "$_finalization_pr_signal_repo_helper" ]; then
  # shellcheck source=workflow_github_repo.sh disable=SC1090
  . "$_finalization_pr_signal_repo_helper"
else
  echo "WARNING: shared GitHub repo identity parser not found at $_finalization_pr_signal_repo_helper" >&2
fi
unset _finalization_pr_signal_repo_helper

# _finalization_pr_number_valid <n> -> 0 when <n> is a canonical PR number.
_finalization_pr_number_valid() {
  [[ "${1:-}" =~ ^[1-9][0-9]{0,9}$ ]]
}

# _finalization_pr_gh <scratch_dir> <label> <gh-arg>... -> gh stdout.
# One bounded read: no rate-limit wait, no REST fallback, one transient
# attempt. gh stderr (which may carry credentials) is discarded.
_finalization_pr_gh() {
  local scratch="$1" label="$2"
  shift 2
  GH_RETRY_TMPDIR="$scratch" GH_RETRY_MAX_RL_WINDOWS=0 GH_RETRY_REST_FALLBACK="" GH_RETRY_MAX_TRANSIENT=1 \
    _gh_retry_core "$label" "$@" 2>/dev/null
}

# _finalization_pr_checks <branch> <number> <url> <head_sha> < view.json
#   -> checks class.
# Fails (no output) unless the PR is OPEN, on <branch>, same-repo (literal
# false), the requested number, at exactly the canonical URL, and its head
# commit is exactly <head_sha>: the local HEAD, or a PR head the caller has
# already proved to be a fast-forward of it. Checks on an older pushed commit
# therefore never vouch for unpushed local work.
_finalization_pr_checks() {
  jq -er --arg branch "$1" --arg num "$2" --arg url "$3" --arg head "$4" '
    def failing_conclusions: ["FAILURE","CANCELLED","TIMED_OUT","ACTION_REQUIRED","STARTUP_FAILURE","STALE"];
    def pending_states: ["QUEUED","IN_PROGRESS","WAITING","PENDING","REQUESTED"];
    def cls:
      if type != "object" then "unknown"
      elif .__typename == "CheckRun" then
        if .status == "COMPLETED" then
          (.conclusion as $c
           | if (["SUCCESS","NEUTRAL","SKIPPED"] | index([$c])) != null then "green"
             elif (failing_conclusions | index([$c])) != null then "failing"
             else "unknown" end)
        elif (.status as $s | pending_states | index([$s])) != null then "pending"
        else "unknown" end
      elif .__typename == "StatusContext" then
        (.state as $s
         | if $s == "SUCCESS" then "green"
           elif $s == "FAILURE" or $s == "ERROR" then "failing"
           elif $s == "PENDING" or $s == "EXPECTED" then "pending"
           else "unknown" end)
      else "unknown" end;
    if type == "object"
       and .state == "OPEN"
       and .headRefName == $branch
       and .isCrossRepository == false
       and (.number | type) == "number"
       and (.number | tostring) == $num
       and .url == $url
       and (.headRefOid | type) == "string"
       and .headRefOid == $head
    then
      (.statusCheckRollup // []) as $rollup
      | if ($rollup | type) != "array" then "unknown"
        elif ($rollup | length) == 0 then "none"
        else [$rollup[] | cls] as $classes
          | if any($classes[]; . == "failing") then "failing"
            elif any($classes[]; . == "unknown") then "unknown"
            elif any($classes[]; . == "pending") then "pending"
            else "green" end
        end
    else empty end' 2>/dev/null
}

# _finalization_pr_head_descends <dir> <branch> <local_head> <pr_head>
#   -> 0 only when <pr_head> is a full commit id, present locally (after at
#   most one fetch), and <local_head> is its ancestor. Anything else is 1.
_finalization_pr_head_descends() {
  local dir="$1" branch="$2" local_head="$3" pr_head="$4" bound=""
  [[ "$pr_head" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || return 1
  if ! git -C "$dir" cat-file -e "${pr_head}^{commit}" 2>/dev/null; then
    ! command -v timeout >/dev/null 2>&1 || bound="timeout 60"
    GIT_TERMINAL_PROMPT=0 $bound git -C "$dir" fetch --quiet --no-tags --no-write-fetch-head --refmap= \
      origin "refs/heads/$branch" </dev/null >/dev/null 2>&1 || :
    git -C "$dir" cat-file -e "${pr_head}^{commit}" 2>/dev/null || return 1
  fi
  git -C "$dir" merge-base --is-ancestor "$local_head" "$pr_head" 2>/dev/null
}

# _finalization_pr_list_number < list.json -> the single same-repo PR number.
# Exactly one PR in total is required; zero or two (even one same-repo plus
# one fork PR) is ambiguous and fails.
_finalization_pr_list_number() {
  jq -er '
    if type == "array" and length == 1 and (.[0] | type) == "object"
       and .[0].isCrossRepository == false and (.[0].number | type) == "number"
    then .[0].number | tostring
    else empty end' 2>/dev/null
}

# _finalization_no_signal <reason> -> prints a fixed reason on stderr, returns 1.
# Reasons are literal strings from this file; no URL, branch or gh output is
# ever interpolated, so nothing credential-bearing can be echoed.
_finalization_no_signal() {
  printf 'WARNING: no open-PR signal: %s\n' "$1" >&2
  return 1
}

_finalization_open_pr_signal_in() {
  local scratch="$1" dir="${2:-}" branch="${3:-}" ref="${4:-}"
  local live="" origin="" repo="" num="" prefix="" url="" list="" view="" checks="" head="" pr_head="" relation="local"

  { [ -n "$dir" ] && [ -d "$dir" ]; } || { _finalization_no_signal "target directory missing"; return 1; }
  # rev-parse exits 0 but prints "false" inside a .git dir or bare repo, where
  # the dirty-worktree probe cannot see uncommitted files: require "true".
  [ "$(git -C "$dir" rev-parse --is-inside-work-tree 2>/dev/null || :)" = "true" ] \
    || { _finalization_no_signal "target directory is not a git worktree"; return 1; }

  case "$branch" in ''|main|master|HEAD|-*) _finalization_no_signal "run branch is empty or a default branch"; return 1 ;; esac
  git check-ref-format "refs/heads/$branch" >/dev/null 2>&1 \
    || { _finalization_no_signal "run branch is not a valid branch name"; return 1; }
  live="$(git -C "$dir" branch --show-current 2>/dev/null || :)"
  if [ -n "$live" ] && [ "$live" != "$branch" ]; then
    _finalization_no_signal "run branch does not match the checked-out branch"
    return 1
  fi
  # Fail closed when HEAD cannot be resolved to a full commit id.
  head="$(git -C "$dir" rev-parse --verify --quiet 'HEAD^{commit}' 2>/dev/null || :)"
  [[ "$head" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || { _finalization_no_signal "local HEAD could not be resolved"; return 1; }

  origin="$(git -C "$dir" config --get remote.origin.url 2>/dev/null || :)"
  declare -F parse_github_repo_identity >/dev/null 2>&1 || { _finalization_no_signal "GitHub repo identity parser is not loaded"; return 1; }
  repo="$(parse_github_repo_identity "$origin")" || { _finalization_no_signal "origin is not a recognised github.com repository"; return 1; }
  prefix="https://github.com/$repo/pull/"

  if [ -n "$ref" ]; then
    if _finalization_pr_number_valid "$ref"; then
      num="$ref"
    else
      # Only the exact canonical URL for the origin repo is accepted: no
      # userinfo, query, fragment or trailing path, and no pr list fallback.
      case "$ref" in
        "$prefix"*) num="${ref#"$prefix"}" ;;
        *) _finalization_no_signal "PR reference is not the canonical URL for the origin repository"; return 1 ;;
      esac
      _finalization_pr_number_valid "$num" || { _finalization_no_signal "PR reference has an invalid PR number"; return 1; }
    fi
  else
    list="$(_finalization_pr_gh "$scratch" "pr list" pr list --repo "$repo" --head "$branch" --state open --json number,url,isCrossRepository --limit 2)" \
      || { _finalization_no_signal "gh pr list failed"; return 1; }
    num="$(printf '%s' "$list" | _finalization_pr_list_number)" \
      || { _finalization_no_signal "gh pr list did not return exactly one same-repo open PR"; return 1; }
    _finalization_pr_number_valid "$num" || { _finalization_no_signal "gh pr list returned an invalid PR number"; return 1; }
  fi

  url="$prefix$num"
  view="$(_finalization_pr_gh "$scratch" "pr view" pr view "$num" --repo "$repo" --json number,url,state,headRefName,headRefOid,isCrossRepository,statusCheckRollup)" \
    || { _finalization_no_signal "gh pr view failed"; return 1; }
  # A PR head that is a fast-forward of the local HEAD stands in for it
  # (#1714); every other condition is still checked against the view.
  pr_head="$(printf '%s' "$view" | jq -r 'if type == "object" and (.headRefOid | type) == "string" then .headRefOid else empty end' 2>/dev/null || :)"
  if [ -n "$pr_head" ] && [ "$pr_head" != "$head" ] && _finalization_pr_head_descends "$dir" "$branch" "$head" "$pr_head"; then
    head="$pr_head"; relation="descendant"
  fi
  checks="$(printf '%s' "$view" | _finalization_pr_checks "$branch" "$num" "$url" "$head")" \
    || { _finalization_no_signal "PR is not open, same-repo, on the run branch, at the canonical URL, with its head at the local HEAD or a fast-forward of it"; return 1; }

  jq -cn --arg pr_number "$num" --arg pr_url "$url" --arg checks "$checks" --arg head "$relation" \
    '{pr_number: $pr_number, pr_url: $pr_url, checks: $checks, head: $head}'
}

finalization_open_pr_signal() {
  local scratch="" status=0
  command -v jq >/dev/null 2>&1 || { _finalization_no_signal "jq is unavailable"; return 1; }
  declare -F _gh_retry_core >/dev/null 2>&1 || { _finalization_no_signal "gh retry helper is not loaded"; return 1; }
  # Never fall back to the working directory for retry scratch files: they
  # would dirty the worktree for this and later runs.
  scratch="$(mktemp -d 2>/dev/null)" || { _finalization_no_signal "could not create a private scratch directory"; return 1; }
  { [ -n "$scratch" ] && [ -d "$scratch" ]; } || { _finalization_no_signal "could not create a private scratch directory"; return 1; }
  _finalization_open_pr_signal_in "$scratch" "$@" || status=$?
  rm -rf -- "$scratch"
  return "$status"
}
