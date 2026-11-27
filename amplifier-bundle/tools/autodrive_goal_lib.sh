# shellcheck shell=bash
# autodrive_goal_lib.sh: the shared helpers of the goal loop (#3129).
#
# Sourced by autodrive_goal_loop.sh, autodrive_goal_turn.sh and goal tools;
# never executed. It defines functions only: no top-level commands and no
# shell-option changes. Bash 3.2 safe (macOS /bin/bash).
#
# Validators (return 0 or 1, print nothing):
#   goal_is_pr <s>         ^[1-9][0-9]{0,9}$
#   goal_is_word <s>       an action or final word, ^[a-z][a-z0-9-]{0,31}$
#   goal_is_entry <s>      empty, or a guidance entry name
#   goal_is_sha <s>        40 or 64 lowercase hex characters
# Checks that print one ERROR line and return 1:
#   goal_check_turn_dir <dir>    a real 0700 folder of this user, named
#                                <state>/turns/<digits>; with AUTODRIVE_STATE_DIR
#                                set, under exactly that folder
#   goal_bundle_file <root> <rel> a regular file under <root>, never a symlink,
#                                no absolute path, no `..`, no leading `-`;
#                                prints the path
# Writers and text:
#   goal_atomic_write <path>     stdin to <path> (umask 077, temp file, mv)
#   goal_clean_text              stdin to stdout without control characters
#                                (newline and tab kept)
#   goal_clean_line <bytes>      one line: control characters become spaces,
#                                cut at <bytes>
#   goal_secret_screen <file>    0 when clean; 1 with GOAL_SECRET_CLASS set
#   goal_neutralise              stdin to stdout with @mentions in backticks
#                                and remote images replaced
#   goal_json_file <file> <max>  0 when <file> is a regular file of at most
#                                <max> bytes holding one JSON object
#   goal_turn_numbers <turns>    the turn folder names in <turns>, ascending
#   goal_last_line <log>         the last line that is neither blank nor a
#                                failure_class marker, cleaned
#   goal_run_log_line <text>     one line appended to $AMPLIHACK_RECIPE_LOG
#
# docs/reference/autodrive-goal-loop.md is the specification.

