# shellcheck shell=bash
# workflow_pr_text.sh: read back a pull request's title and body after it is
# opened, and restore the intended text once (#3129 PR 2). Sourced by
# autodrive_build_goal.sh (open-pr) and quick_change_create_pr.sh; never run.
# It defines functions only: no top-level commands and no shell-option changes.
#
#   pr_text_norm                stdin to stdout: CRLF becomes LF, and trailing
#                               whitespace at the end of the text is removed,
#                               since GitHub normalises both
#   pr_text_same <title_a> <body_a> <title_b> <body_b>
#                               0 when both fields are equal after
#                               pr_text_norm, else 1 (four file paths)
#   pr_text_apply <pr> <title_file> <body_file>
#                               one read (`gh pr view <pr> --json title,body`),
#                               at most one `gh pr edit <pr> --title=<title>
#                               --body-file <body_file>` when the text differs,
#                               then one more read. Prints one JSON line:
#                               {"result":"same|corrected|differs|unread",
#                                "differs":[field names],"error":<text|null>}
#                               Returns 0; 79 and 75 from gh are passed up.
#
# gh chooses the repository from GH_REPO or the current folder. There is no
# loop, retry or timer: a read that fails is reported as `unread`, and the
# caller records it as evidence. Error text carries gh's exit code only, never
# its output, so no token or URL credential can reach a summary.
# docs/reference/build-goal-loop.md#pull-request-text.

pr_text_norm() {
  local s
  s="$(LC_ALL=C sed 's/\r$//')"
  s="${s%"${s##*[![:space:]]}"}"
  printf '%s' "$s"
}

pr_text_same() {
  [ "$(pr_text_norm < "$1")" = "$(pr_text_norm < "$3")" ] \
    && [ "$(pr_text_norm < "$2")" = "$(pr_text_norm < "$4")" ]
}

# _pr_text_read <pr> <dir>: the live title and body into <dir>/title and
# <dir>/body. Returns gh's exit code, or 1 when the answer is not an object.
_pr_text_read() {
  local out rc
  out="$(gh pr view "$1" --json title,body 2>/dev/null)"; rc=$?
  [ "$rc" -eq 0 ] || return "$rc"
  printf '%s' "$out" | jq -e 'type == "object" and (.title | type) == "string" and (.body | type) == "string"' >/dev/null 2>&1 || return 1
  printf '%s' "$out" | jq -j '.title' > "$2/title" && printf '%s' "$out" | jq -j '.body' > "$2/body"
}

# _pr_text_differs <title> <body> <live dir>: the names of the fields that differ.
_pr_text_differs() {
  local f=()
  [ "$(pr_text_norm < "$1")" = "$(pr_text_norm < "$3/title")" ] || f+=(title)
  [ "$(pr_text_norm < "$2")" = "$(pr_text_norm < "$3/body")" ] || f+=(body)
  printf '%s\n' ${f[@]+"${f[@]}"} | jq -R -s -c 'split("\n") | map(select(length > 0))'
}

pr_text_apply() {
  local pr="${1:-}" tf="${2:-}" bf="${3:-}" live rc differs title erc
  live="$(mktemp -d "${TMPDIR:-/tmp}/pr-text.XXXXXX")" || {
    jq -cn '{result:"unread", differs:[], error:"no scratch folder for the read-back"}'; return 0; }
  _pr_text_read "$pr" "$live"; rc=$?
  case "$rc" in 79|75) rm -rf -- "$live"; return "$rc" ;; esac
  if [ "$rc" -ne 0 ]; then
    rm -rf -- "$live"
    jq -cn --arg e "gh pr view exited ${rc}, so the text was not read back" '{result:"unread", differs:[], error:$e}'
    return 0
  fi
  differs="$(_pr_text_differs "$tf" "$bf" "$live")"
  if [ "$differs" = '[]' ]; then
    rm -rf -- "$live"; jq -cn '{result:"same", differs:[], error:null}'; return 0
  fi
  title="$(cat -- "$tf")"
  gh pr edit "$pr" "--title=${title}" --body-file "$bf" >/dev/null 2>&1; erc=$?
  case "$erc" in 79|75) rm -rf -- "$live"; return "$erc" ;; esac
  if [ "$erc" -ne 0 ]; then
    rm -rf -- "$live"
    jq -cn --argjson d "$differs" --arg e "gh pr edit exited ${erc}, so the text was not restored" '{result:"differs", differs:$d, error:$e}'
    return 0
  fi
  _pr_text_read "$pr" "$live"; rc=$?
  case "$rc" in 79|75) rm -rf -- "$live"; return "$rc" ;; esac
  if [ "$rc" -ne 0 ]; then
    rm -rf -- "$live"
    jq -cn --argjson d "$differs" --arg e "the text was edited once, and the second read (gh pr view) exited ${rc}" '{result:"unread", differs:$d, error:$e}'
    return 0
  fi
  differs="$(_pr_text_differs "$tf" "$bf" "$live")"
  rm -rf -- "$live"
  if [ "$differs" = '[]' ]; then
    jq -cn '{result:"corrected", differs:[], error:null}'
  else
    jq -cn --argjson d "$differs" '{result:"differs", differs:$d, error:"the text still differs after one edit"}'
  fi
  return 0
}
