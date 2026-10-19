#!/usr/bin/env bash
# Push the current branch to its remote counterpart when that is a
# fast-forward; refuse divergent history. One ancestry policy (issue #978) for
# two callers:
#
#   --mode pre-probe  `publish-sync-pr-head`, immediately before
#                     `probe-terminal-state`. Issue #1575: a run that continued
#                     an existing PR (resume_checkpoint, existing_branch,
#                     pr_number) reached the probe with unpushed commits, and
#                     the probe correctly refused the stale PR head. This mode
#                     pushes those commits first. It always exits 0 and prints
#                     nothing on stdout: on divergence or a failed push it
#                     explains loudly and leaves the probe to fail closed.
#   --mode step15     step-15-commit-push. Prints exactly one stdout line,
#                     `pushed=<true|false> sha=<oid|> reason=<text>`, and exits
#                     non-zero on divergence, a protected branch, an invalid
#                     ref, a failed push or a failed verification.
#
# Never rebases, never force-pushes, never pushes anything but
# HEAD:refs/heads/<current branch>. Remote URLs are never printed and git
# stderr is redacted. Contract: docs/reference/workflow-publish-helpers.md.

set -uo pipefail

usage() {
  echo "usage: $(basename "$0") --mode pre-probe|step15" >&2
  exit 2
}
[ $# -eq 2 ] && [ "$1" = "--mode" ] || usage
case "$2" in pre-probe | step15) mode="$2" ;; *) usage ;; esac
if [ "$mode" = step15 ]; then label="step-15"; else label="publish-sync-pr-head"; fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OID_RE='^[0-9a-f]{40}([0-9a-f]{24})?$'
COUNT_RE='^[0-9]{1,9}$'
REMOTE_RE='^[A-Za-z0-9][A-Za-z0-9._-]*$'
remote=""
remote_url=""

# Print the step15 line (step15 mode only) and exit. pre-probe always exits 0.
finish() {
  local pushed="$1" sha="$2" reason="$3" rc="$4"
  if [ "$mode" = step15 ]; then
    reason="$(printf '%s' "$reason" | tr '\n\r' '  ' | head -c 500)"
    printf 'pushed=%s sha=%s reason=%s\n' "$pushed" "$sha" "$reason"
    exit "$rc"
  fi
  exit 0
}

# Redact credentials and tokens, and replace the remote URL with its name.
scrub() {
  sed -E \
    -e 's#([A-Za-z][A-Za-z0-9+.-]*://)[^/@[:space:]]+@#\1REDACTED@#g' \
    -e 's#gh[pousr]_[A-Za-z0-9_]{8,}#REDACTED#g' \
    -e 's#github_pat_[A-Za-z0-9_]{8,}#REDACTED#g' \
    -e 's#([Bb]earer[[:space:]]+)[A-Za-z0-9._~+/=-]{8,}#\1REDACTED#g' |
    SCRUB_URL="$remote_url" SCRUB_NAME="<remote $remote>" awk '{
      u = ENVIRON["SCRUB_URL"]
      if (u != "") { while ((i = index($0, u)) > 0) $0 = substr($0, 1, i - 1) ENVIRON["SCRUB_NAME"] substr($0, i + length(u)) }
      print
    }'
}

# Run a networked git command non-interactively with a bounded wait.
net_git() {
  LC_ALL=C GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never GIT_ASKPASS='' SSH_ASKPASS='' timeout 120 git "$@"
}

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ERROR: $label: $(pwd) is not inside a git work tree; nothing to sync" >&2
  finish false "" invalid-ref 1
fi

# 1. Branch.
if ! branch="$(git symbolic-ref -q --short HEAD 2>/dev/null)"; then
  echo "INFO: $label: HEAD is detached; no branch to sync" >&2
  finish false "" detached-head 0
fi
case "$branch" in -* | "") branch_ok=false ;; *) branch_ok=true ;; esac
if [ "$branch_ok" != true ] || ! git check-ref-format --branch "$branch" >/dev/null 2>&1; then
  echo "ERROR: $label: current branch name is not a valid branch name; refusing to push" >&2
  finish false "" invalid-ref 1
