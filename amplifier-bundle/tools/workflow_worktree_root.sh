#!/usr/bin/env bash
# workflow_worktree_root.sh — canonical worktree anchoring (issue #1134).
#
# PROBLEM
#   step-04-setup-worktree resolved its worktree base from the recipe run's own
#   working directory (`repo_path="."` -> `REPO_PATH="$(pwd -P)"`) and then built
#   "${REPO_PATH}/worktrees/<branch>". Nesting *recipe runners* is legitimate and
#   expected, and a nested run's cwd is normally a LINKED worktree — so the new
#   worktree landed INSIDE the parent's worktree, three levels deep in the
#   reported incident, and without bound.
#
# WHY `--show-toplevel` IS THE WRONG TOOL HERE
#   `git rev-parse --show-toplevel` answers "which work tree am I standing in?".
#   Inside a linked worktree that is the LINKED worktree — precisely the wrong
#   anchor, and exactly what `pwd -P` already gave us. A linked worktree's `.git`
#   is a FILE ("gitdir: <main>/.git/worktrees/<name>"), not a directory;
#   `git rev-parse --git-common-dir` follows that pointer back to the MAIN
#   repository's `.git`, which is identical for every worktree of the repo. Its
#   parent directory is therefore the one stable, shared anchor, and that is what
#   `root` returns.
#
# CONTRACT
#   workflow_worktree_root.sh root [repo_path]
#     stdout : exactly one line — absolute path of the MAIN repository work tree.
#     exit 0 : resolved.  exit 1 : not a git work tree / unresolvable.
#
#   workflow_worktree_root.sh assert-not-nested <candidate_path> [main_root]
#     exit 0 : <candidate_path> is not a descendant of any registered worktree
#              other than <main_root> (which legitimately holds `worktrees/`).
#     exit 1 : nesting detected; diagnostics on stderr.
#
# READ-ONLY INVARIANT (hard): this helper only inspects git state and path
# strings. It creates, moves, resets and deletes nothing.
#
# A missing helper is a graceful no-op at the call site (per the #829 precedent),
# so callers guard the invocation with `-f`.

set -euo pipefail

log() { printf '%s\n' "$*" >&2; }

usage() {
    log "usage: workflow_worktree_root.sh root [repo_path]"
    log "       workflow_worktree_root.sh assert-not-nested <candidate_path> [main_root]"
}

# normalize <path>
# Resolve symlinks and '.'/'..' segments (pwd -P semantics) even when the path
# does not exist yet: walk up to the deepest existing ancestor, canonicalize it,
# then re-append the missing tail. Mirrors workflow_worktree_deconflict.sh so the
# two helpers agree on what "the same path" means.
normalize() {
    local p="$1"
    local dir rest base realdir
    if [ -d "$p" ]; then
        (cd "$p" 2>/dev/null && pwd -P) || printf '%s\n' "$p"
        return 0
    fi
    dir="$p"
    rest=""
    while [ ! -d "$dir" ] && [ "$dir" != "/" ] && [ "$dir" != "." ]; do
        base="$(basename -- "$dir")"
        if [ -n "$rest" ]; then
            rest="$base/$rest"
        else
            rest="$base"
        fi
        dir="$(dirname -- "$dir")"
    done
    if ! realdir="$(cd "$dir" 2>/dev/null && pwd -P)"; then
        realdir="$dir"
    fi
    if [ -n "$rest" ]; then
        printf '%s/%s\n' "$realdir" "$rest"
    else
        printf '%s\n' "$realdir"
    fi
}

# cmd_root [repo_path] — echo the MAIN repository work tree.
cmd_root() {
    local repo="${1:-}"
    local common abs
    if [ -z "$repo" ]; then
        repo="$(pwd)"
    fi
    if ! cd "$repo" 2>/dev/null; then
        log "ERROR: repo_path '$repo' is not accessible."
        return 1
    fi
    if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        log "ERROR: '$repo' is not inside a git work tree."
        return 1
    fi
    common="$(git rev-parse --git-common-dir 2>/dev/null || true)"
    if [ -z "$common" ]; then
        log "ERROR: 'git rev-parse --git-common-dir' produced no answer in '$repo'."
        return 1
    fi
    # --git-common-dir may be relative to the current directory.
    case "$common" in
        /*) ;;
        *) common="$(pwd -P)/$common" ;;
    esac
    if ! abs="$(cd "$common" 2>/dev/null && pwd -P)"; then
        log "ERROR: common git dir '$common' is not accessible."
        return 1
    fi
    case "$abs" in
        */.git)
            # Standard layout: <main-work-tree>/.git — the parent is the anchor.
            printf '%s\n' "${abs%/.git}"
            ;;
        *)
            # Bare or non-standard gitdir. It is not a work tree, and git itself
            # owns <gitdir>/worktrees, so anchor BESIDE it, never inside it.
            dirname -- "$abs"
            ;;
    esac
}

# worktree_paths [repo] — porcelain worktree registrations. When <repo> is given
# the query is bound to that repository explicitly, so the caller's cwd can never
# silently point the check at a different checkout.
worktree_paths() {
    local repo="${1:-}"
    if [ -n "$repo" ] && [ -d "$repo" ]; then
        git -C "$repo" worktree list --porcelain 2>/dev/null || true
    else
        git worktree list --porcelain 2>/dev/null || true
    fi
}

# cmd_assert_not_nested <candidate_path> [main_root]
cmd_assert_not_nested() {
    local candidate="${1:-}"
    local main_root="${2:-}"
    local cand root line p pn
    if [ -z "$candidate" ]; then
        log "ERROR: assert-not-nested requires <candidate_path>."
        usage
        return 1
    fi
    cand="$(normalize "$candidate")"
    root=""
    if [ -n "$main_root" ]; then
        root="$(normalize "$main_root")"
    fi
    while IFS= read -r line; do
        case "$line" in
            "worktree "*) p="${line#worktree }" ;;
            *) continue ;;
        esac
        pn="$(normalize "$p")"
        # The MAIN work tree legitimately contains the managed worktrees/ dir.
        if [ "$pn" = "$root" ]; then
            continue
        fi
        # Re-running against our own path is reuse, not nesting.
        if [ "$pn" = "$cand" ]; then
            continue
        fi
        case "$cand" in
            "$pn"/*)
                log "ERROR: refusing worktree path '${cand}' — it is nested inside existing worktree '${pn}' (issue #1134)."
                log "ERROR: worktrees must be anchored at the main repository, never inside another worktree."
                return 1
                ;;
        esac
    done <<EOF
$(worktree_paths "$root")
EOF
    return 0
}

main() {
    local op="${1:-}"
    case "$op" in
        root)
            shift
            cmd_root "$@"
            ;;
        assert-not-nested)
            shift
            cmd_assert_not_nested "$@"
            ;;
        ""|-h|--help|help)
            usage
            [ -z "$op" ] && return 1 || return 0
            ;;
        *)
            log "ERROR: unknown operation '$op'."
            usage
            return 1
            ;;
    esac
}

# Only run main when executed directly, so the file is safe to `source`.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    main "$@"
fi
