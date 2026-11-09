#!/usr/bin/env bash
# autodrive_task_from_pr.sh — sourced by step-02 of autodrive-crusty-loop.yaml
# and autodrive-merge-loop.yaml, only when the task is to be taken from the PR
# (issue #2187). This file only DEFINES a function; sourcing it has no side
# effects. Kept out of autodrive_state.sh, which is at the brick budget and is
# one of the merge gate's trusted files; this one has no part in merging.

# autodrive_task_from_pr <pr> <out-file> [repo-dir]   (issue #2187)
#
# The task for a run whose operator gave pr_number but no task_description:
# "PR #N: <title>\n\n<body>" and, when the PR body links a same-repo issue with
# Closes|Fixes|Resolves|Refs #M (any case, whitespace, then #M; the first such
# link wins; owner/repo#M, URLs and GH-M are never followed), then
# "\n\nLinked issue #M: <title>\n\n<body>".
#
# The PR and issue text is UNTRUSTED: it drives an agent that holds gh write
# access. So it only ever moves between files (gh > file, jq file > file, grep
# on the file); it is never an argv element, a shell variable, or printed to
# stdout or stderr. Fork PRs are refused (isCrossRepository must be exactly
# false, the same rule as autodrive_pr_worktree.sh), and a linked issue is
# followed only when its author_association is OWNER, MEMBER or COLLABORATOR.
# Any unreadable PR or issue, wrong JSON type, or empty title-and-body fails:
# there is no fallback to a partial task. gh runs inside <repo-dir> (never with
# -R) on the operator's existing login, read-only.
#
# Writing: everything happens in a private (umask 077) temporary directory next
# to <out-file>; the task is moved over <out-file> with `mv -f` only on success
# (a symlink there is replaced, never written through), so a failure leaves
# <out-file> unchanged. The work runs in a subshell, so no trap is set (it would
# replace the caller's EXIT trap) and the temporary directory is removed once,
# on every path. On success one INFO line naming only the numbers is printed.
autodrive_task_from_pr() {
  local pr="${1:-}" out="${2:-}" repo_dir="${3:-}" t rc
  case "$pr" in ''|*[!0-9]*) echo "ERROR: auto-drive: PR number '${pr}' is not numeric; cannot take the task from it; pass task_description" >&2; return 1 ;; esac
  if [ "${#pr}" -gt 10 ]; then
    echo "ERROR: auto-drive: PR number has more than 10 digits; cannot take the task from it; pass task_description" >&2
    return 1
  fi
  if [ -z "$out" ]; then
    echo "ERROR: auto-drive: no output file for the task of PR #${pr}; pass task_description" >&2
    return 1
  fi
  t="$(umask 077; mktemp -d "$(dirname -- "$out")/.task-from-pr.XXXXXX")" || {
    echo "ERROR: auto-drive: cannot create a private work directory beside the task file for PR #${pr}; pass task_description" >&2
    return 1
  }
  (
    umask 077
    if [ -n "$repo_dir" ]; then
      cd -- "$repo_dir" 2>/dev/null || { echo "ERROR: auto-drive: cannot enter the PR's repository to read PR #${pr}; pass task_description" >&2; exit 1; }
    fi
    if ! gh pr view "$pr" --json title,body,isCrossRepository > "$t/pr.json" 2>/dev/null; then
      echo "ERROR: auto-drive: cannot read PR #${pr} to take the task from it; pass task_description" >&2
      exit 1
    fi
    if ! jq -e '(.title | type) == "string" and ((.body | type) == "string" or .body == null)' "$t/pr.json" > /dev/null 2>&1; then
      echo "ERROR: auto-drive: PR #${pr} returned an unexpected title or body; pass task_description" >&2
      exit 1
    fi
    if ! jq -e '.isCrossRepository == false' "$t/pr.json" > /dev/null 2>&1; then
      echo "ERROR: auto-drive: PR #${pr} is from a fork or its origin is unknown; its text is not used as a task; pass task_description" >&2
      exit 1
    fi
    if jq -e '.title == "" and ((.body // "") == "")' "$t/pr.json" > /dev/null 2>&1; then
      echo "ERROR: auto-drive: PR #${pr} has an empty title and body; pass task_description" >&2
      exit 1
    fi
    if ! { jq -j '.title' "$t/pr.json" | tr -d '\000' > "$t/pr-title" \
        && jq -j '.body // ""' "$t/pr.json" | tr -d '\000' > "$t/pr-body"; }; then
      echo "ERROR: auto-drive: cannot extract the text of PR #${pr}; pass task_description" >&2
      exit 1
    fi
    # First same-repo link. The character before the keyword may not be a
    # word character or '/', so prefixes, owner/repo#N and URLs never match.
    issue="$(grep -aioE '(^|[^A-Za-z0-9_/])(closes|fixes|resolves|refs)[[:space:]]+#[0-9]+' "$t/pr-body" | head -n 1 | grep -oE '[0-9]+$')"
    case "$issue" in ''|*[!0-9]*) issue="" ;; esac
    [ "${#issue}" -le 10 ] || issue=""
    {
      printf 'PR #%s: ' "$pr" && cat -- "$t/pr-title" && printf '\n\n' && cat -- "$t/pr-body"
    } > "$t/task" || { echo "ERROR: auto-drive: cannot write the task of PR #${pr}; pass task_description" >&2; exit 1; }
    if [ -n "$issue" ]; then
      assoc="$(gh api "repos/{owner}/{repo}/issues/${issue}" --jq .author_association 2>/dev/null)" || {
        echo "ERROR: auto-drive: cannot check who wrote issue #${issue}, linked from PR #${pr}; pass task_description" >&2
        exit 1
      }
      case "$assoc" in
        OWNER|MEMBER|COLLABORATOR) ;;
        *) echo "ERROR: auto-drive: issue #${issue}, linked from PR #${pr}, was not written by a repository member; its text is not used as a task; pass task_description" >&2; exit 1 ;;
      esac
      if ! gh issue view "$issue" --json title,body > "$t/issue.json" 2>/dev/null; then
        echo "ERROR: auto-drive: cannot read issue #${issue}, linked from PR #${pr}; pass task_description" >&2
        exit 1
      fi
      if ! jq -e '(.title | type) == "string" and ((.body | type) == "string" or .body == null)' "$t/issue.json" > /dev/null 2>&1; then
        echo "ERROR: auto-drive: issue #${issue}, linked from PR #${pr}, returned an unexpected title or body; pass task_description" >&2
        exit 1
      fi
      if jq -e '.title == "" and ((.body // "") == "")' "$t/issue.json" > /dev/null 2>&1; then
        echo "ERROR: auto-drive: issue #${issue}, linked from PR #${pr}, has an empty title and body; pass task_description" >&2
        exit 1
      fi
      {
        printf '\n\nLinked issue #%s: ' "$issue" \
          && jq -j '.title' "$t/issue.json" | tr -d '\000' \
          && printf '\n\n' \
          && jq -j '.body // ""' "$t/issue.json" | tr -d '\000'
      } >> "$t/task" || { echo "ERROR: auto-drive: cannot write the text of issue #${issue}, linked from PR #${pr}; pass task_description" >&2; exit 1; }
    fi
    if ! mv -f -- "$t/task" "$out"; then
      echo "ERROR: auto-drive: cannot write the task file for PR #${pr}; pass task_description" >&2
      exit 1
    fi
    if [ -n "$issue" ]; then
      echo "INFO: auto-drive: task_description is empty; task taken from PR #${pr} and issue #${issue}" >&2
    else
      echo "INFO: auto-drive: task_description is empty; task taken from PR #${pr}" >&2
    fi
  )
  rc=$?
  rm -rf -- "$t"
  return "$rc"
}
