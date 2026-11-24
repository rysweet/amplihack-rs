# shellcheck shell=bash
# autodrive_build_lib.sh: the shared checks and lookups of the build goal
# (#3129 PR 2). Sourced by autodrive_build_goal.sh, autodrive_build_reading.sh
# and autodrive_build_run.sh; never executed. It sets umask 077 and `set -u`,
# and sources autodrive_goal_lib.sh for the goal_* helpers rather than copying
# them. docs/reference/build-goal-loop.md is the specification.
#
# Context comes only from RECIPE_VAR_* names, never from bare inherited
# variables (#1561). State files are a cache and never an authority: every
# write to GitHub derives the run branch again from the context and reads the
# pull request live (build_verify_pr).
#
#   build_turn_dir <dir>          goal_check_turn_dir, after making a turn
#                                 folder this user owns private (0700)
#   build_valid_branch <name>     git check-ref-format --branch, no leading
#                                 `-`, no control characters, at most 255 bytes
#   build_repo_slug <launch>      origin -> BUILD_REPO ([HOST/]OWNER/REPO) and
#                                 GH_REPO exported; BUILD_REPO_ERR on failure;
#                                 BUILD_REPO_MISMATCH=true when the context
#                                 pr_repository names another repository
#   build_ws_field <field>        one field of the worktree_setup JSON object
#   build_identity_load           <state>/identity.json -> ID_* variables
#   build_run_branch              the run branch -> BUILD_BRANCH, BUILD_BRANCH_SOURCE
#   build_worktree_lookup <launch> <branch>
#                                 the linked worktree on <branch>, from
#                                 `git worktree list --porcelain`
#   build_worktree_check <path> <branch> <launch>
#                                 0 when <path> is that worktree and is not the
#                                 launch checkout; BUILD_WT_WHY otherwise
#   build_own_pr                  OWN_PR, OWN_HOW (state|branch|context), the
#                                 per-source STATE_PR and BRANCH_PR, CROSS_PR,
#                                 PR_LIST_FAILED, PR_LIST_ERR and
#                                 BUILD_CANDIDATES (a JSON array)
#   build_verify_pr <n> <branch> <need_open>
#                                 a live read: VERIFY_CLASS ok|refused|
#                                 refused-cross-repo|not-open|failed,
#                                 VERIFY_WHY and VERIFY_JSON; returns 75 or 79
#                                 from gh, else 0
#   build_path_under <path> <root>
#                                 0 when <path> resolves under <root> with no
#                                 symlinked component
#   build_mentions_allowed <body> <allow>
#                                 0 when every @mention and closing keyword in
#                                 <body> also appears in <allow>; otherwise 1
#                                 with the first refused token on stdout
#   build_redact_tail             stdin -> one line of at most 200 bytes, URL
#                                 user information and token-shaped strings
#                                 removed
#   build_task_text <out>         the task text (AUTODRIVE_TASK_FILE, at most
#                                 65,536 bytes) into <out>
#   build_allow_text <out> [pr_url]
#                                 the text that may name mentions and closing
#                                 keywords: the task text, goal notes and
#                                 final_requirements, closing_reference,
#                                 then the operator's
#                                 guidance from `amplihack recipe guidance
#                                 show --operator-only [pr_url]`. pr_url is
#                                 passed only when it is a GitHub pull request
#                                 URL. The guidance is appended only when the
#                                 read exits 0; otherwise BUILD_GUIDANCE_UNREAD
#                                 holds its exit code ("" after a good read).
#                                 Always returns 0. issue_number is added by
#                                 build_mentions_allowed. --operator-only
#                                 keeps entries an agent step wrote out of the
#                                 list; it guards against a build widening its
#                                 own list by ordinary use and is not a trust
#                                 boundary. A guidance header's caller login of
#                                 the form user@domain lets @domain through;
#                                 that value comes from the operator's own
#                                 account, so it is accepted (S-V6).

# The variables set below are this library's outputs, read by its callers.
# shellcheck disable=SC2034
umask 077
set -u
_BUILD_LIB_HOME="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || return 2
# shellcheck source=autodrive_goal_lib.sh
. "$_BUILD_LIB_HOME/autodrive_goal_lib.sh" || { echo "ERROR: autodrive_goal_lib.sh is missing beside autodrive_build_lib.sh" >&2; return 2; }
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat GIT_TERMINAL_PROMPT=0

