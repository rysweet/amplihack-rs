#!/usr/bin/env bash
# Verify that a push actually published the commit that was built and tested.
#
# Issue #1269: a workflow run moved HEAD off its own work — `git checkout` to a
# branch literally named `feature` — and then published a branch to origin that
# contained none of the run's nine commits. Nothing in the pipeline compared the
# SHA on the remote against the SHA that had passed the gates, so the run's own
# output pointed at push mechanics while the real defect was invisible without
# `git reflog`. Roughly six hours of security-hardening work survived only
# because it was recoverable.
#
# The lesson generalises past that one bad checkout: any step that moves HEAD
# between the test gates and the push produces the same silent outcome. So this
# checks the published result rather than trying to enumerate the causes.
#
# Usage:
#   verify_pushed_ref.sh <remote> <branch> <expected-sha>
#
# Exit codes:
#   0  the remote ref matches <expected-sha>
#   1  mismatch, missing ref, or a branch name that indicates the name was never
#      actually computed
#   2  usage error

set -uo pipefail

PLACEHOLDER_BRANCHES="feature feat branch HEAD head your-branch branch-name"

usage() {
  echo "usage: $(basename "$0") <remote> <branch> <expected-sha>" >&2
}

[ $# -eq 3 ] || { usage; exit 2; }

remote="$1"
branch="$2"
expected="$3"

if [ -z "$remote" ] || [ -z "$branch" ] || [ -z "$expected" ]; then
  echo "ERROR: remote, branch and expected-sha must all be non-empty" >&2
  usage
  exit 2
fi

# A computed branch name that collapses to a bare placeholder means the name was
# never computed at all. Publishing work to it is how #1269 lost its commits, so
# refuse before anyone treats the push as successful.
for bad in $PLACEHOLDER_BRANCHES; do
  if [ "$branch" = "$bad" ]; then
    echo "ERROR: refusing to verify a push to placeholder branch '$branch' (issue #1269)." >&2
    echo "  A branch name like this means a computed name was never substituted." >&2
    echo "  The work belongs on a real, issue-derived branch." >&2
    exit 1
  fi
done

if ! printf '%s' "$expected" | grep -qE '^[0-9a-f]{40}$'; then
  echo "ERROR: expected-sha '$expected' is not a full 40-character SHA" >&2
  exit 2
fi

remote_line=$(git ls-remote "$remote" "refs/heads/${branch}" 2>/dev/null)
remote_sha=$(printf '%s' "$remote_line" | awk 'NR==1 {print $1}')

if [ -z "$remote_sha" ]; then
  echo "ERROR: push reported success but ${remote}/${branch} does not exist (issue #1269)." >&2
  echo "  expected=$expected" >&2
  exit 1
fi

if [ "$remote_sha" != "$expected" ]; then
  echo "ERROR: push published the wrong commit (issue #1269)." >&2
  echo "  branch:   ${remote}/${branch}" >&2
  echo "  on remote: $remote_sha" >&2
  echo "  expected:  $expected" >&2
  echo "  The commit that passed the build and test gates is not the commit that" >&2
  echo "  was published. HEAD very likely moved after those gates ran; check" >&2
  echo "  'git reflog' in the run's worktree." >&2
  exit 1
fi

echo "INFO: verified ${remote}/${branch} is at $expected" >&2
exit 0