fi

# 2. Protected branches, before any network call.
refuse_protected() {
  echo "ERROR: $label: refusing to push protected branch '$branch' ($1)" >&2
  finish false "" protected-branch 1
}
for protected in main master "${RECIPE_VAR_base_branch:-}" "${BASE_BRANCH:-}"; do
  [ -n "$protected" ] && [ "$branch" = "$protected" ] && refuse_protected "protected or base branch"
done

# 3. Remote.
remote="$(git config --get "branch.$branch.remote" 2>/dev/null || true)"
[ -n "$remote" ] || remote=origin
if [ "$remote" = "." ]; then
  echo "INFO: $label: branch '$branch' tracks a local branch (branch.$branch.remote=.); nothing to push" >&2
  finish false "" local-tracking 0
fi
if ! [[ "$remote" =~ $REMOTE_RE ]] || ! git remote | grep -Fxq -- "$remote"; then
  echo "ERROR: $label: branch.$branch.remote is not a configured remote name; refusing to push" >&2
  finish false "" invalid-ref 1
fi
remote_url="$(git remote get-url -- "$remote" 2>/dev/null || true)"
remote_head="$(git symbolic-ref -q --short "refs/remotes/$remote/HEAD" 2>/dev/null || true)"
[ -n "$remote_head" ] && [ "$branch" = "${remote_head#"$remote"/}" ] && refuse_protected "default branch of $remote"
upstream="$remote/$branch"

# 4. Remote commit: FETCH_HEAD after a successful fetch (SEC-3).
fetch_err="$(mktemp -t sync-pr-head-fetch-XXXXXX)" || { echo "ERROR: $label: mktemp failed" >&2; finish false "" fetch-failed 1; }
trap 'rm -f "$fetch_err" "${push_err:-}" "${verify_err:-}"' EXIT
remote_oid=""
if net_git fetch --no-tags -- "$remote" "refs/heads/$branch" >/dev/null 2>"$fetch_err"; then
  remote_oid="$(git rev-parse -q --verify 'FETCH_HEAD^{commit}' 2>/dev/null || true)"
elif grep -q "couldn't find remote ref" "$fetch_err"; then
  echo "INFO: $label: '$upstream' does not exist yet; nothing to sync" >&2
  finish false "" no-remote-branch 0
else
  echo "WARNING: $label: git fetch $remote $branch failed:" >&2
  scrub <"$fetch_err" >&2
  if [ "$mode" = pre-probe ]; then
    echo "WARNING: $label: PR head not synced; the terminal-state probe decides" >&2
    exit 0
  fi
  remote_oid="$(git rev-parse -q --verify "refs/remotes/$upstream^{commit}" 2>/dev/null || true)"
  if [ -z "$remote_oid" ]; then
    echo "INFO: $label: no remote branch or tracking ref for '$upstream'; nothing to sync" >&2
    finish false "" no-remote-branch 0
  fi
  echo "WARNING: $label: using the cached tracking ref refs/remotes/$upstream" >&2
fi
head_oid="$(git rev-parse -q --verify 'HEAD^{commit}' 2>/dev/null || true)"
if ! [[ "$remote_oid" =~ $OID_RE ]] || ! [[ "$head_oid" =~ $OID_RE ]]; then
  echo "ERROR: $label: could not resolve HEAD or '$upstream' to a commit" >&2
  finish false "" invalid-ref 1
fi

# 5. Ancestry.
ahead="$(git rev-list --count "$remote_oid..$head_oid" 2>/dev/null || true)"
behind="$(git rev-list --count "$head_oid..$remote_oid" 2>/dev/null || true)"
if ! [[ "$ahead" =~ $COUNT_RE ]] || ! [[ "$behind" =~ $COUNT_RE ]]; then
  echo "ERROR: $label: could not count commits between HEAD and '$upstream'" >&2
  finish false "" invalid-ref 1