goal_is_pr() { [[ "${1:-}" =~ ^[1-9][0-9]{0,9}$ ]]; }
goal_is_word() { [[ "${1:-}" =~ ^[a-z][a-z0-9-]{0,31}$ ]]; }
goal_is_entry() {
  [ -z "${1:-}" ] || [[ "$1" =~ ^[0-9]{8}T[0-9]{6}\.[0-9]{9}Z-[0-9]+-[0-9]+\.md$ ]]
}
goal_is_sha() { [[ "${1:-}" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]; }

# The CI endpoints and guidance target must belong to the numbered PR.
goal_github_pr_repo() {
  [[ "$1" =~ ^https://github\.com/([A-Za-z0-9_-]+)/([A-Za-z0-9._-]+)/pull/([0-9]+)$ ]] &&
    [ "${BASH_REMATCH[3]}" = "$2" ] || return 1
  case "${BASH_REMATCH[2]}" in .|..) return 1 ;; esac
  printf '%s/%s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}"
}

# The driver delivers the entry from its own mark when the reading returns.
goal_read_guidance() {
  local rc=0 entry response="$GH_READ_DIR/rate-limit-guidance.json"
  gh_goal_command "${AMPLIHACK_BIN:-amplihack}" recipe guidance wait "--since=$GUIDANCE_SINCE" \
    ${GUIDANCE_TARGET:+"$GUIDANCE_TARGET"} >"$response" || rc=$?
  [ "$rc" -eq 0 ] || return "$rc"
  entry="$(goal_guidance_wait_event < "$response" | jq -r 'select(.event == "guidance") | .entry | strings')" || return 1
  [ -n "$entry" ] && goal_is_entry "$entry"
}

# The CLI can print startup diagnostics before its final JSON event.
goal_guidance_wait_event() {
  tail -n 1 | jq -ce 'objects | select(.event | type == "string")'
}

goal_ci_cache_dir() {
  local cache="$1/ci-rest"
  if [ ! -L "$cache" ] && mkdir -p -- "$cache" && goal_private_dir_ok "$cache"; then
    printf '%s\n' "$cache"
  else
    echo 'the conditional CI cache directory is unavailable' >&2
    return 1
  fi
}

# goal_private_dir_ok <dir>: a real folder of this user with mode exactly 0700.
goal_private_dir_ok() {
  [ -d "$1" ] && [ ! -L "$1" ] && [ -O "$1" ] \
    && [ -n "$(find "$1" -maxdepth 0 -perm 700 -print 2>/dev/null)" ]
}

goal_check_turn_dir() {
  local dir="${1:-}" label parent
  case "$dir" in
    /*) ;;
    *) echo "ERROR: turn folder '${dir}' is not an absolute path" >&2; return 1 ;;
  esac
  label="${dir##*/}"; parent="${dir%/*}"
  if ! [[ "$label" =~ ^[0-9]{1,9}$ ]] || [ "${parent##*/}" != turns ]; then
    echo "ERROR: ${dir} is not a turn folder (<state>/turns/<number>)" >&2; return 1
  fi
  if [ -n "${AUTODRIVE_STATE_DIR:-}" ] && [ "$parent" != "${AUTODRIVE_STATE_DIR%/}/turns" ]; then
    echo "ERROR: ${dir} is not under this run's turns folder ${AUTODRIVE_STATE_DIR%/}/turns" >&2; return 1
  fi
  if ! goal_private_dir_ok "$parent" || ! goal_private_dir_ok "$dir"; then
    echo "ERROR: ${dir} or its turns folder is a symlink, not owned by this user, or not mode 0700" >&2; return 1
  fi
  return 0
}

goal_bundle_file() {
  local root="${1:-}" rel="${2:-}"
  case "$rel" in
    ''|/*|-*|*/../*|../*|*/..|..) echo "ERROR: '${rel}' is not a relative path inside the launch bundle" >&2; return 1 ;;
  esac
  case "$root" in /*) ;; *) echo "ERROR: the launch bundle root is not absolute" >&2; return 1 ;; esac
  if [ -L "$root/$rel" ] || [ ! -f "$root/$rel" ]; then
    echo "ERROR: ${root}/${rel} is not a regular file in the launch bundle" >&2; return 1
  fi
  printf '%s\n' "$root/$rel"
}

goal_atomic_write() {
  local path="${1:-}" tmp
  [ -n "$path" ] || return 1
  tmp="$(umask 077 && mktemp "${path%/*}/.${path##*/}.XXXXXX")" || return 1
  if ! cat > "$tmp"; then rm -f -- "$tmp"; return 1; fi
  mv -f -- "$tmp" "$path" || { rm -f -- "$tmp"; return 1; }
}

goal_write_ci_result() {
  jq -c --arg head "$2" --arg base "$3" --arg base_sha "$4" '
    . as $ci | {outcome:"checks-complete", checks_state:.state, head:$head, base:$base, base_sha:$base_sha, summary:(
      if .state == "passed" then "the checks at the head completed and passed"
      elif .state == "failed" then "the checks at the head completed and at least one failed"
      else "the checks at the head completed with unverified results" end)}
    + (if (.unverified_checks | length) > 0 then {unverified_checks} else {} end)
    | if $ci.execution_detail then
        . + ($ci | {execution_detail}) | .summary += ". " + .execution_detail
      else . end
    | if $ci.required_error then
        . + ($ci | {counts, required_error})
        | .summary += ". The required-check policy is incomplete: " + .required_error
      else . end' "$1" \
    | goal_atomic_write "$5"
}

goal_clean_text() { LC_ALL=C tr -d '\000-\010\013-\037\177'; }

goal_clean_line() {
  LC_ALL=C tr '\000-\037\177' ' ' | head -c "${1:-500}" | LC_ALL=C sed -e 's/[[:space:]]*$//'
}

# goal_json_file <file> <max_bytes>
goal_json_file() {
  local f="${1:-}" max="${2:-65536}" size
  [ -f "$f" ] && [ ! -L "$f" ] || return 1
  size="$(wc -c < "$f" | tr -d '[:space:]')" || return 1
  [ "${size:-0}" -le "$max" ] || return 1
  jq -e 'type == "object"' "$f" >/dev/null 2>&1
}

