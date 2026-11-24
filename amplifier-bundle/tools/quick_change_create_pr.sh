#!/usr/bin/env bash
# quick_change_create_pr.sh: quick-change's create-pr step (#1740, #3129 PR 2).
# Run by quick-change.yaml after the push; it sources quick_change_common.sh
# from its own folder with QC_STEP=quick-change-create-pr, as the inline step
# did, and reads only RECIPE_VAR_* context.
#
# The pull request text is the task's own on this route too (#3129, bb:0's
# second addition). quick-change-pr-text, a fast agent step, returns the PR
# title and body the task text gives (quick_change_pr_text, {"title","body"}),
# or an empty title when the task gives none.
#   - Task-text path: the title is 1 to 256 bytes on one line, the body 1 to
#     60,000 bytes without NUL or control characters other than tab, CR and LF,
#     both pass the secret screen, and every @mention and closing keyword in
#     them also appears in the task text, issue_number or closing_reference.
#     The pull request is created with those bytes, and the template report
#     (Request, Diff, Checks and any warning) goes in one comment.
#   - Template path, otherwise: the title is qc_title (the task's first 72
#     characters) and the body is the template report with qc_request (the
#     task's first 4,000 bytes), as before.
# Any warning makes the pull request a draft. After create, the text is read
# back and restored once (workflow_pr_text.sh pr_text_apply); a difference
# that remains is a WARNING and one short comment, never a failure.
#
# stdout: exactly the pull request URL. Exit 0 once the pull request exists;
# 1 when it could not be created, which leaves quick_change_pr_url empty, so
# quick-change-finish-in-loop hands the work to the build loop. gh runs once,
# with no timeout wrapper and no retry. docs/reference/quick-change-recipe.md.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 1
# shellcheck disable=SC2034  # read by quick_change_common.sh, sourced next
QC_STEP=quick-change-create-pr
# shellcheck source=quick_change_common.sh
. "$HERE/quick_change_common.sh"
set +e
for lib in autodrive_build_lib.sh workflow_pr_text.sh recipe_context_value.sh; do
  [ -f "$QC_TOOLS/$lib" ] || qc_die "$lib is missing from $QC_TOOLS"
  # shellcheck source=/dev/null
  . "$QC_TOOLS/$lib" || qc_die "$lib could not be loaded"
