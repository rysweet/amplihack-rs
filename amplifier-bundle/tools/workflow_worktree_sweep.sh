#!/usr/bin/env bash
# workflow_worktree_sweep.sh — leak-proof, self-healing worktree lifecycle helper
# for the default-workflow (issue #840).
#
# The idempotent reuse/reset/recreate state machine in workflow-worktree.yaml
# already prevents the original "cannot delete branch X used by worktree at PATH"
# collision. This helper closes the two remaining acceptance criteria of #840:
#
#   sweep <repo_path>
#       Best-effort orphan sweep. Runs `git worktree prune`, then for each
#       registered worktree under <repo_path>/worktrees/ that is BOTH stale
#       (directory mtime older than AMPLIHACK_WORKTREE_STALE_SECS) AND carries
#       no unmerged meaningful commits (rev-list --count BASE..HEAD == 0), it
#       removes the worktree directory and its branch so a re-run converges.
#       Stale-but-unmerged worktrees are archived (never destroyed); fresh
#       worktrees are left untouched. Always exits 0 — a sweep failure must
#       never abort setup.
#
#   cleanup_on_failure <repo_path> <branch>
#       Tear down THIS run's worktree+branch after a failed/aborted run, but
#       only AFTER archiving any unique commits to refs/amplihack-archive/<branch>
#       (and best-effort pushing them). Never destroys unmerged meaningful work.
#
# This is a self-contained brick: it is safe to `source` (it only defines
# functions until invoked) and safe to run as a CLI. It depends solely on git
# and POSIX coreutils. A missing helper is a graceful no-op at the call site,
# so callers guard the invocation with `-f`/`|| true` (per #829 precedent).
#
# Env knobs:
#   AMPLIHACK_WORKTREE_STALE_SECS   Staleness threshold in seconds (default
#                                   86400). Validated ^[0-9]+$; anything else
#                                   falls back to the default.
#   AMPLIHACK_WORKTREE_BASE_REF     Base ref for the unmerged-diff gate. When
#                                   unset the helper resolves, in order,
#                                   origin/HEAD -> origin/main -> origin/master
#                                   -> main -> master.

set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_STALE_SECS=86400
readonly ARCHIVE_NS="refs/amplihack-archive"

log_info() { printf 'INFO: %s\n' "$*" >&2; }
log_warn() { printf 'WARN: %s\n' "$*" >&2; }

# stale_threshold — echo the validated staleness threshold (seconds).
stale_threshold() {
    local raw="${AMPLIHACK_WORKTREE_STALE_SECS:-}"
    if [[ "$raw" =~ ^[0-9]+$ ]]; then
        printf '%s\n' "$raw"
    else
        if [ -n "$raw" ]; then
            log_warn "AMPLIHACK_WORKTREE_STALE_SECS='$raw' is not a non-negative integer; using ${DEFAULT_STALE_SECS}."
        fi
        printf '%s\n' "$DEFAULT_STALE_SECS"
    fi
}

# dir_mtime <path> — echo the directory mtime as epoch seconds (portable).
dir_mtime() {
    local path="$1"
    stat -c %Y "$path" 2>/dev/null || stat -f %m "$path" 2>/dev/null || printf '0\n'
}

# valid_branch_name <branch> — reject option-injection / traversal in a branch arg.
valid_branch_name() {
    local b="$1"
    case "$b" in
        -*|*..*) return 1 ;;
    esac
    [[ "$b" =~ ^[A-Za-z0-9._/-]+$ ]]
}

# resolve_base_ref <repo> — echo a base ref usable for the unmerged-diff gate,
# or nothing (and return 1) when none can be resolved. Conservative by design:
# an unresolved base means we cannot prove a worktree is merged, so callers MUST
# preserve the worktree in that case.
resolve_base_ref() {
    local repo="$1" candidate
    candidate="${AMPLIHACK_WORKTREE_BASE_REF:-}"
    if [ -n "$candidate" ] && git -C "$repo" rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null 2>&1; then
        printf '%s\n' "$candidate"
        return 0
    fi
    candidate="$(git -C "$repo" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
    if [ -n "$candidate" ] && git -C "$repo" rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null 2>&1; then
        printf '%s\n' "$candidate"
        return 0
    fi
    local ref
    for ref in origin/main origin/master main master; do
        if git -C "$repo" rev-parse --verify --quiet "${ref}^{commit}" >/dev/null 2>&1; then
            printf '%s\n' "$ref"
            return 0
        fi
    done
    return 1
}

# commits_ahead <repo> <base> <committish> — echo count of commits on
# <committish> not reachable from <base>. Echoes a large sentinel on error so
# the caller treats an unknown state as "has unmerged work" (preserve).
commits_ahead() {
    local repo="$1" base="$2" tip="$3"
    git -C "$repo" rev-list --count "${base}..${tip}" 2>/dev/null || printf '1\n'
}