fi
if [ "$((10#$ahead))" -eq 0 ]; then
  if [ "$((10#$behind))" -gt 0 ]; then
    echo "INFO: $label: HEAD is $behind commit(s) behind '$upstream' and has nothing to push" >&2
  else
    echo "INFO: $label: '$upstream' already matches HEAD; nothing to push" >&2
  fi
  finish true "$head_oid" already-pushed 0
fi
if [ "$((10#$behind))" -gt 0 ]; then
  merge_base="$(git merge-base "$head_oid" "$remote_oid" 2>/dev/null || true)"
  [[ "$merge_base" =~ $OID_RE ]] || merge_base=none
  echo "ERROR: $label refuses to rebase divergent history onto upstream '$upstream' (issue #978)." >&2
  echo "  ancestry: ahead=$ahead behind=$behind merge_base=$merge_base head=$head_oid upstream=$upstream remote_head=$remote_oid" >&2
  echo "  Auto-rebasing would replay already-integrated commits and create add/add conflicts." >&2
  echo "  Point the branch at its intended PR base and fast-forward, or make an explicit rebase decision." >&2
  echo "  Hint: ahead and behind together often mean a commit that was already pushed was amended locally." >&2
  echo "  Recovery, run by hand (this helper never rebases or force-pushes):" >&2
  echo "    git fetch $remote" >&2
  echo "    git log --oneline @{u}..HEAD" >&2
  echo "    git log --oneline HEAD..@{u}" >&2
  echo "    then either: git rebase @{u}" >&2
  echo "    or, if your local commits replace the remote ones: git push --force-with-lease" >&2
  finish false "" "diverged-upstream: ahead=$ahead behind=$behind merge_base=$merge_base upstream=$upstream" 1
fi

# 6. Fast-forward push with an explicit refspec, then verify.
echo "INFO: $label: '$upstream' is an ancestor of HEAD; pushing $ahead commit(s) as a fast-forward (#978)" >&2
push_err="$(mktemp -t sync-pr-head-push-XXXXXX)" || { echo "ERROR: $label: mktemp failed" >&2; finish false "" "push-failed: mktemp failed" 1; }
net_git push -- "$remote" "HEAD:refs/heads/$branch" >/dev/null 2>"$push_err"
push_rc=$?
if [ "$push_rc" -ne 0 ]; then
  sanitized="$(scrub <"$push_err" | tr '\n' ' ')"
  echo "ERROR: $label: git push to '$upstream' failed (exit $push_rc):" >&2
  scrub <"$push_err" >&2
  [ "$mode" = pre-probe ] && echo "ERROR: $label: PR head not synced; the terminal-state probe will refuse the stale PR head" >&2
  finish false "" "push-failed: $sanitized" "$push_rc"
fi

verify=""
for c in "$SCRIPT_DIR/.." "${AMPLIHACK_HOME:-}/amplifier-bundle" "${REPO_PATH:-}/amplifier-bundle" "$(pwd)/amplifier-bundle" "${HOME:-/root}/.copilot/amplifier-bundle" "${HOME:-/root}/.amplihack/amplifier-bundle"; do
  [ -f "$c/tools/verify_pushed_ref.sh" ] && { verify="$c/tools/verify_pushed_ref.sh"; break; }
done
if [ -z "$verify" ]; then
  echo "WARNING: $label: verify_pushed_ref.sh not reachable; published commit unverified (#1269)" >&2
  finish true "$head_oid" pushed 0
fi
verify_err="$(mktemp -t sync-pr-head-verify-XXXXXX)" || { echo "ERROR: $label: mktemp failed" >&2; finish false "" push-verify-failed 1; }
if LC_ALL=C GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never GIT_ASKPASS='' SSH_ASKPASS='' timeout 120 \
  bash "$verify" "$remote" "$branch" "$head_oid" 2>"$verify_err"; then
  scrub <"$verify_err" >&2
  finish true "$head_oid" pushed 0
fi
scrub <"$verify_err" >&2
echo "ERROR: $label: '$upstream' does not point at the pushed HEAD $head_oid" >&2
finish false "" push-verify-failed 1
