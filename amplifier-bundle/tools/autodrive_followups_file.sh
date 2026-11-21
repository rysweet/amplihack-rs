# shellcheck shell=bash
# autodrive_followups_file.sh - the `file` command of autodrive_followups.sh
# (issue #1979), and the parent-task comment it posts. Sourced by
# autodrive_followups.sh after its arguments are parsed and its helpers are
# defined; never run on its own. It uses STATE, PR, TASK_ID, TASK_FILE, HERE,
# PRIV, TSV, SET_FILE, ERR_FILE, LABEL, LABEL_COLOUR, LABEL_DESC, ID_RE and
# JQ_MISSING, the helpers state_ok, safe_file, write_state, norm_set, rows_ok,
# filed_ids, parent_key, parent_commented, resolve_task, closing_ref, gh_line,
# priv_dir and fail from there, and cell(), break_refs() and issue_title()
# from autodrive_finding_rules.sh.
#
# file  One `gh issue create --repo <owner/repo of the PR> --label follow-up`
#       per finding id with no row in followups.tsv, after one
#       `gh label create` for the label itself; the row (id, issue, task, PR)
#       is appended straight after each create. The title is issue_title() of
#       the finding summary: the finding in plain words, no task id, no
#       truncation marker. The body carries the task, the PR, the finding and
#       the last crusty round recorded. When the task is an issue (#N), one
#       `gh issue comment` on it then lists every follow-up, because merging
#       the PR closes that issue while the follow-ups stay open;
#       followups.parent records the list posted, and a changed list is posted
#       again. On failure: followups.error, exit 1 (gh's 79 is final and
#       passed through); a full run removes it.

# post_parent: one comment on the task issue listing every follow-up, posted
# again only when the list changed; followups.parent records the list posted.
post_parent() {
  local key num tf rc
  key="$(parent_key)"
  if [ -z "$key" ]; then
    [ ! -s "$TSV" ] || echo "INFO: followups: the task is not an issue number; no comment lists the follow-ups" >&2
    return 0
  fi
  parent_commented && return 0
  num="${key%%:*}"; tf="$PRIV/parent.md"
  { printf 'Auto-drive filed follow-up issues for the minor crusty findings on PR #%s.\n' "$PR"
    printf 'Merging that PR closes this issue. The task is not complete until these are done:\n\n'
    LC_ALL=C awk -F '\t' 'NF { printf "- #%s (finding `%s`)\n", $2, $1 }' "$TSV"
    printf '\n<!-- autodrive-followups-parent pr=%s -->\n' "$PR"
  } > "$tf" || fail followup-comment-failed "the comment body for #${num} could not be written"
  gh issue comment "$num" --repo "$repo" --body-file "$tf" < /dev/null > /dev/null 2>"$PRIV/gh.err"; rc=$?
  [ "$rc" -ne 79 ] || fail followup-comment-failed "gh issue comment on #${num} exited 79; that code is final and is not retried" 79
  [ "$rc" -eq 0 ] || fail followup-comment-failed "gh issue comment on #${num} exited ${rc}: $(gh_line "$PRIV/gh.err")"
  printf '%s\n' "$key" | write_state followups.parent \
    || fail followup-state-unsafe "the comment on #${num} was posted but followups.parent could not be written; a rerun posts it again"
  echo "INFO: followups: the follow-up list is posted on task #${num}" >&2
}