# archive_branch <repo> <branch> <sha> — record <sha> under the archive
# namespace so unique work stays reachable after a prune. Best-effort remote
# push (relies on configured auth; never echoes tokens).
archive_branch() {
    local repo="$1" branch="$2" sha="$3"
    local ref="${ARCHIVE_NS}/${branch}"
    if git -C "$repo" update-ref "$ref" "$sha" 2>/dev/null; then
        log_info "Archived '${branch}' (${sha}) to ${ref}."
    else
        log_warn "Could not archive '${branch}' to ${ref} locally."
        return 1
    fi
    git -C "$repo" push origin "${sha}:refs/heads/${branch}" >/dev/null 2>&1 \
        || git -C "$repo" push origin "${ref}:${ref}" >/dev/null 2>&1 \
        || log_warn "Best-effort archive push for '${branch}' did not complete; local archive ref retained."
    return 0
}

# remove_worktree_and_branch <repo> <path> <branch> — destructive teardown,
# ordered worktree-first so the branch is no longer checked out when deleted
# (this ordering is what prevents the #840 'branch used by worktree' error).
remove_worktree_and_branch() {
    local repo="$1" path="$2" branch="$3"
    if [ -n "$path" ]; then
        git -C "$repo" worktree remove --force -- "$path" >/dev/null 2>&1 \
            || rm -rf -- "$path" 2>/dev/null \
            || log_warn "Could not fully remove worktree directory '${path}'."
    fi
    git -C "$repo" worktree prune >/dev/null 2>&1 || true
    if [ -n "$branch" ] && git -C "$repo" show-ref --verify --quiet "refs/heads/${branch}"; then
        git -C "$repo" branch -D "$branch" >/dev/null 2>&1 \
            || log_warn "Could not delete branch '${branch}'."
    fi
}

# within_worktrees_dir <repo> <path> — true only when <path> is strictly
# contained under <repo>/worktrees/ after symlink/.. resolution. Containment
# guard: destructive ops never escape the managed worktrees/ subtree.
within_worktrees_dir() {
    local repo="$1" path="$2" base resolved
    base="$(cd "$repo" 2>/dev/null && pwd -P)/worktrees" || return 1
    resolved="$(cd "$path" 2>/dev/null && pwd -P)" || return 1
    case "$resolved" in
        "$base"/?*) return 0 ;;
        *) return 1 ;;
    esac
}

# sweep <repo_path> — best-effort orphan sweep. Always returns 0.
sweep() {
    local repo="${1:-}"
    if [ -z "$repo" ] || [ ! -d "$repo" ]; then
        log_warn "sweep: repo_path '${repo}' is not a directory; nothing to do."
        return 0
    fi
    if ! git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        log_warn "sweep: '${repo}' is not a git work tree; nothing to do."
        return 0
    fi

    git -C "$repo" worktree prune >/dev/null 2>&1 || true

    local threshold base now
    threshold="$(stale_threshold)"
    now="$(date +%s)"
    base="$(resolve_base_ref "$repo" || true)"

    # Parse registered worktrees from porcelain (never scrape `ls`).
    # Enumerate via: git worktree list --porcelain
    local wt_path="" wt_branch=""
    local porcelain
    porcelain="$(git -C "$repo" worktree list --porcelain 2>/dev/null || true)"

    local line
    while IFS= read -r line; do
        case "$line" in
            "worktree "*)
                wt_path="${line#worktree }"
                wt_branch=""
                ;;
            "branch refs/heads/"*)
                wt_branch="${line#branch refs/heads/}"
                ;;
            "")
                _sweep_consider "$repo" "$wt_path" "$wt_branch" "$threshold" "$now" "$base"
                wt_path=""
                wt_branch=""
                ;;
        esac
    done <<EOF
${porcelain}

EOF
    return 0
}

# _sweep_consider <repo> <path> <branch> <threshold> <now> <base>
# Evaluate one registered worktree and act on it. Best-effort throughout.
_sweep_consider() {
    local repo="$1" path="$2" branch="$3" threshold="$4" now="$5" base="$6"
    [ -n "$path" ] || return 0
    [ -d "$path" ] || return 0
    # Containment: only ever touch worktrees under <repo>/worktrees/. This skips
    # the primary checkout and any sibling/external worktree.
    within_worktrees_dir "$repo" "$path" || return 0

    local mtime age
    mtime="$(dir_mtime "$path")"
    age=$(( now - mtime ))
    if [ "$age" -lt "$threshold" ]; then
        log_info "Keeping fresh worktree '${path}' (age ${age}s < ${threshold}s)."
        return 0
    fi

    # Unmerged-diff safety gate. Without a resolvable base we cannot prove the
    # worktree is merged, so we preserve it.
    if [ -z "$branch" ]; then
        log_warn "Keeping detached/branchless stale worktree '${path}' (cannot evaluate merge state)."
        return 0
    fi
    if [ -z "$base" ]; then
        log_warn "Keeping stale worktree '${path}' — no base ref resolved to prove it is merged."
        return 0
    fi

    local ahead sha
    ahead="$(commits_ahead "$repo" "$base" "$branch")"
    if [ "$ahead" -gt 0 ]; then
        sha="$(git -C "$repo" rev-parse --verify --quiet "refs/heads/${branch}" 2>/dev/null || true)"
        if [ -n "$sha" ]; then
            archive_branch "$repo" "$branch" "$sha" || true
        fi
        log_warn "Preserving stale worktree '${path}' — branch '${branch}' has ${ahead} unmerged commit(s) (archived, not destroyed)."
        return 0
    fi

    log_info "Pruning stale orphan worktree '${path}' (branch '${branch}', age ${age}s, no unmerged commits)."
    remove_worktree_and_branch "$repo" "$path" "$branch"
}

