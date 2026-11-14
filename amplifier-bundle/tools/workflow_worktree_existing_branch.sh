#!/usr/bin/env bash
# Exact quick-change continuation. Validate origin before checkout selection;
# never publish, sweep, prune, reset, stash or create an alias here.
# Usage: <caller checkout> <main repository> <worktree base> <branch>
set -euo pipefail
export GIT_PAGER=cat PAGER=cat LESS=FRX GIT_TERMINAL_PROMPT=0

die() { echo "ERROR: existing-branch setup: $*" >&2; exit 1; }
[ "$#" -eq 4 ] || die "expected caller, main repository, worktree base and branch"
CALLER="$1"; MAIN="$2"; WT_BASE="$3"; BRANCH="$4"
TOOLS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
cd "$CALLER"
git check-ref-format --branch "$BRANCH" >/dev/null 2>&1 || die "invalid branch '$BRANCH'"
# check-ref-format --branch also accepts checkout shorthand such as @{-1};
# the supplied value must itself be a literal heads ref.
git check-ref-format "refs/heads/$BRANCH" >/dev/null 2>&1 || die "invalid branch '$BRANCH'"
DEFAULT="$(git symbolic-ref -q refs/remotes/origin/HEAD || true)"
case "$BRANCH" in
  main|master|develop|trunk|"${DEFAULT#refs/remotes/origin/}") die "refusing protected branch '$BRANCH'" ;;
esac

REF="refs/heads/$BRANCH"; RC=0
REMOTE="$(git ls-remote --exit-code --symref origin HEAD "$REF")" || RC=$?
case "$RC" in
  0) ;;
  2) die "branch '$BRANCH' does not exist on origin" ;;
  *) die "cannot access origin to validate branch '$BRANCH' (git ls-remote exit $RC)" ;;
esac
# Read the authoritative default in the same lookup, even when the local
# origin/HEAD is absent or stale. It must be refused before any fetch.
REMOTE_DEFAULT="$(printf '%s\n' "$REMOTE" | awk '$1 == "ref:" && $3 == "HEAD" { print $2 }')"
[ "$REF" != "$REMOTE_DEFAULT" ] || die "refusing origin default branch '$BRANCH'"
# ls-remote patterns can match a suffix: accept only the exact ref, once.
SHA="$(printf '%s\n' "$REMOTE" | awk -v ref="$REF" '$2 == ref { n++; sha=$1 } END { if (n == 1) print sha }')"
[[ "$SHA" =~ ^[0-9a-f]{40}$ ]] || die "branch '$BRANCH' does not exist on origin as an exact heads ref"
# Ignore configured mappings, which may target (and force-update) local heads.
git fetch --no-tags --refmap= origin "$REF:refs/remotes/origin/$BRANCH" >&2 \
  || die "cannot fetch branch '$BRANCH' from origin; local history retained"

# NUL-delimited porcelain preserves spaces, backslashes and newlines in paths.
REGISTRATIONS="$(mktemp)" || die "cannot allocate worktree listing"
trap 'rm -f "$REGISTRATIONS"' EXIT
git worktree list --porcelain -z > "$REGISTRATIONS" || die "cannot list worktrees for '$BRANCH'"
WT=""; REGISTERED=""; CREATED=false
while IFS= read -r -d '' FIELD; do
  case "$FIELD" in
    'worktree '*) REGISTERED="${FIELD#worktree }" ;;
    "branch $REF") [ -z "$WT" ] || die "multiple checkouts registered for '$BRANCH'"; WT="$REGISTERED" ;;
  esac
done < "$REGISTRATIONS"
if [ -n "$WT" ]; then
  WT="$(cd "$WT" 2>/dev/null && pwd -P)" \
    || die "registered checkout for '$BRANCH' is inaccessible; inspect its registration before retrying"
elif [ "$(git symbolic-ref -q HEAD || true)" = "$REF" ]; then
  WT="$(pwd -P)"
else
  WT="$WT_BASE/$BRANCH"
  [ ! -e "$WT" ] && [ ! -L "$WT" ] || die "destination '$WT' for '$BRANCH' already exists; preserve it before retrying"
  bash "$TOOLS/workflow_worktree_root.sh" assert-not-nested "$WT" "$MAIN" || exit 1
  if git show-ref --verify --quiet "$REF"; then
    git worktree add -- "$WT" "$BRANCH" >&2 || die "cannot attach local branch '$BRANCH'"
  else
    git worktree add --track -b "$BRANCH" -- "$WT" "refs/remotes/origin/$BRANCH" >&2 \
      || die "cannot check out remote branch '$BRANCH'"
  fi
  CREATED=true
  WT="$(cd "$WT" && pwd -P)" || die "cannot resolve checkout for '$BRANCH'"
fi
# Common-directory output can be relative to the checkout. Resolve both paths
# physically so linked worktrees match, but unrelated repos at stale paths do not.
common_dir() {
  local checkout="$1" common
  common="$(git -C "$checkout" rev-parse --git-common-dir)" || return 1
  case "$common" in
    /*) ;;
    *) common="$checkout/$common" ;;
  esac
  (cd "$common" && pwd -P)
}
CALLER_COMMON="$(common_dir "$(pwd -P)")" || die "cannot resolve caller's Git common directory"
WT_COMMON="$(common_dir "$WT")" || die "cannot resolve Git common directory for checkout '$WT'"
[ "$CALLER_COMMON" = "$WT_COMMON" ] || die "checkout '$WT' belongs to a different repository; inspect its registration before retrying"
[ "$(git -C "$WT" symbolic-ref -q HEAD)" = "$REF" ] || die "checkout '$WT' is not on requested branch '$BRANCH'"
echo "INFO: Continuing exact branch '$BRANCH' at '$WT'; local history and work retained." >&2
# Reused checkouts remain caller-owned: cleanup must not adopt dirty work.
if [ "$CREATED" = true ]; then
  bash "$TOOLS/amplihack_run_owned.sh" setup-worktree created "$WT" "$BRANCH" >&2 \
    || echo "WARNING: could not record ownership of new checkout '$WT'" >&2
fi
RUNTIME="$(bash "$TOOLS/workflow_runtime_root.sh" "$BRANCH")" || die "cannot create runtime root"
jq -n -c --arg path "$WT" --arg branch "$BRANCH" --arg runtime "$RUNTIME" --argjson created "$CREATED" \
  '{worktree_path: $path, branch_name: $branch, base_ref: "", base_branch: "", runtime_root: $runtime, created: $created}'