build_turn_dir() {
  local dir="${1:-}"
  goal_check_turn_dir "$dir" 2>/dev/null && return 0
  # A turn folder this user owns but made under a wider umask is tightened,
  # never refused; a symlink or another user's folder is still refused.
  case "$dir" in /*) ;; *) goal_check_turn_dir "$dir"; return 1 ;; esac
  local parent="${dir%/*}" d
  for d in "$parent" "$dir"; do
    if [ -d "$d" ] && [ ! -L "$d" ] && [ -O "$d" ]; then chmod 700 "$d" 2>/dev/null || :; fi
  done
  goal_check_turn_dir "$dir"
}

build_valid_branch() {
  local b="${1:-}" LC_ALL=C
  [ -n "$b" ] && [ "${#b}" -le 255 ] || return 1
  case "$b" in -*|HEAD|*@\{*) return 1 ;; esac
  [[ "$b" =~ [[:cntrl:][:space:]] ]] && return 1
  git check-ref-format --branch "$b" >/dev/null 2>&1
}

BUILD_REPO=""; BUILD_REPO_NAME=""; BUILD_REPO_ERR=""; BUILD_REPO_MISMATCH=false
build_repo_slug() {
  local launch="${1:-.}" url rest host path owner name want
  BUILD_REPO=""; BUILD_REPO_NAME=""; BUILD_REPO_ERR=""; BUILD_REPO_MISMATCH=false
  # The configured URL, before insteadOf rewriting: it names the repository
  # the operator cloned, whatever transport git is told to use for it.
  url="$(git -C "$launch" config --get remote.origin.url 2>/dev/null)" || { BUILD_REPO_ERR="the launch checkout has no origin remote"; return 1; }
  case "$url" in
    *://*) rest="${url#*://}"; rest="${rest##*@}"; host="${rest%%/*}"; host="${host%%:*}"; path="${rest#*/}" ;;
    *@*:*|*.*:*) rest="${url##*@}"; host="${rest%%:*}"; path="${rest#*:}" ;;
    *) BUILD_REPO_ERR="origin is a local path, not a GitHub repository"; return 1 ;;
  esac
  path="${path%/}"; path="${path%.git}"
  owner="${path%%/*}"; name="${path#*/}"
  host="$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')"
  if ! [[ "$owner" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || ! [[ "$name" =~ ^[A-Za-z0-9._-]+$ ]] || [ "$name" = "$path" ] \
     || ! [[ "$host" =~ ^[a-z0-9][a-z0-9.-]*$ ]]; then
    BUILD_REPO_ERR="origin cannot be read as [host/]owner/repo"; return 1
  fi
  BUILD_REPO_NAME="${owner}/${name}"
  if [ "$host" = github.com ]; then BUILD_REPO="$BUILD_REPO_NAME"; else BUILD_REPO="${host}/${BUILD_REPO_NAME}"; fi
  want="$(printf '%s' "${RECIPE_VAR_pr_repository:-}" | tr '[:upper:]' '[:lower:]')"
  if [ -n "$want" ] && [ "$want" != "$(printf '%s' "$BUILD_REPO_NAME" | tr '[:upper:]' '[:lower:]')" ] \
     && [ "$want" != "$(printf '%s' "$BUILD_REPO" | tr '[:upper:]' '[:lower:]')" ]; then
    BUILD_REPO_MISMATCH=true
  fi
  export GH_REPO="$BUILD_REPO"
}

build_ws_field() {
  printf '%s' "${RECIPE_VAR_worktree_setup:-}" | jq -r --arg f "$1" 'if type == "object" then (.[$f] // empty | strings) else empty end' 2>/dev/null
}

ID_BRANCH=""; ID_WORKTREE=""; ID_SOURCE=""; ID_BASE=""; ID_STARTED=""; ID_ERR=""
build_identity_load() {
  local f="${STATE:-}/identity.json"
  ID_BRANCH=""; ID_WORKTREE=""; ID_SOURCE=""; ID_BASE=""; ID_STARTED=""; ID_ERR=""
  [ -n "${STATE:-}" ] && [ -e "$f" ] || { ID_ERR="no identity.json in the state folder"; return 1; }
  goal_json_file "$f" 65536 || { ID_ERR="identity.json is not a JSON object file of at most 64 KiB"; return 1; }
  ID_BRANCH="$(jq -r '.branch // empty | strings' "$f")"
  ID_WORKTREE="$(jq -r '.worktree // empty | strings' "$f")"
  ID_SOURCE="$(jq -r '.source // empty | strings' "$f")"
  ID_BASE="$(jq -r '.base // empty | strings' "$f")"
  ID_STARTED="$(jq -r '.started // empty | strings' "$f")"
  build_valid_branch "$ID_BRANCH" || ID_BRANCH=""
  build_valid_branch "$ID_BASE" || ID_BASE=""
  case "$ID_WORKTREE" in /*) ;; *) ID_WORKTREE="" ;; esac
  [[ "$ID_STARTED" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z$ ]] || ID_STARTED=""
  return 0
}

# build_run_branch: worktree_setup.branch_name, then existing_branch. An
# adopted identity (prepare-worktree args.branch) names its own branch; the
# caller checks the adoption rule again before any write.
BUILD_BRANCH=""; BUILD_BRANCH_SOURCE="none"
build_run_branch() {
  local b
  BUILD_BRANCH=""; BUILD_BRANCH_SOURCE="none"
  build_identity_load >/dev/null 2>&1 || :
  if [ "$ID_SOURCE" = adopted ] && [ -n "$ID_BRANCH" ]; then BUILD_BRANCH="$ID_BRANCH"; BUILD_BRANCH_SOURCE=adopted; return 0; fi
  b="$(build_ws_field branch_name)"
  if build_valid_branch "$b"; then BUILD_BRANCH="$b"; BUILD_BRANCH_SOURCE=worktree_setup; return 0; fi
  b="${RECIPE_VAR_existing_branch:-}"
  if build_valid_branch "$b"; then BUILD_BRANCH="$b"; BUILD_BRANCH_SOURCE=existing_branch; return 0; fi
  return 1
}

build_worktree_lookup() {
  git -C "$1" worktree list --porcelain 2>/dev/null | awk -v want="refs/heads/$2" '
    /^worktree /{p = substr($0, 10)} /^branch /{if (substr($0, 8) == want) {print p; exit}}'
}

BUILD_WT_WHY=""
build_worktree_check() {
  local path="${1:-}" branch="${2:-}" launch="${3:-}" listed real lreal
  BUILD_WT_WHY=""
  [ -n "$path" ] || { BUILD_WT_WHY="no worktree is recorded"; return 1; }
  case "$path" in /*) ;; *) BUILD_WT_WHY="the recorded worktree is not an absolute path"; return 1 ;; esac
  [ -d "$path" ] && [ ! -L "$path" ] || { BUILD_WT_WHY="the recorded worktree ${path} is not present"; return 1; }
  [ -n "$branch" ] || { BUILD_WT_WHY="the run branch is not known"; return 1; }
  real="$(cd "$path" && pwd -P)" || { BUILD_WT_WHY="the recorded worktree cannot be entered"; return 1; }
  lreal="$(cd "$launch" 2>/dev/null && pwd -P)" || lreal=""
  [ "$real" != "$lreal" ] || { BUILD_WT_WHY="the recorded worktree is the launch checkout"; return 1; }
  listed="$(build_worktree_lookup "$launch" "$branch")"
  [ -n "$listed" ] || { BUILD_WT_WHY="git lists no worktree on branch ${branch}"; return 1; }
  [ "$(cd "$listed" 2>/dev/null && pwd -P)" = "$real" ] || { BUILD_WT_WHY="the worktree git lists on branch ${branch} is not the recorded one"; return 1; }
  return 0
}

# build_own_pr: the run's own pull request, from the state (pr.json), then the
# run branch (one `gh pr list --head`), then the context pr_number. A list that
# fails is unknown, never "none": PR_LIST_FAILED tells a creator not to create.
OWN_PR=""; OWN_HOW="none"; STATE_PR=""; BRANCH_PR=""; CROSS_PR=""; PR_LIST_FAILED=false; PR_LIST_ERR=""; BUILD_CANDIDATES='[]'
build_own_pr() {
  local rec="${STATE:-}/pr.json" n out rc err base
  OWN_PR=""; OWN_HOW="none"; STATE_PR=""; BRANCH_PR=""; CROSS_PR=""; PR_LIST_FAILED=false; PR_LIST_ERR=""; BUILD_CANDIDATES='[]'
  if [ -n "${STATE:-}" ] && goal_json_file "$rec" 65536; then
    n="$(jq -r '.number // empty | tostring' "$rec")"
    if goal_is_pr "$n" && [ "$(jq -r '.branch // empty | strings' "$rec")" = "${BUILD_BRANCH:-}" ]; then STATE_PR="$n"; OWN_PR="$n"; OWN_HOW=state; fi
  fi
  if [ -n "${BUILD_BRANCH:-}" ]; then
    err="$(mktemp "${TMPDIR:-/tmp}/build-pr-list.XXXXXX")" || err=/dev/null
    out="$(gh pr list --head "$BUILD_BRANCH" --state open --limit 20 \
      --json number,url,state,isDraft,headRefName,headRefOid,baseRefName,isCrossRepository,title,body,createdAt 2>"$err")"; rc=$?
    case "$rc" in 79|75) [ "$err" = /dev/null ] || rm -f -- "$err"; return "$rc" ;; esac
    if [ "$rc" -ne 0 ] || ! printf '%s' "$out" | jq -e 'type == "array"' >/dev/null 2>&1; then
      PR_LIST_FAILED=true
      PR_LIST_ERR="gh pr list --head failed (exit ${rc}): $(build_redact_tail < "$err")"
    else
      base="${RECIPE_VAR_base_branch:-${ID_BASE:-}}"
      BUILD_CANDIDATES="$(printf '%s' "$out" | jq -c --arg b "$BUILD_BRANCH" '[.[] | objects | select(.headRefName == $b)
        | {number, url, state, draft:.isDraft, head_ref:.headRefName, head_sha:.headRefOid, base:.baseRefName,
           cross_repository:.isCrossRepository, created_at:.createdAt,
           title:((.title // "") | tostring | gsub("[[:cntrl:]]"; " ") | .[0:256]),
           body:((.body // "") | tostring | gsub("[\\x00-\\x08\\x0b\\x0c\\x0e-\\x1f\\x7f]"; "") | .[0:2000])}]')"
      n="$(printf '%s' "$BUILD_CANDIDATES" | jq -r --arg base "$base" '[.[] | select(.cross_repository == false)]
        | (map(select($base != "" and .base == $base)) + sort_by(.number)) | .[0].number // empty')"
      if goal_is_pr "$n"; then
        BRANCH_PR="$n"
        [ -n "$OWN_PR" ] || { OWN_PR="$n"; OWN_HOW=branch; }
      fi
      CROSS_PR="$(printf '%s' "$BUILD_CANDIDATES" | jq -r '[.[] | select(.cross_repository != false)] | .[0].number // empty')"
    fi
    [ "$err" = /dev/null ] || rm -f -- "$err"
  fi
  if [ -z "$OWN_PR" ] && goal_is_pr "${RECIPE_VAR_pr_number:-}"; then OWN_PR="$RECIPE_VAR_pr_number"; OWN_HOW=context; fi
  return 0
}

VERIFY_CLASS=""; VERIFY_WHY=""; VERIFY_JSON=""
build_verify_pr() {
  local n="$1" branch="$2" need_open="$3" err rc state head cross
  VERIFY_CLASS=failed; VERIFY_WHY=""; VERIFY_JSON=""
  err="$(mktemp "${TMPDIR:-/tmp}/build-pr-view.XXXXXX")" || err=/dev/null
  VERIFY_JSON="$(gh pr view "$n" --json number,url,state,isDraft,headRefName,headRefOid,baseRefName,isCrossRepository,author 2>"$err")"; rc=$?
  case "$rc" in 79|75) [ "$err" = /dev/null ] || rm -f -- "$err"; return "$rc" ;; esac
  if [ "$rc" -ne 0 ] || ! printf '%s' "$VERIFY_JSON" | jq -e 'type == "object"' >/dev/null 2>&1; then
    VERIFY_WHY="pull request #${n} could not be read (gh pr view exit ${rc}: $(build_redact_tail < "$err"))"
    VERIFY_JSON=""; [ "$err" = /dev/null ] || rm -f -- "$err"; return 0
  fi
  [ "$err" = /dev/null ] || rm -f -- "$err"
  state="$(printf '%s' "$VERIFY_JSON" | jq -r '.state // empty | strings')"
  head="$(printf '%s' "$VERIFY_JSON" | jq -r '.headRefName // empty | strings')"
  cross="$(printf '%s' "$VERIFY_JSON" | jq -r '.isCrossRepository | tostring')"
  if [ "$cross" != false ]; then
    VERIFY_CLASS=refused-cross-repo; VERIFY_WHY="pull request #${n} is from another repository (or its isCrossRepository flag is missing); a cross-repository pull request is never changed by a build"
  elif [ -z "$branch" ] || [ "$head" != "$branch" ]; then
    VERIFY_CLASS=refused; VERIFY_WHY="pull request #${n} has head $(printf '%s' "$head" | goal_clean_line 120), not the run branch ${branch:-(unknown)}"
  elif [ "$need_open" = true ] && [ "$state" != OPEN ]; then
    VERIFY_CLASS=not-open; VERIFY_WHY="pull request #${n} is ${state:-in an unknown state}, not open"
  else
    VERIFY_CLASS=ok
  fi
  return 0
}

build_path_under() {
  local path="${1:-}" root="${2:-}" rroot probe rest
  case "$path" in /*) ;; *) return 1 ;; esac
  case "/$path/" in */../*|*/./*) return 1 ;; esac
  rroot="$(cd "$root" 2>/dev/null && pwd -P)" || return 1
  probe="$path"
  while [ ! -e "$probe" ] && [ ! -L "$probe" ]; do probe="${probe%/*}"; [ -n "$probe" ] || probe=/; done
  # Every existing component from the root down is a real folder, not a link.
  [ "$(cd "$probe" 2>/dev/null && pwd -P)" = "$probe" ] || return 1
  rest="${path#"$rroot"}"
  [ "$rest" != "$path" ] && [ "${rest:0:1}" = / ] && [ "${#rest}" -gt 1 ]
}

# _build_strip_code: fenced blocks and inline code spans removed, since GitHub
# neither notifies nor closes from inside them.
_build_strip_code() {
  LC_ALL=C awk 'BEGIN{f=0} /^[ ]{0,3}(```|~~~)/{f=!f; next} !f{print}' | LC_ALL=C sed -E 's/`[^`]*`//g'
}
# _build_closing_keys <repo>: stdin -> one key per closing reference: N for
# this repository, owner/repo#N (lower case) for another.
_build_closing_keys() {
  local repo
  repo="$(printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]')"
  _build_strip_code | LC_ALL=C grep -oiE '(^|[^a-z0-9_])(close[sd]?|fix(e[sd])?|resolve[sd]?):?[[:space:]]*(([a-z0-9_.-]+/[a-z0-9_.-]+)?#[0-9]+|https?://[^[:space:]/]+/[a-z0-9_.-]+/[a-z0-9_.-]+/issues/[0-9]+)' \
    | tr '[:upper:]' '[:lower:]' | LC_ALL=C sed -E \
      -e 's#^.*https?://[^/]+/([^/]+/[^/]+)/issues/([0-9]+)$#\1\#\2#' \
      -e 's#^[^#]*[^a-z0-9_./-]([a-z0-9_.-]+/[a-z0-9_.-]+)\#([0-9]+)$#\1\#\2#' \
      -e 's#^[^#]*\#([0-9]+)$#\1#' \
    | awk -v r="$repo" '{ if (index($0, "#") && substr($0, 1, index($0, "#") - 1) == r) print substr($0, index($0, "#") + 1); else print }'
}
_build_mentions() {
  _build_strip_code | LC_ALL=C grep -oE '(^|[^A-Za-z0-9_.@`/-])@[A-Za-z0-9][A-Za-z0-9-]*(/[A-Za-z0-9._-]+)?' \
    | LC_ALL=C sed -E 's/^[^@]*//' | tr '[:upper:]' '[:lower:]'
}

build_mentions_allowed() {
  local body="$1" allow="$2" tok keys
  keys="$(_build_closing_keys "${BUILD_REPO_NAME:-}" < "$allow"; printf '%s\n' "${RECIPE_VAR_issue_number:-}" | grep -E '^[0-9]+$';
    printf '%s\n' "${RECIPE_VAR_closing_reference:-}" | LC_ALL=C grep -oE '#[0-9]+' | tr -d '#')"
  while IFS= read -r tok; do
    [ -n "$tok" ] || continue
    printf '%s\n' "$keys" | grep -qxF -- "$tok" || { printf 'closing reference %s\n' "$tok"; return 1; }
  done < <(_build_closing_keys "${BUILD_REPO_NAME:-}" < "$body")
  keys="$(_build_mentions < "$allow")"
  while IFS= read -r tok; do
    [ -n "$tok" ] || continue
    printf '%s\n' "$keys" | grep -qxF -- "$tok" || { printf 'mention %s\n' "$tok"; return 1; }
  done < <(_build_mentions < "$body")
  return 0
}