# cleanup_on_failure <repo_path> <branch> — archive then tear down this run's
# worktree+branch. Always returns 0 (best-effort), but never destroys unique
# commits without archiving them first.
cleanup_on_failure() {
    local repo="${1:-}" branch="${2:-}"
    if [ -z "$repo" ] || [ ! -d "$repo" ]; then
        log_warn "cleanup_on_failure: repo_path '${repo}' is not a directory; nothing to do."
        return 0
    fi
    if ! git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        log_warn "cleanup_on_failure: '${repo}' is not a git work tree; nothing to do."
        return 0
    fi
    if [ -z "$branch" ] || ! valid_branch_name "$branch"; then
        log_warn "cleanup_on_failure: invalid or empty branch '${branch}'; refusing to act."
        return 0
    fi

    git -C "$repo" worktree prune >/dev/null 2>&1 || true

    # Locate the worktree registered for this branch (porcelain), with a
    # conventional fallback to <repo>/worktrees/<branch>.
    local path
    path="$(git -C "$repo" worktree list --porcelain 2>/dev/null | awk -v b="refs/heads/${branch}" '
        $1=="worktree" { wt=$2 }
        $1=="branch" && $2==b { print wt; exit }
    ')"
    if [ -z "$path" ] && [ -d "${repo}/worktrees/${branch}" ]; then
        path="${repo}/worktrees/${branch}"
    fi

    # Archive any commits this branch carries before destroying anything. If the
    # branch is not even present there is nothing to archive or remove.
    local sha
    sha="$(git -C "$repo" rev-parse --verify --quiet "refs/heads/${branch}" 2>/dev/null || true)"
    if [ -z "$sha" ]; then
        log_info "cleanup_on_failure: branch '${branch}' not present; pruning any stale worktree dir."
        if [ -n "$path" ] && within_worktrees_dir "$repo" "$path"; then
            git -C "$repo" worktree remove --force -- "$path" >/dev/null 2>&1 || rm -rf -- "$path" 2>/dev/null || true
            git -C "$repo" worktree prune >/dev/null 2>&1 || true
        fi
        return 0
    fi

    local base ahead
    base="$(resolve_base_ref "$repo" || true)"
    ahead=1
    if [ -n "$base" ]; then
        ahead="$(commits_ahead "$repo" "$base" "$branch")"
    fi

    if [ "$ahead" -gt 0 ]; then
        if ! archive_branch "$repo" "$branch" "$sha"; then
            log_warn "cleanup_on_failure: could not archive unique work on '${branch}'; refusing to prune (data-loss guard)."
            return 0
        fi
    else
        log_info "cleanup_on_failure: branch '${branch}' has no unmerged commits; pruning."
    fi

    if [ -n "$path" ] && ! within_worktrees_dir "$repo" "$path"; then
        log_warn "cleanup_on_failure: worktree '${path}' is outside managed worktrees/; pruning branch only."
        path=""
    fi
    remove_worktree_and_branch "$repo" "$path" "$branch"
    return 0
}

main() {
    local mode="${1:-}"
    case "$mode" in
        sweep)
            shift || true
            sweep "${1:-}"
            ;;
        cleanup_on_failure)
            shift || true
            cleanup_on_failure "${1:-}" "${2:-}"
            ;;
        ""|-h|--help|help)
            cat >&2 <<'USAGE'
Usage:
  workflow_worktree_sweep.sh sweep <repo_path>
  workflow_worktree_sweep.sh cleanup_on_failure <repo_path> <branch>

Env:
  AMPLIHACK_WORKTREE_STALE_SECS   staleness threshold (default 86400)
  AMPLIHACK_WORKTREE_BASE_REF     base ref for the unmerged-diff gate
USAGE
            [ -n "$mode" ] && return 0 || return 2
            ;;
        *)
            log_warn "unknown mode '${mode}'."
            return 2
            ;;
    esac
}

# Only run main when executed directly, so the file is safe to `source`.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    main "$@"
fi
