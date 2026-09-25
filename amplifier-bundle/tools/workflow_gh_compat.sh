#!/usr/bin/env bash
# workflow_gh_compat.sh — `gh` compatibility layer for hosts that block GitHub
# GraphQL (issue #1484).
#
# WHY. Claude Code on the web (cloud) sessions reach GitHub through a proxy that
# serves the REST API but refuses GraphQL with
#
#     HTTP 403: GitHub GraphQL is not available from Claude Code sessions; ...
#
# `gh issue`, `gh pr`, `gh label` and `gh api graphql` are GraphQL clients, and
# ~34 recipe/tool files call them. Rather than edit every call site, the recipe
# runner puts a `gh` launcher first on PATH that execs this script (see
# crates/amplihack-cli/src/commands/recipe/run/gh_compat.rs). Every step, every
# helper and every agent the run spawns then reaches GitHub through here.
#
# BEHAVIOUR.
#   * Anything other than issue/pr/label/`api graphql`/`auth status` is handed
#     to the real gh untouched (`exec`).
#   * For those subcommands the real gh runs first. Only when it fails with the
#     GraphQL-block text is the call replayed over REST (`gh api repos/...`),
#     and the block is remembered for the rest of the run so later calls go
#     straight to REST. Where GraphQL works (terminals, CI) nothing changes.
#   * Output keeps the shapes callers parse: the `--json` field names, `--jq`,
#     a bare URL from `create`, and gh's exit codes (`pr checks`: 1 fail, 8
#     pending).
#   * Where REST has no equivalent the call degrades: it fails the way gh would
#     (non-zero, message on stderr) instead of faking success, and the gap is
#     logged. Nothing in the default workflow depends on those gaps.
#
# Knobs: AMPLIHACK_GH_REST_ONLY=1 forces REST; AMPLIHACK_REAL_GH names the real
# binary; AMPLIHACK_GH_COMPAT_STATE / AMPLIHACK_GH_COMPAT_LOG move the state and
# log files; AMPLIHACK_GH_COMPAT_STATE_TTL_MIN (default 60) is how long a
# recorded block is trusted before the real gh is probed again;
# AMPLIHACK_GH_COMPAT_VERBOSE=1 also logs to stderr. The log stays off stderr by
# default because callers capture `2>&1` (step-03 does, for the URL).
#
# LIMITS. `pr list` / `issue list` page through REST 100 at a time up to
# --limit, reading at most ~10 extra pages past it when a filter (merged, PRs
# mixed into /issues, the search fallback) drops items. Per-PR sub-lists
# (reviews, files, commits, comments, check runs) read the first 100 only.
# `closedByPullRequestsReferences` has no REST source and is always [] (logged);
# its one caller, workflow-design step-06d, fails closed on an empty list.
#
# bash 3.2 compatible (issue #1423): no associative arrays, no case folding
# expansions, no mapfile; no `set -u`, so empty arrays are safe to expand.

set -o pipefail

GHC_BLOCK_RE='GraphQL is not available|GraphQL (API )?(is )?(disabled|blocked)'
GHC_TMP="${TMPDIR:-/tmp}"
# The "GraphQL is blocked" marker. Under the recipe runner TMPDIR is per run, so
# it lives and dies with the run. Elsewhere TMPDIR is shared, so the default is
# a private per-user directory, and a marker older than the TTL is ignored: the
# next call probes the real gh again (and re-records the block if it persists).
GHC_STATE="${AMPLIHACK_GH_COMPAT_STATE:-}"
GHC_STATE_TTL_MIN="${AMPLIHACK_GH_COMPAT_STATE_TTL_MIN:-60}"
case "$GHC_STATE_TTL_MIN" in ''|*[!0-9]*) GHC_STATE_TTL_MIN=60 ;; esac
ghc_init_state() {
  local d
  [ -z "$GHC_STATE" ] || return 0
  d="${GHC_TMP}/amplihack-gh-compat-$(id -u 2>/dev/null || echo 0)"
  [ -d "$d" ] || mkdir -m 700 "$d" 2>/dev/null || true
  # Someone else's (or a symlinked) directory is never trusted: no state.
  if [ -d "$d" ] && [ ! -L "$d" ] && [ -O "$d" ]; then GHC_STATE="$d/graphql-blocked"; fi
}
GHC_LOG="${AMPLIHACK_GH_COMPAT_LOG:-${AMPLIHACK_WORKFLOW_ARTIFACT_DIR:-$GHC_TMP}/gh-compat.log}"

ghc_log() {
  printf '%s gh-compat[%s]: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$$" "$*" >>"$GHC_LOG" 2>/dev/null || true
  if [ "${AMPLIHACK_GH_COMPAT_VERBOSE:-0}" = 1 ]; then printf 'gh-compat: %s\n' "$*" >&2; fi
  return 0
}

ghc_die() { printf '%s\n' "$1" >&2; ghc_log "error: $1"; exit "${2:-1}"; }

# First `gh` on PATH that is not a compat launcher (a dir carrying the marker).
ghc_find_real_gh() {
  local d
  if [ -n "${AMPLIHACK_REAL_GH:-}" ] && [ -x "$AMPLIHACK_REAL_GH" ]; then
    printf '%s\n' "$AMPLIHACK_REAL_GH"; return 0
  fi
  while IFS= read -r -d ':' d; do
    [ -n "$d" ] || continue
    [ -e "$d/.amplihack-gh-compat" ] && continue
    if [ -f "$d/gh" ] && [ -x "$d/gh" ]; then printf '%s\n' "$d/gh"; return 0; fi
  done < <(printf '%s:' "$PATH")
  return 1
}

ghc_rest_mode() {
  [ "${AMPLIHACK_GH_REST_ONLY:-0}" = 1 ] && return 0
  [ -n "$GHC_STATE" ] && [ -f "$GHC_STATE" ] || return 1
  # Fresh markers only (find -mmin is GNU and BSD); a stale one is re-probed.
  [ -n "$(find "$GHC_STATE" -mmin "-${GHC_STATE_TTL_MIN}" 2>/dev/null)" ]
}

# ---------------------------------------------------------------------------
# REST transport. Always the real `gh api`: it already owns auth, proxy and CA
# handling. Sets GHC_STATUS (HTTP code) and GHC_ERR; stdout is the body.
# ghc_api METHOD PATH [JSON_BODY] [ACCEPT]
# ---------------------------------------------------------------------------
GHC_STATUS=""; GHC_ERR=""; GHC_RUN_DIR="${GHC_RUN_DIR:-$GHC_TMP}"  # ghc_main makes a private one
ghc_api() {
  local method="$1" path="$2" body="${3:-}" accept="${4:-}" errf bodyf="" rc=0
  local args=(api -X "$method" "$path")
  errf="$(mktemp "${GHC_RUN_DIR}/err.XXXXXX")" || return 1
  [ -n "$accept" ] && args+=(-H "Accept: $accept")
  if [ -n "$body" ]; then
    bodyf="$(mktemp "${GHC_RUN_DIR}/body.XXXXXX")" || { rm -f "$errf"; return 1; }
    printf '%s' "$body" >"$bodyf"
    args+=(--input "$bodyf")
  fi
  "$GHC_REAL" "${args[@]}" 2>"$errf" || rc=$?
  GHC_ERR="$(cat "$errf")"; GHC_STATUS=""
  [[ "$GHC_ERR" =~ \(HTTP\ ([0-9][0-9][0-9])\) ]] && GHC_STATUS="${BASH_REMATCH[1]}"
  [ "$rc" -eq 0 ] && [ -z "$GHC_STATUS" ] && GHC_STATUS=200
  rm -f "$errf"; [ -n "$bodyf" ] && rm -f "$bodyf"
  ghc_log "REST $method $path -> ${GHC_STATUS:-rc=$rc}"
  # Callers usually run this inside $(...), where these globals die with the
  # subshell; persist them for ghc_last in this invocation's private dir.
  printf '%s' "$GHC_STATUS" >"${GHC_RUN_DIR}/last.status" 2>/dev/null
  printf '%s' "$GHC_ERR" >"${GHC_RUN_DIR}/last.err" 2>/dev/null
  return "$rc"
}

