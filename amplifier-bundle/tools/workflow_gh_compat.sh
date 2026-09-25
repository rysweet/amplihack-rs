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
# LIMITS. `pr list` / `issue list` / `label list` page through REST up to
# --limit, reading at most ~10 extra pages past it when a filter (merged, PRs
# mixed into /issues, the search fallback) drops items; /search stops at its
# 1000 results. Per-PR sub-lists (reviews, files, commits, comments, check
# runs, statuses, timeline) read every page. `--json` is checked against the
# real gh's own field list; fields only GraphQL has (Projects, PR reactions,
# isPinned, label timestamps) fail the call instead of reading null.
# `closedByPullRequestsReferences` comes from the issue's cross-reference
# timeline: open or merged PRs whose body closes the issue. A link made only in
# the Development sidebar leaves no REST trace and is not seen (logged); its one
# caller, workflow-design step-06d, then fails closed.
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
ghc_warn() { printf 'warning: %s\n' "$1" >&2; ghc_log "warning: $1"; }

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
  [ "${GHC_PAGINATE:-0}" = 1 ] && args+=(--paginate)
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

# ghc_all PATH [EXTRACT] — every page of a GET list as one array (gh api
# --paginate follows the Link headers). EXTRACT picks the array out of an
# object page, e.g. .check_runs. Failure is gh-style: a partial list is never
# passed off as the whole one.
ghc_all() {
  local out
  out="$(GHC_PAGINATE=1 ghc_api_or_die GET "$1")" || exit 1
  printf '%s' "$out" | jq -s "map(${2:-.}) | add // []"
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
    raw="$(ghc_api_or_die GET "${path}&per_page=${per}&page=${page}" | jq "${GHC_PAGE_ITEMS:-.}")" || { rm -f "$acc"; return 1; }
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
      # Ignoring a flag would silently widen or change the call (an unknown
      # --milestone would list every issue), so it fails the way gh does.
      -?*) ghc_die "gh-compat: flag $a of 'gh ${GHC_GROUP:-} ${GHC_VERB:-}' has no REST fallback; it needs GitHub GraphQL, which this host blocks" ;;
      *) GHC_POS+=("$a"); shift ;;
    esac
  done
  ghc_check_json
}

# Fields a newer gh has that the installed one may predate; callers ask for
# them (a gh that lacks them rejects them where GraphQL works, too).
GHC_NEWER_FIELDS=" pr.view:closingIssuesReferences pr.list:closingIssuesReferences issue.view:closedByPullRequestsReferences issue.list:closedByPullRequestsReferences "
# Fields gh knows but REST cannot answer (Projects and PR reactions are GraphQL
# only; label timestamps and pinning are not in the REST objects).
GHC_NO_REST_FIELDS=" pr.view:projectCards pr.view:projectItems pr.view:reactionGroups"
GHC_NO_REST_FIELDS="$GHC_NO_REST_FIELDS pr.list:projectCards pr.list:projectItems pr.list:reactionGroups"
GHC_NO_REST_FIELDS="$GHC_NO_REST_FIELDS issue.view:projectCards issue.view:projectItems issue.view:isPinned"
GHC_NO_REST_FIELDS="$GHC_NO_REST_FIELDS issue.list:projectCards issue.list:projectItems issue.list:isPinned"
GHC_NO_REST_FIELDS="$GHC_NO_REST_FIELDS label.list:createdAt label.list:updatedAt "

# ghc_gh_fields GROUP VERB — the installed gh's --json fields, space separated
# (empty when it prints no list).
ghc_gh_fields() {
  "$GHC_REAL" "$1" "$2" --json 2>&1 | sed -n 's/^  *\([A-Za-z][A-Za-z]*\)$/\1/p' | tr '\n' ' '
}

# ghc_wants_newer_field ARGS... — the call asks --json for a GHC_NEWER_FIELDS
# field that the installed gh does not know, so the real gh would refuse it
# before reaching GitHub, and the probe would never see a GraphQL block.
ghc_wants_newer_field() {
  local group="${1:-}" verb="${2:-}" json="" prev="" a f avail
  for a in "$@"; do
    case "$prev" in --json) json="$a" ;; esac
    case "$a" in --json=*) json="${a#--json=}" ;; esac
    prev="$a"
  done
  [ -n "$json" ] || return 1
  for f in $GHC_NEWER_FIELDS; do
    case "$f" in "$group.$verb":*) ;; *) continue ;; esac
    case ",$json," in *",${f#*:},"*) ;; *) continue ;; esac
    [ -n "${avail+set}" ] || avail="$(ghc_gh_fields "$group" "$verb")"
    case " $avail " in *" ${f#*:} "*) ;; *) return 0 ;; esac
  done
  return 1
}

# ghc_graphql_blocked — ask GitHub GraphQL the smallest question; true when the
# answer is the block refusal.
ghc_graphql_blocked() {
  "$GHC_REAL" api graphql -f query='{viewer{login}}' >/dev/null 2>"${GHC_RUN_DIR}/graphql.probe" || true
  grep -Eiq "$GHC_BLOCK_RE" "${GHC_RUN_DIR}/graphql.probe"
}

