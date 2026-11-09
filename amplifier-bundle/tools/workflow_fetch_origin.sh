#!/usr/bin/env bash
# workflow_fetch_origin.sh — fetch origin for a recipe's worktree setup step,
# retrying a transient failure.
#
# Moved out of consensus-issue-worktree.yaml step3-setup-worktree and
# workflow-worktree.yaml step-04-setup-worktree (issue #1808) so those recipes
# stay under the 400-line brick limit without being compressed; see
# scripts/check-brick-budget.sh.
#
# Usage (executed, never sourced, from inside the repository):
#   workflow_fetch_origin.sh [--strict] [refspec...]
#
# Without --strict it runs `git fetch origin --no-tags [-- refspec...]` up to
# three times, waiting 1 s and then 2 s between attempts, and always exits 0: a
# fetch that keeps failing only means the caller continues with the remote refs
# it already has, and the caller's own base-ref resolution (#858) still fails
# closed when none of them resolves.
#
# With --strict (issue #1957, decision D2) it makes exactly one attempt, never
# sleeps, and exits with git's status. On failure it prints one line,
# `WARNING: git fetch origin failed (exit N).`, so workflow_worktree_source.sh
# can refuse instead of trusting stale cached refs. Git's output goes to stderr
# in both modes.
#
# Each refspec must match ^\+refs/heads/[^:]+:refs/remotes/origin/[^:]+$ and
# is passed after `--`. A refspec that does not match, or any other argument
# starting with `-`, is a usage error (exit 2) and nothing is fetched.
#
# GIT_TRACE, GIT_TRACE_PACKET, GIT_TRACE_CURL and GIT_CURL_VERBOSE are unset
# first, so git cannot print the origin URL (which may hold a token) in a trace.
#
# A caller that cannot find this tool runs a single `git fetch origin --no-tags`
# itself.

set -u

unset GIT_TRACE GIT_TRACE_PACKET GIT_TRACE_CURL GIT_CURL_VERBOSE

strict=0
refspecs=()
refspec_re='^\+refs/heads/[^:]+:refs/remotes/origin/[^:]+$'
for arg in "$@"; do
    if [ "$arg" = "--strict" ]; then
        strict=1
    elif [[ "$arg" =~ $refspec_re ]]; then
        refspecs+=("$arg")
    else
        echo "ERROR: workflow_fetch_origin.sh: unsupported argument (expected --strict or +refs/heads/<b>:refs/remotes/origin/<b>)." >&2
        exit 2
    fi
done

fetch_args=(fetch origin --no-tags)
[ "${#refspecs[@]}" -eq 0 ] || fetch_args+=(-- "${refspecs[@]}")

# Strict mode: one attempt, git's status, no retry and no sleep (D2).
if [ "$strict" -eq 1 ]; then
    git "${fetch_args[@]}" >&2
    status=$?
    [ "$status" -ne 0 ] || exit 0
    echo "WARNING: git fetch origin failed (exit ${status})." >&2
    exit "$status"
fi

max_attempts=3
delay=1
status=0
for attempt in 1 2 3; do
    # Read git's status straight after the fetch: `$?` after an `if` whose
    # condition failed and that ran no branch is always 0.
    git "${fetch_args[@]}" >&2
    status=$?
    [ "$status" -ne 0 ] || exit 0
    if [ "$attempt" -lt "$max_attempts" ]; then
        echo "WARNING: git fetch origin failed (exit ${status}); retrying (${attempt}/${max_attempts})." >&2
        sleep "$delay"
        delay=$((delay * 2))
        continue
    fi
    echo "WARNING: git fetch origin failed after ${max_attempts} attempts (exit ${status}); continuing with local refs." >&2
done
exit 0