# ghc_last — reload GHC_STATUS / GHC_ERR from the most recent ghc_api call.
ghc_last() {
  GHC_STATUS="$(cat "${GHC_RUN_DIR}/last.status" 2>/dev/null)"
  GHC_ERR="$(cat "${GHC_RUN_DIR}/last.err" 2>/dev/null)"
}

# ghc_api_or_die METHOD PATH [BODY] [ACCEPT] — body on stdout, gh-style failure.
ghc_api_or_die() {
  local out
  out="$(ghc_api "$@")" || { ghc_last; GHC_ERR="${GHC_ERR#gh: }"; ghc_die "gh: ${GHC_ERR:-REST $1 $2 failed}"; }
  printf '%s\n' "$out"
}

# ghc_paged PATH LIMIT FILTER [jq args...] — GET PATH (which already has a
# query string) page by page, keeping the items the jq FILTER (array -> array)
# passes, until LIMIT are kept, a short page ends the list, or ~10 pages past
# what LIMIT needs have been read. Prints the first LIMIT kept items.
ghc_paged() {
  local path="$1" limit="$2" filter="$3" per page=1 max raw n acc
  shift 3
  per="${GHC_PAGE_SIZE:-100}"; [ "$limit" -lt "$per" ] 2>/dev/null && [ -z "${GHC_PAGE_SIZE:-}" ] && per="$limit"
  [ "$per" -ge 1 ] 2>/dev/null || per=30
  max=$(( (limit + per - 1) / per + 10 ))
  acc="${GHC_RUN_DIR}/paged.$$.$RANDOM"; : >"$acc" || return 1
  while :; do
    raw="$(ghc_api_or_die GET "${path}&per_page=${per}&page=${page}")" || { rm -f "$acc"; return 1; }
    n="$(printf '%s' "$raw" | jq 'length')" || { rm -f "$acc"; return 1; }
    printf '%s' "$raw" | jq -c "$@" "$filter" >>"$acc" || { rm -f "$acc"; return 1; }
    [ "$(jq -s 'add | length' "$acc")" -ge "$limit" ] && break
    [ "$n" -lt "$per" ] && break
    [ "$page" -ge "$max" ] && { ghc_log "list truncated after ${page} pages of ${path}"; break; }
    page=$((page + 1))
  done
  jq -s --argjson n "$limit" 'add // [] | .[:$n]' "$acc"; rm -f "$acc"
}

# ---------------------------------------------------------------------------
# Option parsing. ghc_parse "<value spec>" "<bool spec>" ARGS...
# A spec is " -R:repo --repo:repo ..." (flag:name). Value flags land in
# GHC_O_<name> (repeatable ones joined with ","), booleans in GHC_B_<name>=1,
# positionals in GHC_POS. The value may be attached, as pflag accepts it:
# `--repo=o/r`, `--body=` (explicitly empty), `-Ro/r`, `-L50`. Only a flag with
# no attached value takes the next argument.
# ---------------------------------------------------------------------------
GHC_REPEAT=" label assignee reviewer add_label remove_label field "
ghc_parse() {
  local vspec=" $1 " bspec=" $2 " a val has name cur vn
  shift 2
  GHC_POS=()
  while [ $# -gt 0 ]; do
    a="$1"; val=""; has=0
    case "$a" in
      --*=*) val="${a#*=}"; a="${a%%=*}"; has=1 ;;
      -[!-]?*) case "$vspec" in *" ${a:0:2}:"*) val="${a:2}"; val="${val#=}"; a="${a:0:2}"; has=1 ;; esac ;;
    esac
    case "$vspec" in
      *" $a:"*)
        name="${vspec#* "$a":}"; name="${name%% *}"
        if [ "$has" = 0 ]; then val="${2-}"; shift; fi
        vn="GHC_O_${name}"; cur="${!vn:-}"
        case "$GHC_REPEAT" in
          *" $name "*) [ -n "$cur" ] && val="$cur,$val" ;;
        esac
        printf -v "GHC_O_${name}" '%s' "$val"
        shift; continue ;;
    esac
    case "$bspec" in
      *" $a:"*) name="${bspec#* "$a":}"; name="${name%% *}"
        if [ "$has" = 1 ] && [ "$val" = false ]; then val=""; else val=1; fi   # --draft=false
        printf -v "GHC_B_${name}" '%s' "$val"; shift; continue ;;
    esac
    case "$a" in
      --) shift; GHC_POS+=("$@"); break ;;
      -?*) ghc_log "ignoring unsupported flag $a"; shift ;;
      *) GHC_POS+=("$a"); shift ;;
    esac
  done
}

GHC_COMMON_V="-R:repo --repo:repo --json:json -q:jq --jq:jq -t:template --template:template"

# Body text from --body, or --body-file (path, or - for stdin).
ghc_body() {
  if [ -n "${GHC_O_body_file:-}" ]; then
    if [ "$GHC_O_body_file" = "-" ]; then cat; else cat -- "$GHC_O_body_file"; fi
  else
    printf '%s' "${GHC_O_body:-}"
  fi
}

