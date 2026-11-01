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
#   workflow_worktree_root.sh base [repo_path]                  (issue #1808)
#     stdout : the folder new worktrees of this repository go under.
#              With AMPLIHACK_WORK_ROOT_EXPLICIT=1 and an absolute
#              AMPLIHACK_WORK_ROOT: <work root>/worktrees/<name>-<hash8>, where
#              <name> is the main checkout's basename reduced to [A-Za-z0-9._-]
#              and <hash8> the first 8 hex digits of the SHA-256 of its path.
#              Otherwise <main>/worktrees, as before (#1134 anchoring); when
#              the main checkout cannot be worked out, <repo_path>/worktrees.
#     exit 1 : only with an explicit work root: the main checkout cannot be
#              worked out, the work root is relative, or there is no hash
#              tool. The reason is on stderr. Callers stop and show it; they
#              never fall back, which would put worktrees in the place the
#              user asked to avoid. This is the one fallback rule: callers
#              make a single call and do not add their own.
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
    log "       workflow_worktree_root.sh assert-origin [repo_path]"
    log "       workflow_worktree_root.sh base [repo_path]"
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

# sha256_hex <text> — hex SHA-256 of <text>; fails when no hash tool exists.
sha256_hex() {
    local digest
    if command -v sha256sum >/dev/null 2>&1; then
        digest="$(printf '%s' "$1" | sha256sum)" || return 1
    elif command -v shasum >/dev/null 2>&1; then
        digest="$(printf '%s' "$1" | shasum -a 256)" || return 1
    else
        return 1
    fi
    digest="${digest%% *}"
    case "$digest" in
        *[!0-9a-f]*|"") return 1 ;;
    esac
    printf '%s\n' "$digest"
}

# cmd_base [repo_path] — echo the folder new worktrees go under (issue #1808).
cmd_base() {
    local main work name digest
    if [ "${AMPLIHACK_WORK_ROOT_EXPLICIT:-0}" != "1" ] || [ -z "${AMPLIHACK_WORK_ROOT:-}" ]; then
        main="$(cmd_root "$@" 2>/dev/null)" \
            || main="$(cd -P -- "${1:-.}" 2>/dev/null && pwd -P)" \
            || { log "ERROR: repo_path '${1:-.}' is not accessible."; return 1; }
        printf '%s/worktrees\n' "$main"
        return 0
    fi
    main="$(cmd_root "$@")" || return 1
    work="$AMPLIHACK_WORK_ROOT"
    case "$work" in
        /*) ;;
        *)
            log "ERROR: AMPLIHACK_WORK_ROOT '$work' is not an absolute path."
            return 1
            ;;
    esac
    while [ "${#work}" -gt 1 ] && [ "${work%/}" != "$work" ]; do
        work="${work%/}"
    done
    name="$(basename -- "$main")"
    name="$(printf '%s' "$name" | LC_ALL=C tr -c 'A-Za-z0-9._-' '_')"
    case "$name" in
        ""|.|..) name="repo" ;;
    esac
    if ! digest="$(sha256_hex "$main")"; then
        log "ERROR: no sha256sum or shasum to name the worktree base."
        return 1
    fi
    printf '%s/worktrees/%s-%s\n' "$work" "$name" "${digest:0:8}"
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

# assert-origin [repo_path]
# Refuse a repository that has no `origin` remote, before anything tries to
# fetch from it (issue #1323).
#
# A multi-repository workspace root is often a real git repository — so
# `git rev-parse --is-inside-work-tree` succeeds and the run proceeds — but it
# has no `origin`, because the actual code lives in nested repositories. The
# first fetch then fails with:
#
#     fatal: origin does not appear to be a git repository
#     ERROR: no supported remote base ref found. Expected origin/HEAD, ...
#
# Neither line mentions the real problem, so the reader goes looking for a
# missing branch or a broken remote URL rather than a mis-pointed repo_path.
cmd_assert_origin() {
    local repo="${1:-.}"
    if [ ! -d "$repo" ]; then
        log "ERROR: repo_path '$repo' does not exist or is not a directory."
        return 1
    fi
    if ! git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        log "ERROR: '$repo' is not a git repository."
        return 1
    fi
    if git -C "$repo" remote get-url origin >/dev/null 2>&1; then
        return 0
    fi

    log "ERROR: '$repo' is a git repository but has no 'origin' remote, so this"
    log "       workflow cannot resolve a base ref or push its work."
    local nested
    nested="$(find "$repo" -mindepth 2 -maxdepth 2 -name .git -printf '%h\n' 2>/dev/null | head -5)"
    if [ -n "$nested" ]; then
        log "       This looks like a multi-repository workspace. Repositories found:"
        printf '%s\n' "$nested" | while IFS= read -r d; do log "         $d"; done
        log "       Point repo_path at the one to work in (issue #1323)."
    else
        log "       Add an 'origin' remote, or point repo_path at a checkout that has one."
    fi
    return 1
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
        assert-origin)
            shift
            cmd_assert_origin "$@"
            ;;
        base)
            shift
            cmd_base "$@"
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