done
exec 3>&1 1>&2
git -C "$QC_WT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || qc_die "requires a git repo at $QC_WT; either \`git init\` or rerun from a checkout"
cd "$QC_WT" || qc_die "cannot enter $QC_WT"
BRANCH="$(qc_branch)" || exit 1
BASE="$(qc_base)" || exit 1
CHECKS="$(qc_checks_summary)" || exit 1
DEFAULT="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
DEFAULT="${DEFAULT#origin/}"; [ -n "$DEFAULT" ] || DEFAULT=main
ISSUE="$(qc_issue)"
# Runner outputs, not state files: the agent and check code can write to the
# state dir, so the checks summary and table are checked against them.
REPAIRED=no
for r in "${RECIPE_VAR_quick_change_commit_repaired:-}" "${RECIPE_VAR_quick_change_push_repaired:-}"; do
  case "$r" in
    yes) REPAIRED=yes ;;
    no) ;;
    *) qc_die "runner did not provide the hook-repair outputs" ;;
  esac
done
WARNINGS="$(qc_pr_warnings "$CHECKS" "$REPAIRED")"
DRAFT=(); [ -z "$WARNINGS" ] || DRAFT=(--draft)
{
  [ -z "$WARNINGS" ] || printf '%s\n\n' "$WARNINGS"
  printf '## Request\n\n%s\n' "$(qc_request)"
  [ -z "$ISSUE" ] || printf '\nRefs #%s\n' "$ISSUE"
  printf '\n## Diff\n\n```\n%s\n```\n' "$(git diff --stat "$BASE" HEAD)"
  printf '\n## Checks\n\n| Command | Result |\n| --- | --- |\n'
  qc_checks_table "$CHECKS"
} | qc_write pr-body.md

# The task's own text, when quick-change-pr-text found it and it passes.
S="$QUICK_CHANGE_STATE_DIR"
TF="$S/task-title"; BF="$S/task-body"; WHY=""
write_context_value quick_change_pr_text "$S/pr-text.json" 2>/dev/null || : > "$S/pr-text.json"
if jq -e 'type == "object" and (.title | type) == "string" and (.title | length) > 0 and (.body | type) == "string"' "$S/pr-text.json" >/dev/null 2>&1; then
  jq -j '.title' "$S/pr-text.json" > "$TF"; jq -j '.body' "$S/pr-text.json" > "$BF"
  tn="$(wc -c < "$TF" | tr -d ' ')"; bn="$(wc -c < "$BF" | tr -d ' ')"
  if [ "$tn" -gt 256 ] || [ "$(LC_ALL=C tr -d '\040-\176\200-\377' < "$TF" | wc -c | tr -d ' ')" -ne 0 ]; then WHY="the title is not one line of 1 to 256 bytes"
  elif [ "$bn" -lt 1 ] || [ "$bn" -gt 60000 ] || [ "$(LC_ALL=C tr -d '\011\012\015\040-\176\200-\377' < "$BF" | wc -c | tr -d ' ')" -ne 0 ]; then WHY="the body is not 1 to 60,000 bytes of text"
  elif ! goal_secret_screen "$TF" || ! goal_secret_screen "$BF"; then WHY="the text matched the secret screen (${GOAL_SECRET_CLASS})"
  else
    build_repo_slug "$QC_WT" || :
    build_allow_text "$S/allow"
    if ! tok="$(build_mentions_allowed "$BF" "$S/allow")" || ! tok="$(build_mentions_allowed "$TF" "$S/allow")"; then WHY="the text holds ${tok}, which the task text does not give"; fi
  fi
  [ -z "$WHY" ] || echo "WARNING: quick-change: the task's PR text was not used (${WHY}); the template text is used instead" >&2
else
  WHY="the task gives no PR title"
  rm -f -- "$TF" "$BF"
fi
if [ -z "$WHY" ]; then USE_TASK=true; else USE_TASK=false; qc_title > "$TF"; cp -- "$S/pr-body.md" "$BF"; fi

OUT="$(gh pr create --base "$DEFAULT" --head "$BRANCH" "--title=$(cat "$TF")" --body-file "$BF" ${DRAFT[@]+"${DRAFT[@]}"} 2>"$S/create.err")"; rc=$?
if [ "$rc" -ne 0 ]; then
  echo "ERROR: quick-change-create-pr: gh pr create exited ${rc}: $(build_redact_tail < "$S/create.err"); the work goes to the build loop" >&2
  exit 1
fi
URL="$(printf '%s\n' "$OUT" | grep -oE 'https://[^[:space:]]+/pull/[0-9]+' | tail -n 1)"
[ -n "$URL" ] || { echo "ERROR: quick-change-create-pr: gh pr create printed no pull request URL; the work goes to the build loop" >&2; exit 1; }
echo "quick-change: opened ${URL} with the $([ "$USE_TASK" = true ] && echo "task's own" || echo template) text" >&2
if [ "$USE_TASK" = true ]; then
  gh pr comment "$URL" --body-file "$S/pr-body.md" >/dev/null 2>"$S/comment.err" \
    || echo "WARNING: quick-change: the report comment could not be posted: $(build_redact_tail < "$S/comment.err")" >&2
fi
TEXT="$(pr_text_apply "$URL" "$TF" "$BF")"
RES="$(printf '%s' "$TEXT" | jq -r '.result // "unread"' 2>/dev/null)"
echo "quick-change: the pull request text read back ${RES:-unread}" >&2
if [ "$RES" = differs ]; then
  echo "WARNING: quick-change: the pull request text still differs after one correction ($(printf '%s' "$TEXT" | jq -r '.differs | join(", ")'))" >&2
  printf 'The pull request %s still differs from the intended text after one correction. The intended text is the %s.\n' \
    "$(printf '%s' "$TEXT" | jq -r '.differs | join(" and ")')" "$([ "$USE_TASK" = true ] && echo "task's own" || echo "quick-change template")" > "$S/differs.md"
  gh pr comment "$URL" --body-file "$S/differs.md" >/dev/null 2>&1 || echo "WARNING: quick-change: the difference could not be posted as a comment" >&2
fi
printf '%s\n' "$URL" >&3
qc_remove_state
exit 0
