#!/usr/bin/env bash
set -uo pipefail
cd "${REPO_PATH:-.}" 2>/dev/null || { echo "ERROR: cannot cd to REPO_PATH" >&2; exit 1; }
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
PR="${PR_NUMBER:-}"
if [ -z "$PR" ]; then
  PR="$(gh pr view --json number --jq .number 2>/dev/null || printf '')"
fi
case "$PR" in ''|*[!0-9]*) PR="" ;; esac
hex() { case "${#1}" in 40|64) ;; *) return 1 ;; esac; case "$1" in *[!0123456789abcdef]*) return 1 ;; esac; }
SHA=""
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  SHA="$(git rev-parse HEAD 2>/dev/null || printf '')"
else
  echo "[skip] not a git repo; the round baseline is unobserved, never assumed" >&2
fi
# Crusty reads `gh pr diff`, which is the PR's head on GitHub, so the
# reviewed commit is headRefOid, and step-04 commits on top of the local
# checkout. The two must be the same commit. A checkout on another
# commit, or a push that has not reached GitHub (#1519), fails the round
# by name. Otherwise a verdict would be recorded against a commit crusty
# never read. Criterion 3 of the merge round depends on that record.
if [ -n "$PR" ]; then
  REMOTE="$(gh pr view "$PR" --json headRefOid --jq .headRefOid 2>/dev/null || printf '')"
  hex "$REMOTE" || { echo "ERROR: crusty-pr-head-unreadable: the head commit of PR #${PR} could not be read; no round is run without the commit crusty will review." >&2; exit 1; }
  if [ "$SHA" != "$REMOTE" ]; then
    WHY="the checkout at REPO_PATH is on another commit"
    [ -n "$SHA" ] && git merge-base --is-ancestor "$REMOTE" "$SHA" 2>/dev/null && WHY="local commits have not reached GitHub; push them, then run the round again"
    echo "ERROR: crusty-local-head-not-pr-head: local HEAD ${SHA:-<none>} is not PR #${PR}'s head ${REMOTE}: ${WHY}. The round is not run, so no verdict is recorded against a commit crusty did not read." >&2
    exit 1
  fi
  SHA="$REMOTE"
fi
RESOLVED=""
if [ -n "${AUTODRIVE_RESOLVED_CONCERNS_FILE:-}" ] && [ -f "${AUTODRIVE_RESOLVED_CONCERNS_FILE}" ]; then
  RESOLVED="$(tr '\n' ' ' < "${AUTODRIVE_RESOLVED_CONCERNS_FILE}")"
fi
if [ -z "$PR" ]; then
  echo "WARNING: no pull request could be resolved; crusty will review the working diff instead." >&2
fi
printf '{"pr":"%s","head_sha":"%s","resolved_concerns":"%s","round_label":"%s"}\n' \
  "$PR" "$SHA" "$RESOLVED" "${AUTODRIVE_ROUND_LABEL:-round}"
