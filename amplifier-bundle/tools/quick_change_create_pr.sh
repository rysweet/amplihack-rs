#!/usr/bin/env bash
# quick_change_create_pr.sh: quick-change's create-pr step (#1740, #3129 PR 2).
# Run by quick-change.yaml after the push; it sources quick_change_common.sh
# from its own folder with QC_STEP=quick-change-create-pr, as the inline step
# did, and reads only RECIPE_VAR_* context.
#
# The pull request text is the task's own on this route too (#3129, bb:0's
# second addition). quick-change-pr-text, a fast agent step, returns the PR
# title and body the task text gives (quick_change_pr_text, {"title","body"}),
# or an empty title when the task gives none. What this step does with it:
#
#   quick_change_pr_text                           result
#   object, title "" and body a string             the template: title qc_title
#                                                  (the task's first 72
#                                                  characters), body the report
#                                                  with qc_request (its first
#                                                  4,000 bytes)
#   object, title non-empty, every check passes    the title and body byte for
#                                                  byte; the report goes in one
#                                                  comment
#   object, title non-empty, a check fails         ERROR check=<key>, exit 1
#   missing, not JSON, not an object, or a title   ERROR check=pr-text, exit 1
#   or body that is not a string
#
# The checks, by key: title (one line of 1 to 256 bytes), body (1 to 60,000
# bytes, no NUL or control character other than tab, CR and LF; an empty body
# with a title fails here), secret-screen (goal_secret_screen on both), and
# references (every @mention and closing keyword also appears in the task
# text, closing_reference, issue_number or the operator's guidance; the
# ERROR line says so when the guidance could not be read). A failed check
# prints one line, 'ERROR: quick-change-create-pr: check=<key>: <reason>;
# nothing was created and the work goes to the build loop', creates nothing
# and leaves stdout empty, so quick-change-finish-in-loop hands the work, with
# the task's own lines, to the build loop. The template never replaces text
# the task gave.
#
# Any warning makes the pull request a draft. After create, the text is read
# back and restored once (workflow_pr_text.sh pr_text_apply); a difference
# that remains is a WARNING and one short comment, never a failure.
#
# stdout: exactly the pull request URL. Exit 0 once the pull request exists;
# 1 when it was not created, which leaves quick_change_pr_url empty. gh runs
# once, with no timeout wrapper and no retry. The allowed-text file, which can
# hold guidance, is removed on every exit. docs/reference/quick-change-recipe.md.
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

S="$QUICK_CHANGE_STATE_DIR"
TF="$S/task-title"; BF="$S/task-body"
trap 'rm -f -- "$S/allow" "$S/allow.guidance"' EXIT
# refuse <key> <reason>: the one ERROR line, then exit 1 with nothing created.
refuse() {
  echo "ERROR: quick-change-create-pr: check=$1: $2; nothing was created and the work goes to the build loop" >&2
  exit 1
}

# The task's own PR text, checked before anything else so that a refusal
# creates nothing.
write_context_value quick_change_pr_text "$S/pr-text.json"; rc=$?
[ "$rc" -eq 0 ] || refuse pr-text "quick_change_pr_text could not be read (write_context_value exit ${rc})"
jq -e 'type == "object" and (.title | type) == "string" and (.body | type) == "string"' "$S/pr-text.json" >/dev/null 2>&1 \
  || refuse pr-text "quick_change_pr_text is not a JSON object with a string title and a string body"
if [ "$(jq -r '.title | length' "$S/pr-text.json")" -gt 0 ]; then
  USE_TASK=true
  jq -j '.title' "$S/pr-text.json" > "$TF"; jq -j '.body' "$S/pr-text.json" > "$BF"
  tn="$(wc -c < "$TF" | tr -d ' ')"; bn="$(wc -c < "$BF" | tr -d ' ')"
  if [ "$tn" -gt 256 ] || [ "$(LC_ALL=C tr -d '\040-\176\200-\377' < "$TF" | wc -c | tr -d ' ')" -ne 0 ]; then
    refuse title "the title is ${tn} bytes or holds a line break or control character; it must be one line of 1 to 256 bytes"
  fi
  if [ "$bn" -lt 1 ] || [ "$bn" -gt 60000 ] || [ "$(LC_ALL=C tr -d '\011\012\015\040-\176\200-\377' < "$BF" | wc -c | tr -d ' ')" -ne 0 ]; then
    refuse body "the body is ${bn} bytes or holds a control character other than tab, CR and LF; with a title it must be 1 to 60,000 bytes of text"
  fi
  if ! goal_secret_screen "$TF" || ! goal_secret_screen "$BF"; then
    refuse secret-screen "the title or body matched the secret screen (${GOAL_SECRET_CLASS})"
  fi
  build_repo_slug "$QC_WT" || :
  build_allow_text "$S/allow"
  if ! tok="$(build_mentions_allowed "$BF" "$S/allow")" || ! tok="$(build_mentions_allowed "$TF" "$S/allow")"; then
    unread=""
    [ -z "$BUILD_GUIDANCE_UNREAD" ] || unread=" (the run's guidance could not be read: amplihack recipe guidance show exited ${BUILD_GUIDANCE_UNREAD})"
    refuse references "the text holds ${tok}, which the task text, closing_reference, issue_number and the operator's guidance do not give${unread}"
  fi
  rm -f -- "$S/allow"
else
  USE_TASK=false
  rm -f -- "$TF" "$BF"
fi

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
if [ "$USE_TASK" = false ]; then qc_title > "$TF"; cp -- "$S/pr-body.md" "$BF"; fi

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