goal_turn_numbers() {
  find "${1:-}" -mindepth 1 -maxdepth 1 -type d -name '[0-9]*' 2>/dev/null | sed 's#.*/##' \
    | grep -E '^[0-9]{1,9}$' | sort -n
}

# goal_last_line <log>: the log's last line that is neither blank nor a
# runner failure_class marker, as one clean line. Nothing is matched against
# its wording: the loop only compares one turn's line with the next (#3286).
goal_last_line() {
  [ -f "${1:-}" ] || return 0
  LC_ALL=C awk '!/^amplihack\.recipe\.failure_class / && /[^[:space:]]/ { last = $0 } END { print last }' "$1" 2>/dev/null \
    | goal_clean_line 300
}

# goal_run_log_line <text>: one line appended to the run's own log, named by
# AMPLIHACK_RECIPE_LOG, when that is an absolute path to a regular file of
# this user that is not a symlink. Best effort and silent: a log that cannot
# be written never changes what the loop does (#3286).
goal_run_log_line() {
  local log="${AMPLIHACK_RECIPE_LOG:-}"
  case "$log" in /*) ;; *) return 0 ;; esac
  [ -f "$log" ] && [ ! -L "$log" ] && [ -O "$log" ] || return 0
  printf '%s\n' "$(printf '%s' "${1:-}" | goal_clean_line 2000)" >> "$log" 2>/dev/null || :
}

# goal_secret_screen <file>: token prefixes, AWS keys, PEM keys, Authorization
# and Bearer values, and the values of this process's environment variables
# whose names hold TOKEN, SECRET or KEY. The values reach grep through a pipe
# (process substitution), never argv or a file.
goal_secret_screen() {
  local f="${1:-}" name
  GOAL_SECRET_CLASS=""
  [ -f "$f" ] || return 0
  if LC_ALL=C grep -Eq 'gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}' "$f"; then GOAL_SECRET_CLASS="GitHub token"
  elif LC_ALL=C grep -Eq 'AKIA[0-9A-Z]{16}' "$f"; then GOAL_SECRET_CLASS="AWS access key"
  elif LC_ALL=C grep -Eq -- '-----BEGIN [A-Z ]*PRIVATE KEY-----' "$f"; then GOAL_SECRET_CLASS="private key"
  elif LC_ALL=C grep -Eqi 'authorization[[:space:]]*:[[:space:]]*(bearer|basic|token)[[:space:]]+[^[:space:]]{8,}' "$f"; then GOAL_SECRET_CLASS="Authorization header"
  elif LC_ALL=C grep -Eqi 'bearer[[:space:]]+[A-Za-z0-9._~+/=-]{16,}' "$f"; then GOAL_SECRET_CLASS="Bearer token"
  elif LC_ALL=C grep -Eq 'sk-[A-Za-z0-9_-]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}' "$f"; then GOAL_SECRET_CLASS="API key"
  elif LC_ALL=C grep -Fq -f <(
      for name in $(compgen -e); do
        case "$name" in
          RECIPE_VAR_*|*_PATH|*_FILE|*_DIR|*_DIRS) ;;
          *TOKEN*|*SECRET*|*KEY*) printf '%s\n' "${!name}" | awk 'length($0) >= 8' ;;
        esac
      done
      printf '%s\n' 'amplihack-goal-loop-no-secret-placeholder-3129') "$f"; then
    GOAL_SECRET_CLASS="environment secret"
  fi
  [ -z "$GOAL_SECRET_CLASS" ]
}

# goal_neutralise: @mentions go in backticks so nobody is notified, and
# markdown or HTML images are replaced, so published text loads nothing.
goal_neutralise() {
  LC_ALL=C sed -E \
    -e 's/!\[[^]]*\]\([^)]*\)/[image removed]/g' \
    -e 's/<[Ii][Mm][Gg][^>]*>/[image removed]/g' \
    -e 's/(^|[^`A-Za-z0-9_])@([A-Za-z0-9][A-Za-z0-9-]*(\/[A-Za-z0-9._-]+)?)/\1`@\2`/g'
}