cmd_file() {
  local fset filed n task="" prj url body repo out rc num re title id summary evidence created=0 tf
  local round="not recorded" mrow recfile lab
  [ -z "${JQ_MISSING:-}" ] || { echo "ERROR: followup-create-failed: jq is not on PATH" >&2; exit 1; }
  state_ok || { echo "ERROR: followup-state-unsafe: the state dir is not private to this user" >&2; exit 1; }
  for n in followups.jsonl followups.tsv followups.error followups.parent; do
    safe_file "$STATE/$n" || fail followup-state-unsafe "$n is a symlink, not a regular file, or not private"
  done
  fset="$(norm_set "$SET_FILE")" || fail followup-set-invalid "followups.jsonl has a line that is not a valid finding"
  rows_ok || fail followup-row-malformed "followups.tsv has a row that is not id, issue, task, PR #${PR}"
  filed="$(filed_ids)"
  n="$(printf '%s' "$fset" | jq --argjson d "$filed" '[.[] | select(.id as $i | $d | index([$i]) | not)] | length')"
  if [ "$n" = 0 ] && parent_commented; then
    rm -f -- "$ERR_FILE"
    echo "INFO: followups: every minor finding has an issue ($(printf '%s' "$fset" | jq length) in the set)" >&2
    printf 'followups-ok %s\n' "$(printf '%s' "$fset" | jq length)"
    exit 0
  fi
  if [ "$n" != 0 ] && [ -n "$TASK_ID" ]; then
    task="$(resolve_task "$TASK_ID")" || fail followup-task-unresolved "the plan task id is neither #N nor 1 to 64 of A-Za-z0-9._:/-"
  fi
  . "$HERE/workflow_issue_tracking.sh" 2>/dev/null && declare -F sanitize_cli_output >/dev/null \
    || fail followup-create-failed "workflow_issue_tracking.sh is missing beside autodrive_followups.sh"
  priv_dir || fail followup-create-failed "no private temporary directory"
  prj="$(gh pr view "$PR" --json url,body < /dev/null 2>"$PRIV/gh.err")"; rc=$?
  [ "$rc" -ne 79 ] || fail followup-repo-unresolved "gh exited 79 reading PR #${PR}; that code is final and is not retried" 79
  [ "$rc" -eq 0 ] || fail followup-repo-unresolved "gh pr view ${PR} exited ${rc}: $(gh_line "$PRIV/gh.err")"
  url="$(printf '%s' "$prj" | jq -r '.url // "" | strings' 2>/dev/null)"
  re="^https://github\\.com/([A-Za-z0-9-]+/[A-Za-z0-9._-]+)/pull/${PR}\$"
  [[ "$url" =~ $re ]] || fail followup-repo-unresolved "the URL of PR #${PR} is not https://github.com/<owner>/<repo>/pull/${PR}"
  repo="${BASH_REMATCH[1]}"
  if [ "$n" != 0 ] && [ -z "$task" ]; then
    body="$(printf '%s' "$prj" | jq -r '.body // "" | strings' 2>/dev/null)"
    num="$(printf '%s' "$body" | closing_ref)"
    if [ -z "$num" ] && [ -n "$TASK_FILE" ] && [ -f "$TASK_FILE" ]; then num="$(closing_ref < "$TASK_FILE")"; fi
    [ -n "$num" ] && task="$(resolve_task "$num")" \
      || fail followup-task-unresolved "no plan task id, and neither the PR body nor the task names Closes/Fixes/Resolves #N"
  fi
  if [ "$n" != 0 ]; then
    echo "INFO: followups: filing ${n} follow-up issue(s) in ${repo} for task ${task}, PR #${PR}" >&2
    # The label, once per run and before the first issue (#2915). Advisory: a
    # refusal is logged with gh's own message and the drive goes on, because
    # every create below carries --label and reports a refusal of its own.
    if gh label create "$LABEL" --repo "$repo" --force --color "$LABEL_COLOUR" \
         --description "$LABEL_DESC" < /dev/null > /dev/null 2>"$PRIV/gh.err"
    then echo "INFO: followups: the ${LABEL} label is present in ${repo}" >&2
    else echo "INFO: followups: gh label create ${LABEL} in ${repo} did not succeed: $(gh_line "$PRIV/gh.err"); the creates carry --label and report any refusal" >&2
    fi
    # The round standing when the follow-ups were filed, for the body; the set
    # spans rounds, so it is not per finding. Anything unreadable leaves "not
    # recorded" rather than ending the drive, and the record is not hashed
    # again, because collect already bound it to the manifest.
    mrow="$(autodrive_crusty_manifest_row "$STATE")" || mrow=""
    recfile="${mrow%% *}"
    if [ -n "$recfile" ] && [ ! -L "$STATE/$recfile" ] && [ -f "$STATE/$recfile" ]; then
      lab="$(LC_ALL=C sed -n '1s/.*"round_label"[[:space:]]*:[[:space:]]*"\([A-Za-z0-9._-]\{1,64\}\)".*/\1/p' -- "$STATE/$recfile" 2>/dev/null)"
      [ -z "$lab" ] || round="$lab"
    fi
  fi
  tf="$PRIV/body.md"
  while IFS=$'\x1f' read -r id summary evidence; do
    [[ "$id" =~ $ID_RE ]] || fail followup-set-invalid "a finding id is not valid"
    # The finding in plain words, with no task id: the fallback keeps a finding
    # whose summary is empty from reaching gh as an empty title (#2915).
    title="$(issue_title "$summary" "Minor crusty finding ${id}")"
    { printf 'Follow-up from the auto-drive crusty review of PR #%s.\n\n' "$PR"
      printf 'Task: %s\nFinding: %s (severity: minor)\nCrusty round: %s\n\n' "$task" "$id" "$round"
      printf 'Summary: %s\nEvidence: %s\n\n' "$(cell "$summary")" "$(cell "$evidence")"
      printf '<!-- autodrive-followup task=%s finding=%s -->\n' "$task" "$id"
    } > "$tf" || fail followup-create-failed "the issue body for ${id} could not be written"
    out="$(gh issue create --repo "$repo" --title "$title" --label "$LABEL" --body-file "$tf" < /dev/null 2>"$PRIV/gh.err")"; rc=$?
    [ "$rc" -ne 79 ] || fail followup-create-failed "gh issue create for ${id} exited 79; that code is final and is not retried" 79
    [ "$rc" -eq 0 ] || fail followup-create-failed "gh issue create for ${id} exited ${rc}: $(gh_line "$PRIV/gh.err")"
    out="${out%$'\n'}"; out="${out##*$'\n'}"
    re="^https://github\\.com/${repo//./\\.}/issues/([1-9][0-9]{0,9})\$"
    [[ "$out" =~ $re ]] || fail followup-create-failed "gh issue create for ${id} printed no issue URL of ${repo}"
    num="${BASH_REMATCH[1]}"
    { safe_file "$TSV" && printf '%s\t%s\t%s\t%s\n' "$id" "$num" "$task" "$PR" >> "$TSV"; } \
      || fail followup-state-unsafe "issue #${num} was created for ${id} but its followups.tsv row could not be written; a rerun files it again"
    created=$((created + 1))
    echo "INFO: followup ${id} -> #${num} (task ${task}, PR #${PR})" >&2
  done < <(printf '%s' "$fset" | jq -r --argjson d "$filed" \
             '.[] | select(.id as $i | $d | index([$i]) | not) | [.id, .summary, .evidence] | join("\u001f")')
  [ "$created" = "$n" ] || fail followup-create-failed "${created} of ${n} follow-up issue(s) were filed"
  post_parent
  rm -f -- "$ERR_FILE"
  printf 'followups-ok %s\n' "$(printf '%s' "$fset" | jq length)"
}