# ---------------------------------------------------------------------------
# Repository + target resolution.
# ---------------------------------------------------------------------------
ghc_repo_from_url() {
  local u="$1"
  u="${u%/}"; u="${u%.git}"
  if [[ "$u" =~ github\.com[:/]+([^/]+)/([^/]+)$ ]]; then
    printf '%s/%s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}"; return 0
  fi
  # Proxied remotes (e.g. http://proxy@127.0.0.1:port/git/OWNER/REPO).
  if [[ "$u" =~ ^[a-z]+://.*/([^/]+)/([^/]+)$ ]]; then
    printf '%s/%s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}"; return 0
  fi
  return 1
}

GHC_REPO=""
ghc_resolve_repo() {
  local r="${GHC_O_repo:-${GH_REPO:-}}"
  if [ -n "$r" ]; then
    case "$r" in *github.com*) r="$(ghc_repo_from_url "$r")" || r="" ;; esac
    r="${r#github.com/}"
  else
    r="$(ghc_repo_from_url "$(git remote get-url origin 2>/dev/null)")" || r=""
  fi
  [ -n "$r" ] || ghc_die "gh: could not determine the repository (no --repo, GH_REPO, or github.com origin remote)"
  GHC_REPO="$r"
}

ghc_current_branch() { git branch --show-current 2>/dev/null; }

# ghc_uri TEXT — percent-encode one query/path component (branch names may hold
# '&', '#', '+' ...).
ghc_uri() { jq -rn --arg v "$1" '$v|@uri'; }

# ghc_pr_target TARGET -> sets GHC_N (TARGET: "", N, #N, URL or branch). Runs
# in the caller's shell, not a $(...), because a URL target also sets the repo.
GHC_N=""
ghc_pr_target() {
  local t="${1:-}" owner pulls
  t="${t#\#}"; GHC_N=""
  case "$t" in
    '') t="$(ghc_current_branch)"; [ -n "$t" ] || ghc_die "gh: could not determine the current branch" ;;
    *[!0-9]*) ;;
    *) GHC_N="$t"; return 0 ;;
  esac
  if [[ "$t" =~ /pull/([0-9]+) ]]; then
    GHC_N="${BASH_REMATCH[1]}"
    GHC_REPO="$(ghc_repo_from_url "${t%%/pull/*}")" || ghc_die "gh: cannot parse pull request URL: $t"
    return 0
  fi
  owner="${GHC_REPO%%/*}"
  case "$t" in *:*) ;; *) t="${owner}:${t}" ;; esac   # OWNER:BRANCH names a fork's branch
  pulls="$(ghc_api_or_die GET "repos/${GHC_REPO}/pulls?head=$(ghc_uri "$t")&state=all&per_page=30")" || exit 1
  GHC_N="$(printf '%s' "$pulls" | jq -r 'sort_by(if .state == "open" then 0 else 1 end) | .[0].number // empty')"
  [ -n "$GHC_N" ] || ghc_die "no pull requests found for branch \"${t#"${owner}":}\""
}

# ---------------------------------------------------------------------------
# REST -> gh JSON shapes.
# ---------------------------------------------------------------------------
# shellcheck disable=SC2016  # jq program, not shell expansions.
GHC_JQ_DEFS='
def up: (. // "") | ascii_upcase;
def closing: [ (.body // "") | scan("(?i)\\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\\s*:?\\s+#([0-9]+)") | {number: (.[0] | tonumber)} ] | unique;
def pr: {
  number, id: .node_id, title: (.title // ""), body: (.body // ""),
  state: (if (.merged_at // null) != null then "MERGED" elif .state == "open" then "OPEN" else "CLOSED" end),
  isDraft: (.draft // false), createdAt: .created_at, updatedAt: .updated_at,
  closedAt: .closed_at, mergedAt: .merged_at, url: .html_url,
  headRefName: .head.ref, baseRefName: .base.ref, headRefOid: .head.sha, baseRefOid: .base.sha,
  headRepositoryOwner: {login: (.head.repo.owner.login // "")},
  headRepository: {name: (.head.repo.name // ""), nameWithOwner: (.head.repo.full_name // "")},
  isCrossRepository: ((.head.repo.full_name // "") != (.base.repo.full_name // "")),
  author: {login: (.user.login // "")}, labels: [.labels[]? | {name, color, description}],
  assignees: [.assignees[]? | {login}],
  mergeable: (if .mergeable == true then "MERGEABLE" elif .mergeable == false then "CONFLICTING" else "UNKNOWN" end),
  mergeStateStatus: (.mergeable_state // "unknown" | ascii_upcase),
  additions, deletions, changedFiles: .changed_files,
  mergeCommit: (if .merged_at then {oid: .merge_commit_sha} else null end),
  closingIssuesReferences: closing, reviewDecision: ""
};
def issue: {
  number, id: .node_id, title: (.title // ""), body: (.body // ""), state: (.state | up),
  stateReason: (.state_reason | up), url: .html_url, author: {login: (.user.login // "")},
  labels: [.labels[]? | {name, color, description}], assignees: [.assignees[]? | {login}],
  createdAt: .created_at, updatedAt: .updated_at, closedAt: .closed_at,
  milestone: (if .milestone then {title: .milestone.title} else null end),
  closedByPullRequestsReferences: []
};
def bucket: if .status != null and .status != "completed" then "pending"
  elif .conclusion == "success" then "pass"
  elif (.conclusion == "skipped" or .conclusion == "neutral") then "skipping"
  elif .conclusion == "cancelled" then "cancel"
  else "fail" end;
'

# ghc_emit JSON — apply --json field selection and --jq like gh does.
ghc_emit() {
  local json="$1"
  # Go templates have no jq translation; JSON in place of the asked-for text
  # would be a success that is not one.
  if [ -n "${GHC_O_template:-}" ]; then ghc_die "gh-compat: --template is not supported without GitHub GraphQL; use --json/--jq"; fi
  if [ -n "${GHC_O_json:-}" ]; then
    json="$(printf '%s' "$json" | jq --arg f "$GHC_O_json" '
      def pick: . as $o | reduce ($f | split(",")[] | select(. != "")) as $k ({}; .[$k] = $o[$k]);
      if type == "array" then map(pick) else pick end')" || return 1
  fi
  if [ -n "${GHC_O_jq:-}" ]; then
    printf '%s' "$json" | jq -r "$GHC_O_jq"
  else
    printf '%s\n' "$json" | jq .
  fi
}

ghc_wants() { case ",${GHC_O_json:-}," in *",$1,"*) return 0 ;; esac; return 1; }

# Check runs + commit statuses for SHA, as gh's statusCheckRollup.
ghc_rollup() {
  local sha="$1" runs statuses
  runs="$(ghc_api GET "repos/${GHC_REPO}/commits/${sha}/check-runs?filter=latest&per_page=100")" || runs='{}'
  statuses="$(ghc_api GET "repos/${GHC_REPO}/commits/${sha}/status?per_page=100")" || statuses='{}'
  jq -n --argjson r "$runs" --argjson s "$statuses" '
    [($r.check_runs // [])[] | {__typename: "CheckRun", name, status: (.status // "" | ascii_upcase),
      conclusion: (.conclusion // "" | ascii_upcase), detailsUrl: .details_url,
      startedAt: .started_at, completedAt: .completed_at, workflowName: (.app.name // "")}]
    + [($s.statuses // [])[] | {__typename: "StatusContext", context, state: (.state // "" | ascii_upcase),
      targetUrl: .target_url, description}]'
}

# ghc_pr_full NUMBER — mapped PR, enriched with whatever --json asked for.
ghc_pr_full() {
  local n="$1" obj extra
  obj="$(ghc_api_or_die GET "repos/${GHC_REPO}/pulls/${n}" | jq "${GHC_JQ_DEFS} pr")" || exit 1
  if ghc_wants reviews || ghc_wants latestReviews || ghc_wants reviewDecision; then
    # A merge gate reads reviewDecision; an unreadable review list must fail the
    # call, not report "no reviews". REST has no REVIEW_REQUIRED (that needs
    # branch protection); mergeStateStatus BLOCKED still carries it.
    extra="$(ghc_api_or_die GET "repos/${GHC_REPO}/pulls/${n}/reviews?per_page=100")" || exit 1
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '
      ($r | map({author: {login: (.user.login // "")}, state, body, submittedAt: .submitted_at, id: .node_id})) as $m
      | ([$r[] | select(.state == "APPROVED" or .state == "CHANGES_REQUESTED" or .state == "DISMISSED")]
         | group_by(.user.login // "") | map(max_by(.submitted_at // "") | .state)) as $last
      | $o + {reviews: $m, latestReviews: $m,
              reviewDecision: (if any($last[]; . == "CHANGES_REQUESTED") then "CHANGES_REQUESTED"
                               elif any($last[]; . == "APPROVED") then "APPROVED" else "" end)}')" || exit 1
  fi
  if ghc_wants statusCheckRollup; then
    extra="$(ghc_rollup "$(printf '%s' "$obj" | jq -r .headRefOid)")"
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {statusCheckRollup: $r}')"
  fi
  if ghc_wants files; then
    extra="$(ghc_api GET "repos/${GHC_REPO}/pulls/${n}/files?per_page=100")" || extra='[]'
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {files: ($r | map({path: .filename, additions, deletions}))}')"
  fi
  if ghc_wants commits; then
    extra="$(ghc_api GET "repos/${GHC_REPO}/pulls/${n}/commits?per_page=100")" || extra='[]'
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {commits: ($r | map({oid: .sha, messageHeadline: (.commit.message | split("\n")[0])}))}')"
  fi
  if ghc_wants comments; then
    extra="$(ghc_api GET "repos/${GHC_REPO}/issues/${n}/comments?per_page=100")" || extra='[]'
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {comments: ($r | map({author: {login: (.user.login // "")}, body, createdAt: .created_at, url: .html_url}))}')"
  fi
  printf '%s\n' "$obj"
}

# ghc_search KIND(pr|issue) STATE TEXT LIMIT — raw REST issue objects, like
# /search/issues. The cloud proxy refuses /search (it only serves repository-
# scoped paths), so on failure the repo's issues are listed and the free-text
# terms matched client-side against title and body.
ghc_search() {
  local kind="$1" state="$2" text="$3" limit="$4" q raw rstate author="${GHC_O_author:-}"
  q="repo:${GHC_REPO} is:${kind} ${text}"
  case "$state" in open|closed|merged) q="$q is:$state" ;; esac
  [ -n "${GHC_O_author:-}" ] && q="$q author:${GHC_O_author}"
  [ -n "${GHC_O_label:-}" ] && q="$q label:\"${GHC_O_label}\""
  # /search pages hold at most 100; a larger --limit is cut there.
  if raw="$(ghc_api GET "search/issues?per_page=$([ "$limit" -gt 100 ] && echo 100 || echo "$limit")&q=$(jq -rn --arg q "$q" '$q|@uri')")"; then
    printf '%s' "$raw" | jq '.items // []'; return 0
  fi
  ghc_last
  ghc_log "search unavailable (${GHC_STATUS:-?}); matching '${text}' client-side over repos/${GHC_REPO}/issues"
  # /search resolves @me server-side; the client-side match compares logins,
  # where a literal "@me" would silently match nobody (quality-loop's
  # `--author=@me` lists would come back empty).
  [ "$author" = "@me" ] && { author="$(ghc_api_or_die GET user | jq -r '.login // empty')" || exit 1; }
  [ -n "${GHC_O_author:-}" ] && [ -z "$author" ] && ghc_die "gh: could not resolve --author ${GHC_O_author}"
  rstate="$state"; case "$state" in open|closed) ;; *) rstate=all ;; esac
  # Full pages: the text match keeps few items, and --limit 1 must not stop
  # the scan after ~11 issues.
  GHC_PAGE_SIZE=100 ghc_paged "repos/${GHC_REPO}/issues?state=${rstate}$([ -n "${GHC_O_label:-}" ] && printf '&labels=%s' "$(ghc_uri "$GHC_O_label")")" "$limit" '
    # Whole words, as /search matches them. A substring test lets "a" or "it"
    # match any body, and step-03 would adopt an unrelated issue as its tracker.
    def words: ascii_downcase | [scan("[a-z0-9]+")];
    ($t | split(" ") | map(select(contains(":") | not)) | join(" ") | words) as $words
    | map(select((.pull_request != null) == ($k == "pr"))
          | select($a == "" or .user.login == $a)
          | select(((.title // "") + " " + (.body // "") | words) as $h | all($words[]; . as $w | $h | index([$w]) != null)))' \
    --arg k "$kind" --arg t "$text" --arg a "$author"
}

# ---------------------------------------------------------------------------
# gh pr ...
# ---------------------------------------------------------------------------
ghc_pr_view() {
  local n obj
  ghc_parse "$GHC_COMMON_V" "-c:comments --comments:comments -w:web --web:web" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  obj="$(ghc_pr_full "$n")" || exit 1
  if [ -z "${GHC_O_json:-}" ]; then
    printf '%s' "$obj" | jq -r '"title:\t\(.title)\nstate:\t\(.state)\nauthor:\t\(.author.login)\nnumber:\t\(.number)\nurl:\t\(.url)\n--\n\(.body)"'
    return 0
  fi
  ghc_emit "$obj"
}

ghc_pr_list() {
  local state limit q raw owner nums n out="[]" obj
  ghc_parse "$GHC_COMMON_V -s:state --state:state -L:limit --limit:limit -H:head --head:head -B:base --base:base -S:search --search:search -A:author --author:author -l:label --label:label" "-d:draft --draft:draft -w:web --web:web" "$@"
  ghc_resolve_repo
  state="${GHC_O_state:-open}"; limit="${GHC_O_limit:-30}"; owner="${GHC_REPO%%/*}"
  case "$limit" in ''|*[!0-9]*|0) ghc_die "invalid value for --limit: ${limit}" ;; esac
  if [ -n "${GHC_O_search:-}${GHC_O_author:-}${GHC_O_label:-}" ]; then
    raw="$(ghc_search pr "$state" "${GHC_O_search:-}" "$limit")" || exit 1
    nums="$(printf '%s' "$raw" | jq -r '.[].number')"
    for n in $nums; do
      obj="$(ghc_pr_full "$n")" || exit 1
      out="$(jq -n --argjson a "$out" --argjson o "$obj" '$a + [$o]')"
    done
  else
    local rstate="$state" qs=""
    [ "$state" = merged ] && rstate=closed
    # OWNER:BRANCH keeps its (fork) owner; a bare branch is looked up in this repo's owner.
    case "${GHC_O_head:-}" in '') ;; *:*) qs="&head=$(ghc_uri "$GHC_O_head")" ;; *) qs="&head=$(ghc_uri "${owner}:${GHC_O_head}")" ;; esac
    [ -n "${GHC_O_base:-}" ] && qs="${qs}&base=$(ghc_uri "$GHC_O_base")"
    out="$(ghc_paged "repos/${GHC_REPO}/pulls?state=${rstate}${qs}" "$limit" "${GHC_JQ_DEFS}"' map(pr) | if $s == "merged" then map(select(.state == "MERGED")) else . end' --arg s "$state")" || exit 1
    if ghc_wants reviews || ghc_wants statusCheckRollup || ghc_wants mergeable || ghc_wants files || ghc_wants commits || ghc_wants comments; then
      local full="[]"
      for n in $(printf '%s' "$out" | jq -r '.[].number'); do
        obj="$(ghc_pr_full "$n")" || exit 1
        full="$(jq -n --argjson a "$full" --argjson o "$obj" '$a + [$o]')"
      done
      out="$full"
    fi
  fi
  [ "${GHC_B_draft:-}" = 1 ] && out="$(printf '%s' "$out" | jq 'map(select(.isDraft))')"
  if [ -z "${GHC_O_json:-}" ]; then
    printf '%s' "$out" | jq -r '.[] | "\(.number)\t\(.title)\t\(.headRefName)\t\(.state)"'
    return 0
  fi
  ghc_emit "$out"
}

ghc_pr_create() {
  local head base body payload resp url n q
  ghc_parse "-R:repo --repo:repo -t:title --title:title -b:body --body:body -F:body_file --body-file:body_file -B:base --base:base -H:head --head:head -l:label --label:label -a:assignee --assignee:assignee -r:reviewer --reviewer:reviewer -m:milestone --milestone:milestone -p:project --project:project" "-d:draft --draft:draft -f:fill --fill:fill --fill-first:fill -w:web --web:web --dry-run:dry_run" "$@"
  ghc_resolve_repo
  head="${GHC_O_head:-$(ghc_current_branch)}"
  [ -n "$head" ] || ghc_die "gh: could not determine the head branch"
  base="${GHC_O_base:-}"
  [ -n "$base" ] || base="$(ghc_api_or_die GET "repos/${GHC_REPO}" | jq -r .default_branch)" || exit 1
  body="$(ghc_body)"
  if [ -z "${GHC_O_title:-}" ] && [ "${GHC_B_fill:-}" = 1 ]; then
    GHC_O_title="$(git log -1 --format=%s 2>/dev/null)"
  fi
  [ -n "${GHC_O_title:-}" ] || ghc_die "gh: --title is required when GraphQL is unavailable"
  payload="$(jq -n --arg t "$GHC_O_title" --arg b "$body" --arg h "$head" --arg B "$base" --argjson d "$([ "${GHC_B_draft:-}" = 1 ] && echo true || echo false)" \
    '{title: $t, body: $b, head: $h, base: $B, draft: $d}')"
  if ! resp="$(ghc_api POST "repos/${GHC_REPO}/pulls" "$payload")"; then
    ghc_last
    if [ "$GHC_STATUS" = 422 ] && printf '%s' "$resp$GHC_ERR" | grep -q 'already exists'; then
      case "$head" in *:*) q="$head" ;; *) q="${GHC_REPO%%/*}:${head}" ;; esac
      url="$(ghc_api GET "repos/${GHC_REPO}/pulls?head=$(ghc_uri "$q")&base=$(ghc_uri "$base")&state=open" | jq -r '.[0].html_url // empty')"
      ghc_die "a pull request for branch \"$head\" into branch \"$base\" already exists:
$url"
    fi
    ghc_die "pull request create failed: ${GHC_ERR}"
  fi
  url="$(printf '%s' "$resp" | jq -r .html_url)"; n="$(printf '%s' "$resp" | jq -r .number)"
  ghc_add_labels "$n" "${GHC_O_label:-}"
  if [ -n "${GHC_O_assignee:-}" ]; then
    ghc_api POST "repos/${GHC_REPO}/issues/${n}/assignees" "$(ghc_csv_json assignees "$(ghc_expand_me "$GHC_O_assignee")")" >/dev/null || ghc_log "assignees not applied to #$n"
  fi
  if [ -n "${GHC_O_reviewer:-}" ]; then
    ghc_api POST "repos/${GHC_REPO}/pulls/${n}/requested_reviewers" "$(ghc_csv_json reviewers "$GHC_O_reviewer")" >/dev/null || ghc_log "reviewers not requested on #$n"
  fi
  printf '%s\n' "$url"
}

# ghc_csv_json KEY "a,b" -> {"KEY":["a","b"]}
ghc_csv_json() { jq -n --arg k "$1" --arg v "$2" '{($k): ($v | split(",") | map(select(. != "")))}'; }

ghc_expand_me() {
  local v="$1" me
  case ",$v," in
    *,@me,*) me="$(ghc_api GET user | jq -r '.login // empty')"
      [ -n "$me" ] && v="$(printf '%s' "$v" | sed "s/@me/${me}/g")" ;;  # unresolved: GitHub rejects "@me" rather than widening to everyone
  esac
  printf '%s' "$v"
}

# Label writes are best-effort, as `gh pr edit --add-label` callers already assume.
ghc_add_labels() {
  [ -n "${2:-}" ] || return 0
  ghc_api POST "repos/${GHC_REPO}/issues/$1/labels" "$(ghc_csv_json labels "$2")" >/dev/null \
    || { ghc_log "labels '$2' not applied to #$1: $GHC_ERR"; return 1; }
}

ghc_pr_edit() {
  local n patch rc=0
  ghc_parse "-R:repo --repo:repo -t:title --title:title -b:body --body:body -F:body_file --body-file:body_file -B:base --base:base --add-label:add_label --remove-label:remove_label --add-assignee:assignee --add-reviewer:reviewer" "" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  patch="$(jq -n --arg t "${GHC_O_title:-}" --arg B "${GHC_O_base:-}" '{} + (if $t != "" then {title: $t} else {} end) + (if $B != "" then {base: $B} else {} end)')"
  if [ -n "${GHC_O_body:-}${GHC_O_body_file:-}" ]; then
    patch="$(jq -n --argjson p "$patch" --arg b "$(ghc_body)" '$p + {body: $b}')"
  fi
  if [ "$patch" != "{}" ]; then ghc_api_or_die PATCH "repos/${GHC_REPO}/pulls/${n}" "$patch" >/dev/null || exit 1; fi
  ghc_add_labels "$n" "${GHC_O_add_label:-}" || rc=1
  ghc_remove_labels "$n" "${GHC_O_remove_label:-}"
  if [ -n "${GHC_O_assignee:-}" ]; then
    ghc_api POST "repos/${GHC_REPO}/issues/${n}/assignees" "$(ghc_csv_json assignees "$(ghc_expand_me "$GHC_O_assignee")")" >/dev/null || rc=1
  fi
  if [ -n "${GHC_O_reviewer:-}" ]; then
    ghc_api POST "repos/${GHC_REPO}/pulls/${n}/requested_reviewers" "$(ghc_csv_json reviewers "$GHC_O_reviewer")" >/dev/null || rc=1
  fi
  [ "$rc" -eq 0 ] || ghc_die "failed to update pull request #${n}: ${GHC_ERR}"
  printf 'https://github.com/%s/pull/%s\n' "$GHC_REPO" "$n"
}

ghc_remove_labels() {
  local l
  [ -n "${2:-}" ] || return 0
  for l in $(printf '%s' "$2" | tr ',' ' '); do
    ghc_api DELETE "repos/${GHC_REPO}/issues/$1/labels/$(jq -rn --arg l "$l" '$l|@uri')" >/dev/null || ghc_log "label '$l' not removed from #$1"
  done
}

ghc_comment() { # ghc_comment KIND(pr|issue) ARGS...
  local kind="$1" n resp
  shift
  ghc_parse "-R:repo --repo:repo -b:body --body:body -F:body_file --body-file:body_file" "--edit-last:edit_last -w:web --web:web" "$@"
  ghc_resolve_repo
  if [ "$kind" = pr ]; then ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"; else ghc_issue_target "${GHC_POS[0]:-}"; n="$GHC_N"; fi
  resp="$(ghc_api_or_die POST "repos/${GHC_REPO}/issues/${n}/comments" "$(jq -n --arg b "$(ghc_body)" '{body: $b}')")" || exit 1
  printf '%s' "$resp" | jq -r .html_url
}

ghc_pr_ready() {
  local n route=ready_for_review
  ghc_parse "-R:repo --repo:repo" "--undo:undo" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  [ "${GHC_B_undo:-}" = 1 ] && route=convert_to_draft
  # Draft state has no public REST write; the CCR proxy exposes one.
  if ! ghc_api POST "repos/${GHC_REPO}/pulls/${n}/ccr/${route}" >/dev/null; then
    ghc_log "no REST equivalent for pr ready (ccr/${route} -> ${GHC_STATUS:-?})"
    ghc_die "gh-compat: cannot change draft state of #${n} without GraphQL (ccr/${route}: ${GHC_ERR})"
  fi
  if [ "${GHC_B_undo:-}" = 1 ]; then
    printf '✓ Pull request %s#%s is converted to "draft"\n' "$GHC_REPO" "$n" >&2
  else
    printf '✓ Pull request %s#%s is marked as "ready for review"\n' "$GHC_REPO" "$n" >&2
  fi
}

ghc_pr_merge() {
  local n method=merge obj branch
  ghc_parse "-R:repo --repo:repo -t:subject --subject:subject -b:body --body:body --match-head-commit:sha" "-m:merge --merge:merge -s:squash --squash:squash -r:rebase --rebase:rebase --auto:auto -d:delete --delete-branch:delete --admin:admin --disable-auto:disable_auto" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  [ "${GHC_B_squash:-}" = 1 ] && method=squash
  [ "${GHC_B_rebase:-}" = 1 ] && method=rebase
  # gh never picks a merge method for a non-interactive caller.
  if [ "${GHC_B_disable_auto:-}" != 1 ] && [ -z "${GHC_B_merge:-}${GHC_B_squash:-}${GHC_B_rebase:-}" ]; then
    ghc_die "--merge, --rebase, or --squash required when not running interactively"
  fi
  if [ "${GHC_B_disable_auto:-}" = 1 ]; then
    ghc_api_or_die DELETE "repos/${GHC_REPO}/pulls/${n}/ccr/auto_merge" >/dev/null || exit 1; return 0
  fi
  if [ "${GHC_B_auto:-}" = 1 ]; then
    ghc_api_or_die PUT "repos/${GHC_REPO}/pulls/${n}/ccr/auto_merge" "$(jq -n --arg m "$method" '{merge_method: $m}')" >/dev/null || exit 1
    printf '✓ Pull request %s#%s will be automatically merged via %s when all requirements are met\n' "$GHC_REPO" "$n" "$method" >&2
    return 0
  fi
  obj="$(jq -n --arg m "$method" --arg t "${GHC_O_subject:-}" --arg b "${GHC_O_body:-}" --arg s "${GHC_O_sha:-}" \
    '{merge_method: $m} + (if $t != "" then {commit_title: $t} else {} end) + (if $b != "" then {commit_message: $b} else {} end) + (if $s != "" then {sha: $s} else {} end)')"
  ghc_api_or_die PUT "repos/${GHC_REPO}/pulls/${n}/merge" "$obj" >/dev/null || exit 1
  printf '✓ Merged pull request %s#%s\n' "$GHC_REPO" "$n" >&2
  if [ "${GHC_B_delete:-}" = 1 ]; then
    branch="$(ghc_api GET "repos/${GHC_REPO}/pulls/${n}" | jq -r .head.ref)"
    ghc_api DELETE "repos/${GHC_REPO}/git/refs/heads/${branch}" >/dev/null || ghc_log "branch ${branch} not deleted"
  fi
}

ghc_pr_close() {
  local n
  ghc_parse "-R:repo --repo:repo -c:comment --comment:comment" "-d:delete --delete-branch:delete" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  if [ -n "${GHC_O_comment:-}" ]; then
    ghc_api POST "repos/${GHC_REPO}/issues/${n}/comments" "$(jq -n --arg b "$GHC_O_comment" '{body: $b}')" >/dev/null || true
  fi
  ghc_api_or_die PATCH "repos/${GHC_REPO}/pulls/${n}" '{"state":"closed"}' >/dev/null || exit 1
  printf '✓ Closed pull request %s#%s\n' "$GHC_REPO" "$n" >&2
}

ghc_pr_diff() {
  local n
  ghc_parse "-R:repo --repo:repo --color:color" "--name-only:name_only --patch:patch -w:web --web:web" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  if [ "${GHC_B_name_only:-}" = 1 ]; then
    ghc_api_or_die GET "repos/${GHC_REPO}/pulls/${n}/files?per_page=100" | jq -r '.[].filename'
  else
    ghc_api_or_die GET "repos/${GHC_REPO}/pulls/${n}" "" "application/vnd.github.v3.diff"
  fi
}

# Required check names for BASE, from branch protection or rulesets. Empty
# output means "unknown" — the caller then treats every check as required.
ghc_required_checks() {
  local base="$1" out
  if out="$(ghc_api GET "repos/${GHC_REPO}/branches/${base}/protection/required_status_checks")"; then
    printf '%s' "$out" | jq -r '[(.contexts // [])[], ((.checks // [])[] | .context)] | unique[]'
    return 0
  fi
  if out="$(ghc_api GET "repos/${GHC_REPO}/rules/branches/${base}")"; then
    printf '%s' "$out" | jq -r '[.[]? | select(.type == "required_status_checks") | .parameters.required_status_checks[]?.context] | unique[]'
  fi
}

ghc_pr_checks() {
  local n pr sha base interval checks required fails pend
  ghc_parse "-R:repo --repo:repo --json:json -q:jq --jq:jq -t:template --template:template -i:interval --interval:interval" "--required:required --watch:watch --fail-fast:fail_fast -w:web --web:web" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  pr="$(ghc_api_or_die GET "repos/${GHC_REPO}/pulls/${n}")" || exit 1
  sha="$(printf '%s' "$pr" | jq -r .head.sha)"; base="$(printf '%s' "$pr" | jq -r .base.ref)"
  interval="${GHC_O_interval:-10}"
  required=""
  if [ "${GHC_B_required:-}" = 1 ]; then
    required="$(ghc_required_checks "$base")"
    [ -n "$required" ] || ghc_log "pr checks --required: required checks unreadable over REST; treating all checks as required"
  fi
  while :; do
    checks="$(ghc_rollup "$sha" | jq --arg req "$required" "${GHC_JQ_DEFS}"'
      ($req | split("\n") | map(select(. != ""))) as $r
      | map(if .__typename == "CheckRun" then
              {name, state: (if .status != "COMPLETED" then .status else .conclusion end),
               bucket: ({status: (.status | ascii_downcase), conclusion: (.conclusion | ascii_downcase | if . == "" then null else . end)} | bucket),
               link: .detailsUrl, description: "", workflow: .workflowName, startedAt, completedAt, event: ""}
            else
              {name: .context, state, link: .targetUrl, description: (.description // ""), workflow: "", startedAt: null, completedAt: null, event: "",
               bucket: (if .state == "SUCCESS" then "pass" elif .state == "PENDING" then "pending" else "fail" end)}
            end)
      | if ($r | length) > 0 then map(select(.name as $n | $r | index($n))) else . end')" || exit 1
    fails="$(printf '%s' "$checks" | jq '[.[] | select(.bucket == "fail" or .bucket == "cancel")] | length')"
    pend="$(printf '%s' "$checks" | jq '[.[] | select(.bucket == "pending")] | length')"
    if [ "${GHC_B_watch:-}" = 1 ] && [ "$pend" -gt 0 ]; then
      if ! { [ "${GHC_B_fail_fast:-}" = 1 ] && [ "$fails" -gt 0 ]; }; then sleep "$interval"; continue; fi
    fi
    break
  done
  if [ "$(printf '%s' "$checks" | jq length)" -eq 0 ]; then
    ghc_die "no checks reported on the '$(printf '%s' "$pr" | jq -r .head.ref)' branch"
  fi
  if [ -n "${GHC_O_json:-}" ]; then
    ghc_emit "$checks" || exit 1
  else
    printf '%s' "$checks" | jq -r '.[] | "\(.name)\t\(.bucket)\t0\t\(.link // "")\t\(.description)"'
  fi
  [ "$fails" -gt 0 ] && exit 1
  [ "$pend" -gt 0 ] && exit 8
  return 0
}

# ---------------------------------------------------------------------------
# gh issue ... / gh label ...
# ---------------------------------------------------------------------------
# ghc_issue_target TARGET -> sets GHC_N (TARGET: N, #N or issue/PR URL). Runs in
# the caller's shell: a URL names its own repository, as it does for gh.
ghc_issue_target() {
  local t="${1#\#}"
  if [[ "$t" =~ ^https?://.*/(issues|pull)/([0-9]+)/?$ ]]; then
    GHC_N="${BASH_REMATCH[2]}"
    GHC_REPO="$(ghc_repo_from_url "${t%/*/*}")" || ghc_die "gh: cannot parse issue URL: $1"
    return 0
  fi
  case "$t" in ''|*[!0-9]*) ghc_die "gh: invalid issue number: $1" ;; esac
  GHC_N="$t"
}

ghc_issue_view() {
  local n obj c
  ghc_parse "$GHC_COMMON_V" "-c:comments --comments:comments -w:web --web:web" "$@"
  ghc_resolve_repo
  ghc_issue_target "${GHC_POS[0]:-}"; n="$GHC_N"
  obj="$(ghc_api_or_die GET "repos/${GHC_REPO}/issues/${n}" | jq "${GHC_JQ_DEFS} issue")" || exit 1
  if ghc_wants closedByPullRequestsReferences; then ghc_log "issue view: closedByPullRequestsReferences has no REST equivalent; reporting []"; fi
  if ghc_wants comments || [ "${GHC_B_comments:-}" = 1 ]; then
    c="$(ghc_api GET "repos/${GHC_REPO}/issues/${n}/comments?per_page=100")" || c='[]'
    obj="$(jq -n --argjson o "$obj" --argjson r "$c" '$o + {comments: ($r | map({author: {login: (.user.login // "")}, body, createdAt: .created_at, url: .html_url}))}')"
  fi
  if [ -z "${GHC_O_json:-}" ]; then
    printf '%s' "$obj" | jq -r '"title:\t\(.title)\nstate:\t\(.state)\nauthor:\t\(.author.login)\nlabels:\t\([.labels[].name] | join(", "))\nnumber:\t\(.number)\nurl:\t\(.url)\n--\n\(.body)"'
    return 0
  fi
  ghc_emit "$obj"
}

ghc_issue_list() {
  local state limit q raw out
  ghc_parse "$GHC_COMMON_V -s:state --state:state -L:limit --limit:limit -S:search --search:search -l:label --label:label -a:assignee --assignee:assignee -A:author --author:author" "-w:web --web:web" "$@"
  ghc_resolve_repo
  state="${GHC_O_state:-open}"; limit="${GHC_O_limit:-30}"
  case "$limit" in ''|*[!0-9]*|0) ghc_die "invalid value for --limit: ${limit}" ;; esac
  if [ -n "${GHC_O_search:-}${GHC_O_author:-}" ]; then
    raw="$(ghc_search issue "$state" "${GHC_O_search:-}" "$limit")" || exit 1
  else
    q="state=${state}"
    [ -n "${GHC_O_label:-}" ] && q="${q}&labels=$(ghc_uri "$GHC_O_label")"
    [ -n "${GHC_O_assignee:-}" ] && q="${q}&assignee=$(ghc_uri "$(ghc_expand_me "$GHC_O_assignee")")"
    # /issues also returns pull requests; drop them per page so --limit counts issues.
    raw="$(ghc_paged "repos/${GHC_REPO}/issues?${q}" "$limit" 'map(select(.pull_request == null))')" || exit 1
  fi
  out="$(printf '%s' "$raw" | jq "${GHC_JQ_DEFS}"' map(select(.pull_request == null) | issue)')" || exit 1
  if [ -z "${GHC_O_json:-}" ]; then
    printf '%s' "$out" | jq -r '.[] | "\(.number)\t\(.state)\t\(.title)\t\([.labels[].name] | join(", "))\t\(.updatedAt)"'
    return 0
  fi
  ghc_emit "$out"
}

ghc_issue_create() {
  local payload resp labels
  ghc_parse "-R:repo --repo:repo -t:title --title:title -b:body --body:body -F:body_file --body-file:body_file -l:label --label:label -a:assignee --assignee:assignee -m:milestone --milestone:milestone -p:project --project:project -T:template --template:template" "-w:web --web:web" "$@"
  ghc_resolve_repo
  [ -n "${GHC_O_title:-}" ] || ghc_die "gh: --title is required when GraphQL is unavailable"
  labels="${GHC_O_label:-}"
  payload="$(jq -n --arg t "$GHC_O_title" --arg b "$(ghc_body)" --arg l "$labels" --arg a "$(ghc_expand_me "${GHC_O_assignee:-}")" \
    '{title: $t, body: $b} + (if $l != "" then {labels: ($l | split(","))} else {} end) + (if $a != "" then {assignees: ($a | split(","))} else {} end)')"
  resp="$(ghc_api POST "repos/${GHC_REPO}/issues" "$payload")" || {
    ghc_last
    # gh fails the whole create on an unknown label; mirror that message so
    # callers that retry without --label (step-03 does) behave as before.
    [ -n "$labels" ] && [ "$GHC_STATUS" = 422 ] && ghc_die "could not add label: '${labels}' not found"
    GHC_ERR="${GHC_ERR#gh: }"; ghc_die "gh: ${GHC_ERR:-issue create failed}"
  }
  printf '%s' "$resp" | jq -r .html_url
}

ghc_issue_state() { # ghc_issue_state close|reopen ARGS...
  local verb="$1" n state=closed reason
  shift
  ghc_parse "-R:repo --repo:repo -c:comment --comment:comment -r:reason --reason:reason" "" "$@"
  ghc_resolve_repo
  ghc_issue_target "${GHC_POS[0]:-}"; n="$GHC_N"
  [ "$verb" = reopen ] && state=open
  if [ -n "${GHC_O_comment:-}" ]; then
    ghc_api POST "repos/${GHC_REPO}/issues/${n}/comments" "$(jq -n --arg b "$GHC_O_comment" '{body: $b}')" >/dev/null || true
  fi
  reason="$(printf '%s' "${GHC_O_reason:-}" | tr ' A-Z' '_a-z')"
  ghc_api_or_die PATCH "repos/${GHC_REPO}/issues/${n}" "$(jq -n --arg s "$state" --arg r "$reason" '{state: $s} + (if $r != "" then {state_reason: $r} else {} end)')" >/dev/null || exit 1
}

ghc_issue_edit() {
  local n patch
  ghc_parse "-R:repo --repo:repo -t:title --title:title -b:body --body:body -F:body_file --body-file:body_file --add-label:add_label --remove-label:remove_label --add-assignee:assignee" "" "$@"
  ghc_resolve_repo
  ghc_issue_target "${GHC_POS[0]:-}"; n="$GHC_N"
  patch="$(jq -n --arg t "${GHC_O_title:-}" '{} + (if $t != "" then {title: $t} else {} end)')"
  if [ -n "${GHC_O_body:-}${GHC_O_body_file:-}" ]; then patch="$(jq -n --argjson p "$patch" --arg b "$(ghc_body)" '$p + {body: $b}')"; fi
  if [ "$patch" != "{}" ]; then ghc_api_or_die PATCH "repos/${GHC_REPO}/issues/${n}" "$patch" >/dev/null || exit 1; fi
  ghc_add_labels "$n" "${GHC_O_add_label:-}" || ghc_die "failed to add labels to #${n}"
  ghc_remove_labels "$n" "${GHC_O_remove_label:-}"
  printf 'https://github.com/%s/issues/%s\n' "$GHC_REPO" "$n"
}

ghc_label_create() {
  local name payload
  ghc_parse "-R:repo --repo:repo -c:color --color:color -d:description --description:description" "-f:force --force:force" "$@"
  ghc_resolve_repo
  name="${GHC_POS[0]:-}"; [ -n "$name" ] || ghc_die "gh: label name required"
  payload="$(jq -n --arg n "$name" --arg c "${GHC_O_color:-ededed}" --arg d "${GHC_O_description:-}" '{name: $n, color: ($c | ltrimstr("#")), description: $d}')"
  if ghc_api POST "repos/${GHC_REPO}/labels" "$payload" >/dev/null; then
    printf '✓ Label "%s" created in %s\n' "$name" "$GHC_REPO" >&2; return 0
  fi
  if [ "$GHC_STATUS" = 422 ]; then
    if [ "${GHC_B_force:-}" = 1 ]; then
      ghc_api_or_die PATCH "repos/${GHC_REPO}/labels/$(jq -rn --arg l "$name" '$l|@uri')" "$payload" >/dev/null || exit 1; return 0
    fi
    ghc_die "label with name \"$name\" already exists; use \`--force\` to update its color and description"
  fi
  ghc_die "gh: ${GHC_ERR#gh: }"
}

ghc_label_list() {
  local out
  ghc_parse "$GHC_COMMON_V -L:limit --limit:limit -S:search --search:search" "-w:web --web:web" "$@"
  ghc_resolve_repo
  out="$(ghc_api_or_die GET "repos/${GHC_REPO}/labels?per_page=${GHC_O_limit:-100}" | jq 'map({name, color, description, url})')" || exit 1
  if [ -z "${GHC_O_json:-}" ]; then
    printf '%s' "$out" | jq -r '.[] | "\(.name)\t\(.description // "")\t#\(.color)"'; return 0
  fi
  ghc_emit "$out"
}

# ---------------------------------------------------------------------------
# gh api graphql — only the viewer/permission query the identity preflight
# sends has a REST answer. Anything else fails as it did, and is logged.
# ---------------------------------------------------------------------------
ghc_api_graphql() {
  local query="" owner="" name="" jqf="" a f login perm repo out
  while [ $# -gt 0 ]; do
    f=""
    case "$1" in
      -f|-F|--field|--raw-field) f="${2-}"; shift; [ $# -eq 0 ] || shift ;;
      -f?*|-F?*) f="${1:2}"; f="${f#=}"; shift ;;
      --field=*|--raw-field=*) f="${1#*=}"; shift ;;
      -q|--jq) jqf="${2-}"; shift; [ $# -eq 0 ] || shift ;;
      -q?*) jqf="${1:2}"; jqf="${jqf#=}"; shift ;;
      --jq=*) jqf="${1#--jq=}"; shift ;;
      --hostname|-H|--header) shift; [ $# -eq 0 ] || shift ;;
      *) shift ;;
    esac
    case "$f" in query=*) query="${f#query=}" ;; owner=*) owner="${f#owner=}" ;; name=*) name="${f#name=}" ;; esac
  done
  # Exact shapes only (whitespace ignored). A substring test such as *viewer*
  # also matches reviewer/viewerDidAuthor queries and would answer them with a
  # bare viewer object and exit 0 — a fabricated success.
  a="$(printf '%s' "$query" | tr -d ' \t\r\n')"
  case "$a" in
    '{viewer{login}}'|'query{viewer{login}}') ;;
    'query($owner:String!,$name:String!){viewer{login}repository(owner:$owner,name:$name){nameWithOwnerviewerPermission}}') ;;
    *) ghc_log "api graphql: no REST equivalent for this query; failing as gh does"
       ghc_die "gh: GitHub GraphQL is not available and this query has no REST equivalent (HTTP 403)" ;;
  esac
  login="$(ghc_api_or_die GET user | jq -r .login)" || exit 1
  if [[ "$a" == *viewerPermission* ]] && [ -n "$owner" ] && [ -n "$name" ]; then
    repo="$(ghc_api_or_die GET "repos/${owner}/${name}")" || exit 1
    perm="$(printf '%s' "$repo" | jq -r '.permissions // {} | if .admin then "ADMIN" elif .maintain then "MAINTAIN" elif .push then "WRITE" elif .triage then "TRIAGE" elif .pull then "READ" else "" end')"
    out="$(jq -n -c --arg l "$login" --arg r "$(printf '%s' "$repo" | jq -r .full_name)" --arg p "$perm" \
      '{data: {viewer: {login: $l}, repository: {nameWithOwner: $r, viewerPermission: $p}}}')"
  else
    out="$(jq -n -c --arg l "$login" '{data: {viewer: {login: $l}}}')"
  fi
  if [ -n "$jqf" ]; then printf '%s' "$out" | jq -r "$jqf"; else printf '%s\n' "$out"; fi
}

# gh auth status. On a GraphQL-blocked host gh's own token check fails ("The
# token in GH_TOKEN is invalid") although the token works for REST. Only there
# is the failure replaced, and only after confirming both halves: GraphQL
# answers with the block text, and REST /user answers. Anywhere else (an expired
# token, a second account, another host) gh's real output and exit code stand.
ghc_auth_status() {
  local outf errf probef rc=0 login a
  for a in "$@"; do case "$a" in -h|--hostname|--hostname=*|-h?*) GHC_AUTH_HOSTNAME=1 ;; esac; done
  outf="${GHC_RUN_DIR}/auth.out"; errf="${GHC_RUN_DIR}/auth.err"; probef="${GHC_RUN_DIR}/auth.probe"
  "$GHC_REAL" auth status "$@" >"$outf" 2>"$errf" || rc=$?
  if [ "$rc" -ne 0 ] && [ -z "${GHC_AUTH_HOSTNAME:-}" ]; then
    if ! ghc_rest_mode; then
      "$GHC_REAL" api graphql -f query='{viewer{login}}' >/dev/null 2>"$probef" || true
      if grep -Eiq "$GHC_BLOCK_RE" "$probef"; then ghc_mark_blocked; fi
    fi
    if ghc_rest_mode && login="$(ghc_api GET user 2>/dev/null | jq -r '.login // empty')" && [ -n "$login" ]; then
      ghc_log "auth status: gh's token check failed on a GraphQL-blocked host; REST /user answers as ${login}"
      printf 'github.com\n  ✓ Logged in to github.com account %s (GraphQL is blocked on this host; token verified over REST by amplihack gh-compat)\n' "$login"
      return 0
    fi
  fi
  cat "$outf"; cat "$errf" >&2
  return "$rc"
}

ghc_mark_blocked() {
  [ -n "$GHC_STATE" ] && { : >"$GHC_STATE"; } 2>/dev/null
  ghc_log "host refuses GitHub GraphQL; routing gh issue/pr/label/api graphql over REST (marker: ${GHC_STATE:-none})"
  return 0
}

ghc_rest_dispatch() {
  local group="$1" verb="${2:-}"
  shift 2
  case "$group $verb" in
    "pr view") ghc_pr_view "$@" ;;
    "pr list") ghc_pr_list "$@" ;;
    "pr create") ghc_pr_create "$@" ;;
    "pr edit") ghc_pr_edit "$@" ;;
    "pr comment") ghc_comment pr "$@" ;;
    "pr ready") ghc_pr_ready "$@" ;;
    "pr merge") ghc_pr_merge "$@" ;;
    "pr close") ghc_pr_close "$@" ;;
    "pr diff") ghc_pr_diff "$@" ;;
    "pr checks") ghc_pr_checks "$@" ;;
    "issue view") ghc_issue_view "$@" ;;
    "issue list") ghc_issue_list "$@" ;;
    "issue create") ghc_issue_create "$@" ;;
    "issue comment") ghc_comment issue "$@" ;;
    "issue close") ghc_issue_state close "$@" ;;
    "issue reopen") ghc_issue_state reopen "$@" ;;
    "issue edit") ghc_issue_edit "$@" ;;
    "label create") ghc_label_create "$@" ;;
    "label list") ghc_label_list "$@" ;;
    "api graphql") ghc_api_graphql "$@" ;;
    *) ghc_log "no REST equivalent for 'gh $group $verb'"
       ghc_die "gh-compat: 'gh $group $verb' needs GitHub GraphQL, which this host blocks, and has no REST fallback" ;;
  esac
}

# ghc_reads_stdin ARGS... — true when a --body-file/-F argument is "-".
ghc_reads_stdin() {
  local prev="" a
  for a in "$@"; do
    case "$prev $a" in "-F -"|"--body-file -") return 0 ;; esac
    case "$a" in --body-file=-|-F-|-F=-) return 0 ;; esac
    prev="$a"
  done
  return 1
}

# ghc_stderr_filter FILE — the real gh's stderr, line by line: every line goes
# to FILE, and every line except the GraphQL-block refusal is passed on at once
# (a long `pr checks --watch` keeps its progress). The refusal is held back
# because the REST replay answers in its place. Reads all of its input.
ghc_stderr_filter() {
  local l
  while IFS= read -r l || [ -n "$l" ]; do
    printf '%s\n' "$l" >>"$1"
    # Only lines naming GraphQL pay for a grep (-i: bash 3.2's =~ has no nocasematch).
    case "$l" in
      *[Gg][Rr][Aa][Pp][Hh][Qq][Ll]*) printf '%s\n' "$l" | grep -Ei "$GHC_BLOCK_RE" >/dev/null && continue ;;
    esac
    printf '%s\n' "$l" >&2
  done
  return 0
}

ghc_main() {
  GHC_REAL="$(ghc_find_real_gh)" || { printf 'gh: command not found (amplihack gh-compat found no real gh on PATH)\n' >&2; exit 127; }
  case "${1:-} ${2:-}" in
    "auth status"|"pr "?*|"issue "?*|"label "?*|"api graphql") ;;
    *) exec "$GHC_REAL" "$@" ;;
  esac
  command -v jq >/dev/null 2>&1 || exec "$GHC_REAL" "$@"
  # Private per-invocation scratch (REST error/status hand-off, request bodies,
  # buffered stdin); never a predictable name in a shared TMPDIR.
  GHC_RUN_DIR="$(mktemp -d "${GHC_TMP}/ghc.XXXXXX")" || exec "$GHC_REAL" "$@"
  trap 'rm -rf "$GHC_RUN_DIR"' EXIT
  ghc_init_state
  if [ "${1:-} ${2:-}" = "auth status" ]; then shift 2; ghc_auth_status "$@"; exit $?; fi
  if ! ghc_rest_mode; then
    local errf rc=0 stdinf=""
    errf="${GHC_RUN_DIR}/probe.err"; : >"$errf"
    # `--body-file -` reads stdin. The probe would consume it and leave the REST
    # replay with an empty body, so buffer it once and feed both.
    if ghc_reads_stdin "$@"; then
      stdinf="${GHC_RUN_DIR}/stdin"; cat >"$stdinf" || exit 1
    fi
    # stdout goes straight through (fd 4); stderr streams through the filter.
    # Under pipefail the pipeline's status is gh's: the filter always returns 0.
    exec 4>&1
    if [ -n "$stdinf" ]; then
      { "$GHC_REAL" "$@" <"$stdinf" 2>&1 1>&4 4>&-; } | ghc_stderr_filter "$errf" || rc=$?
    else
      { "$GHC_REAL" "$@" 2>&1 1>&4 4>&-; } | ghc_stderr_filter "$errf" || rc=$?
    fi
    exec 4>&-
    if [ "$rc" -eq 0 ] || ! grep -Eiq "$GHC_BLOCK_RE" "$errf"; then exit "$rc"; fi
    [ -z "$stdinf" ] || exec <"$stdinf"
    ghc_mark_blocked
  fi
  ghc_log "REST fallback: gh $1 $2"
  ghc_rest_dispatch "$@"
}

if [ "${GHC_LIB_ONLY:-0}" != 1 ]; then ghc_main "$@"; fi