# ghc_check_json — validate --json against the real gh's own field list (it
# prints it offline for a bare --json), failing with gh's words on a field it
# does not know. A typo must not come back as null on blocked hosts only. A
# field gh knows but REST cannot answer fails too, rather than reading null.
ghc_check_json() {
  local avail f
  [ -n "${GHC_O_json:-}" ] || return 0
  avail="$(ghc_gh_fields "$GHC_GROUP" "$GHC_VERB")"
  for f in $GHC_NEWER_FIELDS; do
    case "$f" in "$GHC_GROUP.$GHC_VERB":*)
      [ -z "$avail" ] || case " $avail " in *" ${f#*:} "*) ;; *) avail="$avail ${f#*:}" ;; esac ;;
    esac
  done
  for f in $(printf '%s' "$GHC_O_json" | tr ',' ' '); do
    case " $avail " in
      "  "|*" $f "*) ;;   # known to gh (or gh printed no list to check against)
      *) { printf 'Unknown JSON field: "%s"\nAvailable fields:\n' "$f"; printf '  %s\n' $avail; } >&2
         ghc_log "unknown --json field $f for gh $GHC_GROUP $GHC_VERB"; exit 1 ;;
    esac
    case "$GHC_NO_REST_FIELDS" in
      *" $GHC_GROUP.$GHC_VERB:$f "*) ghc_die "gh-compat: --json field \"$f\" of 'gh $GHC_GROUP $GHC_VERB' needs GitHub GraphQL, which this host blocks, and has no REST equivalent" ;;
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