build_redact_tail() {
  tail -c 4096 | LC_ALL=C sed -E \
    -e 's#(https?://)[^/@[:space:]]*@#\1<redacted>@#g' \
    -e 's#gh[pousr]_[A-Za-z0-9_]{8,}#<redacted-token>#g' \
    -e 's#github_pat_[A-Za-z0-9_]{8,}#<redacted-token>#g' \
    -e 's#[Bb]earer[[:space:]]+[A-Za-z0-9._~+/=-]{8,}#Bearer <redacted-token>#g' \
    -e 's#(sk-|xox[baprs]-)[A-Za-z0-9_-]{8,}#<redacted-token>#g' | goal_clean_line 4096 | tail -c 200
}

build_task_text() {
  if [ -n "${AUTODRIVE_TASK_FILE:-}" ] && [ -f "$AUTODRIVE_TASK_FILE" ]; then head -c 65536 -- "$AUTODRIVE_TASK_FILE" > "$1"
  else printf '%s' "${RECIPE_VAR_task_description:-}" | head -c 65536 > "$1"; fi
}

BUILD_GUIDANCE_UNREAD=""
build_allow_text() {
  local out="$1" url="${2:-}" rc bin="${AMPLIHACK_BIN:-amplihack}"
  BUILD_GUIDANCE_UNREAD=""
  build_task_text "$out"
  # goal_notes under the shared loop's recipe; final_requirements when the
  # build step runs the driver itself.
  { printf '\n'; printf '%s\n' "${RECIPE_VAR_goal_notes:-}" | head -c 65536
    printf '%s\n' "${RECIPE_VAR_final_requirements:-}" | head -c 65536
    printf '%s\n' "${RECIPE_VAR_closing_reference:-}" | head -c 4096; } >> "$out"
  # The run's guidance folders (AMPLIHACK_RUN_GUIDANCE_DIRS) and, when the
  # recorded URL is a GitHub pull request, that pull request's folder. A URL
  # of any other shape is dropped, never passed on.
  [[ "$url" =~ ^https://github\.com/[A-Za-z0-9-]{1,39}/[A-Za-z0-9._-]{1,100}/pull/[1-9][0-9]{0,9}$ ]] || url=""
  set -- recipe guidance show --operator-only
  [ -z "$url" ] || set -- "$@" "$url"
  "$bin" "$@" > "$out.guidance" 2>/dev/null < /dev/null; rc=$?
  # Fail closed: only a read that succeeded widens the text. A binary older
  # than --operator-only exits 2 here, which counts as unread.
  if [ "$rc" -eq 0 ]; then head -c 262144 -- "$out.guidance" >> "$out"; else BUILD_GUIDANCE_UNREAD="$rc"; fi
  rm -f -- "$out.guidance"
  return 0
}