export GHC_REPO=""   # exported: the jq shapes read it as env.GHC_REPO
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
def login: if . then {login: (.login // "")} else null end;
def reponame($r): ($r | split("/")) as $p | {name: $p[1], owner: {login: $p[0]}};
# Closing keywords in a body: "#N" (this repository), "OWNER/REPO#N" and issue
# URLs, as GitHub links them. $home is the repository the body lives in.
def closing($home): [ (.body // "") | scan("(?i)\\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\\s*:?\\s+(?:https?://github\\.com/([\\w.-]+)/([\\w.-]+)/issues/|([\\w.-]+)/([\\w.-]+)#|#)([0-9]+)\\b")
  | (if .[0] then "\(.[0])/\(.[1])" elif .[2] then "\(.[2])/\(.[3])" else $home end) as $r
  | {number: (.[4] | tonumber), repository: reponame($r), url: "https://github.com/\($r)/issues/\(.[4])"} ] | unique;
def milestone_: if .milestone then {number: .milestone.number, title: .milestone.title, description: (.milestone.description // ""), dueOn: .milestone.due_on} else null end;
def reactions: [ (.reactions // {}) | to_entries[]
  | select((.value | type) == "number" and .value > 0 and .key != "total_count")
  | {content: ({"+1": "THUMBS_UP", "-1": "THUMBS_DOWN", laugh: "LAUGH", hooray: "HOORAY", confused: "CONFUSED", heart: "HEART", rocket: "ROCKET", eyes: "EYES"}[.key]), users: {totalCount: .value}}
  | select(.content != null) ];
def pr: (.base.repo.full_name // env.GHC_REPO // "") as $home | {
  number, id: .node_id, fullDatabaseId: (.id | if . then tostring else null end),
  title: (.title // ""), body: (.body // ""),
  state: (if (.merged_at // null) != null then "MERGED" elif .state == "open" then "OPEN" else "CLOSED" end),
  closed: (.state != "open"),
  isDraft: (.draft // false), createdAt: .created_at, updatedAt: .updated_at,
  closedAt: .closed_at, mergedAt: .merged_at, url: .html_url,
  headRefName: .head.ref, baseRefName: .base.ref, headRefOid: .head.sha, baseRefOid: .base.sha,
  headRepositoryOwner: {login: (.head.repo.owner.login // "")},
  headRepository: {name: (.head.repo.name // ""), nameWithOwner: (.head.repo.full_name // "")},
  isCrossRepository: ((.head.repo.full_name // "") != (.base.repo.full_name // "")),
  author: {login: (.user.login // "")}, labels: [.labels[]? | {name, color, description}],
  assignees: [.assignees[]? | {login}], milestone: milestone_,
  maintainerCanModify: (.maintainer_can_modify // false),
  mergeable: (if .mergeable == true then "MERGEABLE" elif .mergeable == false then "CONFLICTING" else "UNKNOWN" end),
  mergeStateStatus: (.mergeable_state // "unknown" | ascii_upcase),
  additions, deletions, changedFiles: .changed_files,
  mergeCommit: (if .merged_at then {oid: .merge_commit_sha} else null end),
  # For an open PR, merge_commit_sha is the test merge commit.
  potentialMergeCommit: (if .merged_at == null and .merge_commit_sha then {oid: .merge_commit_sha} else null end),
  mergedBy: (.merged_by | login),
  autoMergeRequest: (if .auto_merge then {mergeMethod: (.auto_merge.merge_method | up), enabledBy: (.auto_merge.enabled_by | login),
    commitHeadline: .auto_merge.commit_title, commitBody: .auto_merge.commit_message, enabledAt: null} else null end),
  reviewRequests: ([.requested_reviewers[]? | {__typename: "User", login}] + [.requested_teams[]? | {__typename: "Team", name, slug}]),
  # GitHub honours closing keywords only in PRs into the default branch.
  closingIssuesReferences: (if .base.ref == (.base.repo.default_branch // .base.ref) then closing($home) else [] end),
  reviewDecision: ""
};
def issue: {
  number, id: .node_id, title: (.title // ""), body: (.body // ""), state: (.state | up),
  closed: (.state != "open"),
  stateReason: (.state_reason | up), url: .html_url, author: {login: (.user.login // "")},
  labels: [.labels[]? | {name, color, description}], assignees: [.assignees[]? | {login}],
  createdAt: .created_at, updatedAt: .updated_at, closedAt: .closed_at,
  milestone: milestone_, reactionGroups: reactions
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
    # gh prints --jq results compact. The leading space keeps an expression
    # such as "-1" from reading as a jq option.
    printf '%s' "$json" | jq -r -c " $GHC_O_jq"
  else
    printf '%s\n' "$json" | jq .
  fi
}

ghc_wants() { case ",${GHC_O_json:-}," in *",$1,"*) return 0 ;; esac; return 1; }
ghc_wants_any() { local f; for f in "$@"; do ghc_wants "$f" && return 0; done; return 1; }

# Check runs + commit statuses for SHA, as gh's statusCheckRollup.
ghc_rollup() {
  local sha="$1" names="${2:-soft}" runs statuses wruns
  # A merge gate reads this: an unreadable list fails the call, never "no checks".
  runs="$(ghc_all "repos/${GHC_REPO}/commits/${sha}/check-runs?filter=latest&per_page=100" .check_runs)" || exit 1
  statuses="$(ghc_all "repos/${GHC_REPO}/commits/${sha}/status?per_page=100" .statuses)" || exit 1
  # Workflow name and triggering event live on the Actions run a check run's
  # details_url points into; one listing covers every run for the commit.
  # NAMES: skip (not asked for), strict (asked for: unreadable fails the call),
  # soft (part of statusCheckRollup: unreadable reads null, never a made-up
  # "", and never fails a merge gate over a cosmetic field).
  wruns='[]'
  if [ "$names" != skip ] && printf '%s' "$runs" | jq -e 'any(.[]; (.details_url // "") | test("/actions/runs/[0-9]+"))' >/dev/null; then
    if [ "$names" = strict ]; then
      wruns="$(ghc_all "repos/${GHC_REPO}/actions/runs?head_sha=${sha}&per_page=100" .workflow_runs)" || exit 1
    elif ! wruns="$(GHC_PAGINATE=1 ghc_api GET "repos/${GHC_REPO}/actions/runs?head_sha=${sha}&per_page=100" | jq -s 'map(.workflow_runs) | add // []')"; then
      ghc_log "workflow runs for ${sha} unreadable; statusCheckRollup workflowName is null"; wruns=null
    fi
  fi
  jq -n --argjson r "$runs" --argjson s "$statuses" --argjson w "$wruns" '
    (($w // []) | map({key: (.id | tostring), value: {name, event}}) | from_entries) as $wm
    | [$r[] | ((.details_url // "") | capture("/actions/runs/(?<id>[0-9]+)").id // "") as $id
      | {__typename: "CheckRun", name, status: (.status // "" | ascii_upcase),
      conclusion: (.conclusion // "" | ascii_upcase), detailsUrl: .details_url, title: (.output.title // ""),
      startedAt: .started_at, completedAt: .completed_at,
      workflowName: (if $w == null then null else ($wm[$id].name // "") end),
      event: (if $w == null then null else ($wm[$id].event // "") end)}]
    + [$s[] | {__typename: "StatusContext", context, state: (.state // "" | ascii_upcase),
      targetUrl: .target_url, description}]'
}

# ghc_pr_full NUMBER — mapped PR, enriched with whatever --json asked for.
ghc_pr_full() {
  local n="$1" obj extra
  obj="$(ghc_api_or_die GET "repos/${GHC_REPO}/pulls/${n}" | jq "${GHC_JQ_DEFS} pr")" || exit 1
  if ghc_wants_any reviews latestReviews reviewDecision; then
    # A merge gate reads reviewDecision; an unreadable review list must fail the
    # call, not report "no reviews". REST has no REVIEW_REQUIRED (that needs
    # branch protection); mergeStateStatus BLOCKED still carries it.
    extra="$(ghc_all "repos/${GHC_REPO}/pulls/${n}/reviews?per_page=100")" || exit 1
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '
      ($r | map({author: {login: (.user.login // "")}, authorAssociation: .author_association, state, body, submittedAt: .submitted_at, id: .node_id})) as $m
      | ([$r[] | select(.state == "APPROVED" or .state == "CHANGES_REQUESTED" or .state == "DISMISSED")]
         | group_by(.user.login // "") | map(max_by(.submitted_at // "") | .state)) as $last
      | $o + {reviews: $m, latestReviews: $m,
              reviewDecision: (if any($last[]; . == "CHANGES_REQUESTED") then "CHANGES_REQUESTED"
                               elif any($last[]; . == "APPROVED") then "APPROVED" else "" end)}')" || exit 1
  fi
  if ghc_wants statusCheckRollup; then
    extra="$(ghc_rollup "$(printf '%s' "$obj" | jq -r .headRefOid)")" || exit 1
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {statusCheckRollup: ($r | map(del(.title, .event)))}')"
  fi
  if ghc_wants files; then
    extra="$(ghc_all "repos/${GHC_REPO}/pulls/${n}/files?per_page=100")" || exit 1
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {files: ($r | map({path: .filename, additions, deletions}))}')"
  fi
  if ghc_wants commits; then
    extra="$(ghc_all "repos/${GHC_REPO}/pulls/${n}/commits?per_page=100")" || exit 1
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {commits: ($r | map({oid: .sha, messageHeadline: (.commit.message | split("\n")[0])}))}')"
  fi
  if ghc_wants comments; then
    extra="$(ghc_comments "$n")" || exit 1
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {comments: $r}')"
  fi
  printf '%s\n' "$obj"
}

# ghc_comments NUMBER — an issue's or PR's conversation comments, gh's shape.
ghc_comments() {
  ghc_all "repos/${GHC_REPO}/issues/$1/comments?per_page=100" \
    | jq 'map({id: .node_id, author: {login: (.user.login // "")}, authorAssociation: .author_association, body,
               createdAt: .created_at, includesCreatedEdit: (.updated_at != null and .updated_at != .created_at), url: .html_url})'
}

# Non-JSON `view --comments`: gh prints, without a terminal, only the raw
# comment list (and a PR's reviews), oldest first.
# shellcheck disable=SC2016  # jq program, not shell expansions.
GHC_JQ_RAW_COMMENTS='
  [ (.comments // [])[] | {a: .author.login, as: (.authorAssociation // "none"), e: (.includesCreatedEdit // false), st: "none", b: (.body // ""), t: (.createdAt // "")} ]
  + [ (.reviews // [])[] | select((.body // "") != "" or .state != "COMMENTED")
      | {a: .author.login, as: (.authorAssociation // "none"), e: false, st: .state, b: (.body // ""), t: (.submittedAt // "")} ]
  | sort_by(.t)[] | "author:\t\(.a)\nassociation:\t\(.as | ascii_downcase)\nedited:\t\(.e)\nstatus:\t\(.st | ascii_downcase)\n--\n\(.b)\n--"'


# ghc_closed_by NUMBER — gh's closedByPullRequestsReferences: the open or merged
# pull requests into their default branch whose body names this issue with a
# closing keyword, found through the issue's cross-reference timeline. A link
# made by hand in the Development sidebar leaves no trace in REST and is not
# seen (logged each time).
ghc_closed_by() {
  local cands out="[]" c pr
  ghc_log "closedByPullRequestsReferences: from cross-referencing PR bodies; sidebar-only links are not visible over REST"
  cands="$(ghc_all "repos/${GHC_REPO}/issues/$1/timeline?per_page=100" | jq -c --arg repo "$GHC_REPO" --argjson n "$1" "${GHC_JQ_DEFS}"'
    [ .[] | select(.event == "cross-referenced") | .source.issue // empty
      | select(.pull_request != null and (.state == "open" or .pull_request.merged_at != null))
      | (.repository.full_name // "") as $src
      | select(any(closing($src)[]; .number == $n and "\(.repository.owner.login)/\(.repository.name)" == $repo))
      | {id: .node_id, number, url: .html_url, repository: reponame($src), src: $src} ] | unique_by(.url) | .[]')" || exit 1
  # Only a PR into its default branch closes anything; the timeline does not
  # say which branch a PR targets, so each candidate is looked up.
  while IFS= read -r c; do
    [ -n "$c" ] || continue
    pr="$(ghc_api_or_die GET "repos/$(printf '%s' "$c" | jq -r '.src')/pulls/$(printf '%s' "$c" | jq -r .number)")" || exit 1
    printf '%s' "$pr" | jq -e '.base.ref == (.base.repo.default_branch // .base.ref)' >/dev/null || continue
    out="$(jq -n --argjson a "$out" --argjson c "$c" '$a + [$c | del(.src)]')"
  done <<EOF_CANDS
$cands
EOF_CANDS
  printf '%s\n' "$out"
}

# ghc_search KIND(pr|issue) STATE TEXT LIMIT — /search/issues items (for
# issues) or at least their .number (for PRs), honouring --author, --label,
# --assignee, --head, --base and --draft as gh's search query does. The cloud
# proxy refuses /search (it only serves repository-scoped paths), so on failure
# the repository's issues or pulls are listed and everything matched
# client-side, the free-text terms against title and body.
ghc_search() {
  local kind="$1" state="$2" text="$3" limit="$4" q l rstate path author="${GHC_O_author:-}" assignee="${GHC_O_assignee:-}" qs=""
  q="repo:${GHC_REPO} is:${kind} ${text}"
  # /search serves the first 1000 results only.
  [ "$limit" -gt 1000 ] && { ghc_log "search: --limit ${limit} cut to /search's 1000"; limit=1000; }
  case "$state" in open|closed|merged) q="$q is:$state" ;; esac
  [ -n "$author" ] && q="$q author:${author}"
  [ -n "$assignee" ] && q="$q assignee:${assignee}"
  # Repeated --label means every one of them, so one qualifier each.
  while IFS= read -r l; do
    [ -z "$l" ] || q="$q label:\"$l\""
  done <<EOF_LABELS
$(printf '%s' "${GHC_O_label:-}" | tr ',' '\n')
EOF_LABELS
  [ -n "${GHC_O_head:-}" ] && q="$q head:${GHC_O_head#*:}"
  [ -n "${GHC_O_base:-}" ] && q="$q base:${GHC_O_base}"
  [ "${GHC_B_draft:-}" = 1 ] && q="$q draft:true"
  # Probe /search with one small page; when it answers, page through it.
  if ghc_api GET "search/issues?per_page=1&q=$(ghc_uri "$q")" >/dev/null; then
    GHC_PAGE_ITEMS='.items // []' ghc_paged "search/issues?q=$(ghc_uri "$q")" "$limit" '.'; return
  fi
  ghc_last
  ghc_log "search unavailable (${GHC_STATUS:-?}); matching '${text}' client-side over repos/${GHC_REPO}"
  # /search resolves @me server-side; the client-side match compares logins,
  # where a literal "@me" would silently match nobody (quality-loop's
  # `--author=@me` lists would come back empty).
  [ "$author" = "@me" ] && { author="$(ghc_api_or_die GET user | jq -r '.login // empty')" || exit 1; }
  [ -n "${GHC_O_author:-}" ] && [ -z "$author" ] && ghc_die "gh: could not resolve --author ${GHC_O_author}"
  rstate="$state"; case "$state" in open|closed) ;; merged) rstate=closed ;; *) rstate=all ;; esac
  if [ "$kind" = pr ]; then
    # /pulls filters head and base itself; /issues cannot.
    case "${GHC_O_head:-}" in '') ;; *:*) qs="&head=$(ghc_uri "$GHC_O_head")" ;; *) qs="&head=$(ghc_uri "${GHC_REPO%%/*}:${GHC_O_head}")" ;; esac
    [ -n "${GHC_O_base:-}" ] && qs="${qs}&base=$(ghc_uri "$GHC_O_base")"
    [ -n "$assignee" ] && { assignee="$(ghc_expand_me "$assignee")"; [ "$assignee" != "@me" ] || ghc_die "gh: could not resolve --assignee @me"; }
    path="repos/${GHC_REPO}/pulls?state=${rstate}${qs}"
  else
    [ -n "${GHC_O_label:-}" ] && qs="&labels=$(ghc_uri "$GHC_O_label")"
    [ -n "$assignee" ] && qs="${qs}&assignee=$(ghc_uri "$(ghc_expand_me "$assignee")")"
    assignee=""   # /issues filtered it
    path="repos/${GHC_REPO}/issues?state=${rstate}${qs}"
  fi
  # Full pages: the text match keeps few items, and --limit 1 must not stop
  # the scan after ~11 items.
  GHC_PAGE_SIZE=100 ghc_paged "$path" "$limit" '
    # Whole words, as /search matches them. A substring test lets "a" or "it"
    # match any body, and step-03 would adopt an unrelated issue as its tracker.
    def words: ascii_downcase | [scan("[a-z0-9]+")];
    ($t | split(" ") | map(select(contains(":") | not)) | join(" ") | words) as $words
    | ($l | split(",") | map(select(. != ""))) as $labels
    | map(select($k == "pr" or .pull_request == null)
          | select($a == "" or .user.login == $a)
          | select($k == "issue" or (
              ([.labels[]?.name] as $have | all($labels[]; . as $x | $have | index([$x]) != null))
              and ($as == "" or any(.assignees[]?; .login == $as))
              and ($d == "" or (.draft // false))
              and ($s != "merged" or .merged_at != null)))
          | select(((.title // "") + " " + (.body // "") | words) as $h | all($words[]; . as $w | $h | index([$w]) != null)))' \
    --arg k "$kind" --arg t "$text" --arg a "$author" --arg l "${GHC_O_label:-}" --arg as "$assignee" \
    --arg d "${GHC_B_draft:-}" --arg s "$state"
}

# ---------------------------------------------------------------------------
# gh pr ...
# ---------------------------------------------------------------------------
ghc_pr_view() {
  local n obj
  ghc_parse "$GHC_COMMON_V" "-c:comments --comments:comments -w:web --web:web" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  if [ -z "${GHC_O_json:-}" ] && [ "${GHC_B_comments:-}" = 1 ]; then
    obj="$(GHC_O_json=comments,reviews ghc_pr_full "$n")" || exit 1
    printf '%s' "$obj" | jq -r "$GHC_JQ_RAW_COMMENTS"; return 0
  fi
  obj="$(ghc_pr_full "$n")" || exit 1
  if [ -z "${GHC_O_json:-}" ]; then
    printf '%s' "$obj" | jq -r '"title:\t\(.title)\nstate:\t\(.state)\nauthor:\t\(.author.login)\nnumber:\t\(.number)\nurl:\t\(.url)\n--\n\(.body)"'
    return 0
  fi
  ghc_emit "$obj"
}

ghc_pr_list() {
  local state limit q raw owner nums n out="[]" obj
  ghc_parse "$GHC_COMMON_V -s:state --state:state -L:limit --limit:limit -H:head --head:head -B:base --base:base -S:search --search:search -A:author --author:author -l:label --label:label -a:assignee --assignee:assignee" "-d:draft --draft:draft -w:web --web:web" "$@"
  ghc_resolve_repo
  state="${GHC_O_state:-open}"; limit="${GHC_O_limit:-30}"; owner="${GHC_REPO%%/*}"
  case "$limit" in ''|*[!0-9]*|0) ghc_die "invalid value for --limit: ${limit}" ;; esac
  if [ -n "${GHC_O_search:-}${GHC_O_author:-}${GHC_O_label:-}${GHC_O_assignee:-}" ]; then
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
    # merged and --draft filter each page, so --limit counts what survives.
    out="$(ghc_paged "repos/${GHC_REPO}/pulls?state=${rstate}${qs}" "$limit" "${GHC_JQ_DEFS}"' map(pr)
      | if $s == "merged" then map(select(.state == "MERGED")) else . end
      | if $d == "1" then map(select(.isDraft)) else . end' --arg s "$state" --arg d "${GHC_B_draft:-}")" || exit 1
    # A /pulls listing leaves these out (or, for reviews, needs another call).
    if ghc_wants_any reviews latestReviews reviewDecision statusCheckRollup mergeable mergeStateStatus \
        files commits comments additions deletions changedFiles mergedBy maintainerCanModify potentialMergeCommit; then
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
  ghc_add_labels "$n" "${GHC_O_label:-}" || ghc_warn "could not add labels ${GHC_O_label} to #${n}: ${GHC_ERR#gh: }"
  if [ -n "${GHC_O_assignee:-}" ]; then
    # The PR exists either way, so its URL still prints; the miss is not silent.
    ghc_api POST "repos/${GHC_REPO}/issues/${n}/assignees" "$(ghc_csv_json assignees "$(ghc_expand_me "$GHC_O_assignee")")" >/dev/null \
      || ghc_warn "could not assign ${GHC_O_assignee} to #${n}: ${GHC_ERR#gh: }"
  fi
  if [ -n "${GHC_O_reviewer:-}" ]; then
    ghc_api POST "repos/${GHC_REPO}/pulls/${n}/requested_reviewers" "$(ghc_csv_json reviewers "$GHC_O_reviewer")" >/dev/null \
      || ghc_warn "could not request review from ${GHC_O_reviewer} on #${n}: ${GHC_ERR#gh: }"
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
  ghc_remove_labels "$n" "${GHC_O_remove_label:-}" || rc=1
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
  local l rc=0
  [ -n "${2:-}" ] || return 0
  # One label per line: a label name may hold spaces ("good first issue").
  while IFS= read -r l; do
    [ -n "$l" ] || continue
    ghc_api DELETE "repos/${GHC_REPO}/issues/$1/labels/$(ghc_uri "$l")" >/dev/null \
      || { ghc_log "label '$l' not removed from #$1: $GHC_ERR"; rc=1; }
  done <<EOF_LABELS
$(printf '%s' "$2" | tr ',' '\n')
EOF_LABELS
  return "$rc"
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
    ghc_api DELETE "repos/${GHC_REPO}/git/refs/heads/${branch}" >/dev/null \
      || ghc_die "failed to delete remote branch ${branch}: ${GHC_ERR#gh: }"
  fi
}

ghc_pr_close() {
  local n
  ghc_parse "-R:repo --repo:repo -c:comment --comment:comment" "-d:delete --delete-branch:delete" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  if [ -n "${GHC_O_comment:-}" ]; then
    ghc_api_or_die POST "repos/${GHC_REPO}/issues/${n}/comments" "$(jq -n --arg b "$GHC_O_comment" '{body: $b}')" >/dev/null || exit 1
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
    ghc_all "repos/${GHC_REPO}/pulls/${n}/files?per_page=100" | jq -r '.[].filename'
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
  local n pr sha base interval checks required fails pend names=skip
  ghc_parse "-R:repo --repo:repo --json:json -q:jq --jq:jq -t:template --template:template -i:interval --interval:interval" "--required:required --watch:watch --fail-fast:fail_fast -w:web --web:web" "$@"
  ghc_resolve_repo
  ghc_pr_target "${GHC_POS[0]:-}"; n="$GHC_N"
  pr="$(ghc_api_or_die GET "repos/${GHC_REPO}/pulls/${n}")" || exit 1
  sha="$(printf '%s' "$pr" | jq -r .head.sha)"; base="$(printf '%s' "$pr" | jq -r .base.ref)"
  interval="${GHC_O_interval:-10}"
  if ghc_wants_any workflow event; then names=strict; fi
  required=""
  if [ "${GHC_B_required:-}" = 1 ]; then
    required="$(ghc_required_checks "$base")"
    [ -n "$required" ] || ghc_log "pr checks --required: required checks unreadable over REST; treating all checks as required"
  fi
  while :; do
    checks="$(ghc_rollup "$sha" "$names" | jq --arg req "$required" "${GHC_JQ_DEFS}"'
      ($req | split("\n") | map(select(. != ""))) as $r
      | map(if .__typename == "CheckRun" then
              {name, state: (if .status != "COMPLETED" then .status else .conclusion end),
               bucket: ({status: (.status | ascii_downcase), conclusion: (.conclusion | ascii_downcase | if . == "" then null else . end)} | bucket),
               link: .detailsUrl, description: .title, workflow: .workflowName, startedAt, completedAt, event}
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

# ghc_issue_enrich ISSUE_JSON — add the fields that cost another request, when
# --json (or --comments) asks for them.
ghc_issue_enrich() {
  local obj="$1" n extra
  n="$(printf '%s' "$obj" | jq -r .number)"
  if ghc_wants comments || [ "${GHC_B_comments:-}" = 1 ]; then
    extra="$(ghc_comments "$n")" || exit 1
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {comments: $r}')"
  fi
  if ghc_wants closedByPullRequestsReferences; then
    extra="$(ghc_closed_by "$n")" || exit 1
    obj="$(jq -n --argjson o "$obj" --argjson r "$extra" '$o + {closedByPullRequestsReferences: $r}')"
  fi
  printf '%s\n' "$obj"
}

ghc_issue_view() {
  local n obj
  ghc_parse "$GHC_COMMON_V" "-c:comments --comments:comments -w:web --web:web" "$@"
  ghc_resolve_repo
  ghc_issue_target "${GHC_POS[0]:-}"; n="$GHC_N"
  obj="$(ghc_api_or_die GET "repos/${GHC_REPO}/issues/${n}" | jq "${GHC_JQ_DEFS} issue")" || exit 1
  obj="$(ghc_issue_enrich "$obj")" || exit 1
  if [ -z "${GHC_O_json:-}" ] && [ "${GHC_B_comments:-}" = 1 ]; then
    printf '%s' "$obj" | jq -r "$GHC_JQ_RAW_COMMENTS"; return 0
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
  if ghc_wants comments || ghc_wants closedByPullRequestsReferences; then
    local full="[]" i obj
    for i in $(printf '%s' "$out" | jq -r 'keys[]'); do
      obj="$(ghc_issue_enrich "$(printf '%s' "$out" | jq ".[$i]")")" || exit 1
      full="$(jq -n --argjson a "$full" --argjson o "$obj" '$a + [$o]')"
    done
    out="$full"
  fi
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
    ghc_api_or_die POST "repos/${GHC_REPO}/issues/${n}/comments" "$(jq -n --arg b "$GHC_O_comment" '{body: $b}')" >/dev/null || exit 1
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
  ghc_remove_labels "$n" "${GHC_O_remove_label:-}" || ghc_die "failed to remove labels from #${n}"
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
  local limit out
  ghc_parse "$GHC_COMMON_V -L:limit --limit:limit -S:search --search:search --sort:sort --order:order" "-w:web --web:web" "$@"
  ghc_resolve_repo
  limit="${GHC_O_limit:-30}"
  case "$limit" in ''|*[!0-9]*|0) ghc_die "invalid value for --limit: ${limit}" ;; esac
  case "${GHC_O_sort:-created}" in created|name) ;; *) ghc_die "invalid argument \"${GHC_O_sort}\" for \"--sort\" flag: valid values are {created|name}" ;; esac
  case "${GHC_O_order:-asc}" in asc|desc) ;; *) ghc_die "invalid argument \"${GHC_O_order}\" for \"--order\" flag: valid values are {asc|desc}" ;; esac
  # A repository's labels are few: read them all, then search (name or
  # description, as gh's label query), sort (REST ids follow creation order)
  # and cut to --limit here, so the cut sees the whole ordered list.
  out="$(ghc_all "repos/${GHC_REPO}/labels?per_page=100" | jq --arg q "${GHC_O_search:-}" --arg s "${GHC_O_sort:-created}" --arg o "${GHC_O_order:-asc}" --argjson n "$limit" '
    map(select($q == "" or ((.name + " " + (.description // "")) | ascii_downcase | contains($q | ascii_downcase))))
    | (if $s == "name" then sort_by(.name | ascii_downcase) else sort_by(.id) end)
    | (if $o == "desc" then reverse else . end) | .[:$n]
    | map({id: .node_id, name, color, description: (.description // ""), isDefault: (.default // false), url})')" || exit 1
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
  if [ -n "$jqf" ]; then printf '%s' "$out" | jq -r -c " $jqf"; else printf '%s\n' "$out"; fi
}

# gh auth status. On a GraphQL-blocked host gh's own token check fails ("The
# token in GH_TOKEN is invalid") although the token works for REST. Only there
# is the failure replaced, and only after confirming both halves: GraphQL
# answers with the block text, and REST /user answers. Anywhere else (an expired
# token, a second account, another host) gh's real output and exit code stand.
ghc_auth_status() {
  local outf errf rc=0 login a
  for a in "$@"; do case "$a" in -h|--hostname|--hostname=*|-h?*) GHC_AUTH_HOSTNAME=1 ;; esac; done
  outf="${GHC_RUN_DIR}/auth.out"; errf="${GHC_RUN_DIR}/auth.err"
  "$GHC_REAL" auth status "$@" >"$outf" 2>"$errf" || rc=$?
  if [ "$rc" -ne 0 ] && [ -z "${GHC_AUTH_HOSTNAME:-}" ]; then
    if ! ghc_rest_mode && ghc_graphql_blocked; then ghc_mark_blocked; fi
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
  GHC_GROUP="$group"; GHC_VERB="$verb"
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

# The real gh's stderr goes to a file, not a pipe, so the shim is done the
# moment gh exits: a child gh leaves behind holding the descriptor (a --web
# browser) cannot make it wait. ghc_stderr_follow tails that file in the
# background and passes each line on as it lands (a long `pr checks --watch`
# keeps its progress), except the GraphQL-block refusal: that one is held
# back, and ghc_main prints it only if no REST replay answers in its place.
#
# ghc_stderr_follow FILE DONE OWNER — stops once DONE exists (gh has exited, so
# FILE is complete) or the shim (pid OWNER) is gone.
ghc_stderr_follow() {
  local file="$1" donef="$2" owner="$3" l part="" last="" fin=0
  # A reader that went away must not kill the follower (the call would end 141)
  # or stall gh: writes just fail from then on, and the file keeps everything.
  trap '' PIPE
  GHC_ERR_OPEN=1
  exec 5<"$file" || return 0
  while :; do
    [ -e "$donef" ] && fin=1
    while IFS= read -r l <&5; do ghc_stderr_line "$part$l" nl; part=""; done
    part="$part$l"   # a failed read keeps the partial line it consumed
    if [ "$fin" = 1 ]; then [ -z "$part" ] || ghc_stderr_line "$part"; return 0; fi
    kill -0 "$owner" 2>/dev/null || return 0
    # A prompt with no newline yet: pass it on once it stops growing.
    if [ -n "$part" ] && [ "$part" = "$last" ]; then ghc_stderr_line "$part"; part=""; fi
    last="$part"
    sleep 0.1
  done
}

# ghc_stderr_line TEXT [nl] — pass TEXT on to stderr unless it is the block
# refusal. Only lines naming GraphQL pay for a grep (-i: bash 3.2's =~ has no
# nocasematch).
ghc_stderr_line() {
  case "$1" in
    *[Gg][Rr][Aa][Pp][Hh][Qq][Ll]*) printf '%s\n' "$1" | grep -Eiq "$GHC_BLOCK_RE" && return 0 ;;
  esac
  [ "$GHC_ERR_OPEN" = 1 ] || return 0
  { if [ -n "${2:-}" ]; then printf '%s\n' "$1"; else printf '%s' "$1"; fi; } >&2 2>/dev/null || GHC_ERR_OPEN=0
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
  local via_rest=0
  if ghc_rest_mode; then
    via_rest=1
  elif ghc_wants_newer_field "$@" && ghc_graphql_blocked; then
    ghc_mark_blocked; via_rest=1
  fi
  if [ "$via_rest" = 0 ]; then
    local errf rc=0 stdinf=""
    errf="${GHC_RUN_DIR}/probe.err"; : >"$errf"
    # `--body-file -` reads stdin. The probe would consume it and leave the REST
    # replay with an empty body, so buffer it once and feed both.
    if ghc_reads_stdin "$@"; then
      stdinf="${GHC_RUN_DIR}/stdin"; cat >"$stdinf" || exit 1
    fi
    # stdout goes straight through; stderr is followed from its file.
    local donef="${GHC_RUN_DIR}/probe.done" fpid
    ghc_stderr_follow "$errf" "$donef" "$$" >/dev/null &
    fpid=$!
    if [ -n "$stdinf" ]; then
      "$GHC_REAL" "$@" <"$stdinf" 2>"$errf" || rc=$?
    else
      "$GHC_REAL" "$@" 2>"$errf" || rc=$?
    fi
    : >"$donef"; wait "$fpid" 2>/dev/null
    if [ "$rc" -eq 0 ] || ! grep -Eiq "$GHC_BLOCK_RE" "$errf"; then
      # No replay: a held-back block line belongs to this answer after all.
      [ "$rc" -ne 0 ] || grep -Ei "$GHC_BLOCK_RE" "$errf" >&2 2>/dev/null
      exit "$rc"
    fi
    [ -z "$stdinf" ] || exec <"$stdinf"
    ghc_mark_blocked
  fi
  ghc_log "REST fallback: gh $1 $2"
  ghc_rest_dispatch "$@"
}

if [ "${GHC_LIB_ONLY:-0}" != 1 ]; then ghc_main "$@"; fi
