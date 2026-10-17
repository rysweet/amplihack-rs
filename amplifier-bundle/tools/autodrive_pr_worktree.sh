#!/usr/bin/env bash
# autodrive_pr_worktree.sh — the PR's linked worktree for auto-drive phases 2/3
# (issue #1528).
#
# PROBLEM
#   The crusty loop and the merge-ready loop ran their rounds — and the rounds'
#   commit steps — in the checkout auto-drive was launched from, normally the
#   MAIN checkout on `main`. The PR branch's files live in a linked worktree
#   under the gitignored `worktrees/<branch>/`, so an agent asked to commit the
#   round's work found it there and force-added it, recording a
#   gitlink to another checkout in the PR. The rounds must run, and commit,
#   only inside the PR's own linked worktree.
#
# CONTRACT
#   autodrive_pr_worktree.sh resolve --repo DIR (--pr N | --head BRANCH)
#     --repo : any checkout of the repository (main or linked).
#     --pr   : a positive PR number; the head branch comes from
#              `gh pr view`, run inside --repo. A fork PR (or one whose
#              `isCrossRepository` is missing or null) is refused.
#     --head : an operator-supplied branch name; gh is never called.
#     stdout : on success exactly one line — the absolute, symlink-free path of
#              the one linked worktree that has the PR head branch checked out.
#              On any refusal stdout is empty.
#     exit 0 : resolved (an existing linked worktree, or one created at
#              <main>/worktrees/<branch>).
#     exit 1 : refused; the reason, mentioning #1528, is on stderr. The
#              rejected branch name is never echoed.
#     exit 2 : usage error (missing, repeated, unknown or dash-prefixed flag
#              values; an invalid --pr). Detected before gh or git runs.
#
#   Refusals: the main checkout is on the branch; a prunable or ambiguous
#   match; a branch name git would reject or outside [A-Za-z0-9._/-]; a target
#   path that already exists unregistered, is registered to another branch, or
#   has a symlinked or non-directory component under worktrees/; a worktree
#   that belongs to another repository; any path containing `"`, `$`, a
#   backtick, a backslash or a control character (the path is substituted into
#   agent prompts as `cd "{{repo_path}}"`).
#
#   Stale local branch: if refs/heads/<branch> exists locally and is behind
#   origin, a newly created worktree checks out the stale local tip. The
#   round's push then fails non-fast-forward and step-04 stops. It never
#   forces.
#
# READ-ONLY INVARIANT (hard): the only writes are a fetch into the
# remote-tracking ref (`refs/remotes/origin/<branch>`, no tags) and
# `git worktree add` of a NEW worktree for the branch. Nothing is ever forced,
# staged, committed, checked out in an existing tree, pruned or deleted.
#
# The main repository work tree comes from workflow_worktree_root.sh, so both
# helpers agree on where `worktrees/` lives.

set -euo pipefail
export LC_ALL=C

# Repository-selecting variables (amplihack_git::REPOSITORY_ENV_VARS) would
# point every git call at another repository; CDPATH would make `cd` resolve a
# relative --repo elsewhere and print the result on stdout; GH_REPO would make
# `gh pr view` look the PR up in a repository other than the `origin` that is
# fetched and pushed.
unset GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR \
    GIT_DIR GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_INDEX_FILE GIT_INDEX_VERSION \
    GIT_NAMESPACE GIT_OBJECT_DIRECTORY GIT_PREFIX GIT_QUARANTINE_PATH \
    GIT_WORK_TREE CDPATH GH_REPO

log() { printf '%s\n' "$*" >&2; }

usage() {
    log "usage: autodrive_pr_worktree.sh resolve --repo DIR (--pr N | --head BRANCH)"
}

usage_error() {
    log "autodrive_pr_worktree.sh: $*"
    usage
    exit 2
}

refuse() {
    log "autodrive_pr_worktree.sh: refusing: $*"
    log "autodrive_pr_worktree.sh: auto-drive rounds run only in the PR's linked worktree (#1528)."
    exit 1
}

# safe_path <path> — no character that is live inside a double-quoted string.
safe_path() {
    case "$1" in
        *[\"\$\`\\]* | *[[:cntrl:]]*) return 1 ;;
        /*) return 0 ;;
        *) return 1 ;;
    esac
}

# canonical_dir <dir> — `pwd -P` of an existing directory, nothing otherwise.
canonical_dir() {
    (cd -P -- "$1" && pwd -P)
}

# common_dir_of <dir> — the canonical common git dir of the checkout at <dir>.
common_dir_of() {
    local raw
    raw="$(git -C "$1" rev-parse --path-format=absolute --git-common-dir)" || return 1
    canonical_dir "$raw"
}

# verify_worktree <path> <branch> <common> — <path> is a LINKED worktree of
# this repository (same common dir) with refs/heads/<branch> checked out.
verify_worktree() {
    local path="$1" branch="$2" common="$3" theirs gitdir ref
    theirs="$(common_dir_of "$path")" || return 1
    [ "$theirs" = "$common" ] || return 1
    gitdir="$(git -C "$path" rev-parse --absolute-git-dir)" || return 1
    gitdir="$(canonical_dir "$gitdir")" || return 1
    [ "$gitdir" != "$common" ] || return 1
    ref="$(git -C "$path" symbolic-ref -q HEAD)" || return 1
    [ "$ref" = "refs/heads/$branch" ]
}

# check_target_components <root> <branch> — every existing component of
# <root>/worktrees/<branch> below <root> is a real directory, and the final
# component does not exist at all (not even as a dangling symlink).
check_target_components() {
    local current="$1/worktrees" rest="$2" part
    if [ -L "$current" ] || { [ -e "$current" ] && [ ! -d "$current" ]; }; then
        refuse "worktrees/ under the main checkout is a symlink or not a directory"
    fi
    while :; do
        part="${rest%%/*}"
        current="$current/$part"
        if [ "$part" = "$rest" ]; then
            if [ -e "$current" ] || [ -L "$current" ]; then
                refuse "the target worktree path already exists but is not a registered worktree of the branch; it is left untouched"
            fi
            return 0
        fi
        rest="${rest#*/}"
        if [ -L "$current" ]; then
            refuse "a component of the target path under worktrees/ is a symlink"
        fi
        if [ -e "$current" ] && [ ! -d "$current" ]; then
            refuse "a component of the target path under worktrees/ is not a directory"
        fi
    done
}

cmd_resolve() {
    local repo="" pr="" head="" seen_repo=0 seen_pr=0 seen_head=0 flag value
    while [ "$#" -gt 0 ]; do
        flag="$1"
        case "$flag" in
            --repo | --pr | --head) ;;
            *) usage_error "unknown argument" ;;
        esac
        [ "$#" -ge 2 ] || usage_error "$flag needs a value"
        value="$2"
        case "$value" in
            "" | -*) usage_error "$flag needs a value" ;;
        esac
        case "$flag" in
            --repo)
                [ "$seen_repo" = 0 ] || usage_error "--repo given twice"
                seen_repo=1
                repo="$value"
                ;;
            --pr)
                [ "$seen_pr" = 0 ] || usage_error "--pr given twice"
                seen_pr=1
                pr="$value"
                ;;
            --head)
                [ "$seen_head" = 0 ] || usage_error "--head given twice"
                seen_head=1
                head="$value"
                ;;
        esac
        shift 2
    done
    [ "$seen_repo" = 1 ] || usage_error "--repo is required"
    [ $((seen_pr + seen_head)) = 1 ] || usage_error "exactly one of --pr or --head is required"
    if [ "$seen_pr" = 1 ] && ! [[ "$pr" =~ ^[1-9][0-9]{0,8}$ ]]; then
        usage_error "--pr must be a positive PR number"
    fi

    cd -P -- "$repo" || refuse "--repo is not an accessible directory"
    git rev-parse --is-inside-work-tree >/dev/null || refuse "--repo is not inside a git work tree"
    local common
    common="$(common_dir_of .)" || refuse "cannot read the repository's common git dir"

    local root_helper main_root
    root_helper="$(dirname -- "${BASH_SOURCE[0]}")/workflow_worktree_root.sh"
    [ -f "$root_helper" ] || refuse "workflow_worktree_root.sh not found next to this helper"
    main_root="$(bash "$root_helper" root "$PWD")" || refuse "cannot resolve the main repository work tree"
    main_root="$(canonical_dir "$main_root")" || refuse "the main repository work tree is not accessible"

    if [ "$seen_pr" = 1 ]; then
        head="$(gh pr view "$pr" --json headRefName,isCrossRepository \
            --jq 'select(.isCrossRepository == false) | .headRefName')" \
            || refuse "gh could not resolve the PR's head branch"
        [ -n "$head" ] || refuse "gh reported no same-repository head branch (a fork PR, or the fork flag is missing)"
    fi

    # The name is attacker-controlled: allowlist first, then git's own rules.
    [[ "$head" =~ ^[A-Za-z0-9._/-]+$ ]] || refuse "the branch name contains characters outside [A-Za-z0-9._/-]"
    case "$head" in
        -*) refuse "the branch name starts with a dash" ;;
    esac
    local checked
    checked="$(git check-ref-format --branch "$head" 2>/dev/null)" \
        || refuse "the branch name is not a valid git branch name"
    [ "$checked" = "$head" ] || refuse "the branch name is not a valid git branch name"

    local listing
    listing="$(git worktree list --porcelain)" || refuse "cannot list the repository's worktrees"
    local target="$main_root/worktrees/$head"
    local line rec_path="" rec_branch="" rec_prunable=0 index=0
    local matches=0 match_path="" main_on_branch=0 prunable_match=0 target_taken=0
    while IFS= read -r line; do
        case "$line" in
            "worktree "*)
                rec_path="${line#worktree }"
                rec_branch=""
                rec_prunable=0
                ;;
            "branch "*) rec_branch="${line#branch }" ;;
            prunable | "prunable "*) rec_prunable=1 ;;
            "")
                if [ -n "$rec_path" ]; then
                    index=$((index + 1))
                    if [ "$rec_branch" = "refs/heads/$head" ]; then
                        if [ "$index" = 1 ]; then
                            main_on_branch=1
                        else
                            matches=$((matches + 1))
                            match_path="$rec_path"
                            [ "$rec_prunable" = 0 ] || prunable_match=1
                        fi
                    elif [ "$rec_path" = "$target" ]; then
                        target_taken=1
                    fi
                fi
                rec_path=""
                ;;
        esac
    done <<EOF
$listing

EOF

    if [ "$main_on_branch" = 1 ]; then
        refuse "the main checkout has the PR branch checked out; switch the main checkout off it so a linked worktree can hold it"
    fi
    if [ "$prunable_match" = 1 ]; then
        refuse "a stale (prunable) worktree registration holds the branch; inspect it and run 'git worktree prune' yourself"
    fi
    if [ "$matches" -gt 1 ]; then
        refuse "more than one linked worktree has the branch checked out; remove the extra ones"
    fi

    local found
    if [ "$matches" = 1 ]; then
        safe_path "$match_path" || refuse "the matching worktree's path contains shell metacharacters or control characters"
        [ -d "$match_path" ] || refuse "the matching worktree's path is not a directory"
        found="$(canonical_dir "$match_path")" || refuse "the matching worktree is not accessible"
        safe_path "$found" || refuse "the matching worktree's path contains shell metacharacters or control characters"
        verify_worktree "$found" "$head" "$common" \
            || refuse "the registered worktree is not a linked worktree of this repository on the branch"
        printf '%s\n' "$found"
        return 0
    fi

    # No worktree has the branch: create one at <main>/worktrees/<branch>.
    safe_path "$target" || refuse "the target worktree path contains shell metacharacters or control characters"
    [ "$target_taken" = 0 ] || refuse "the target worktree path is registered to another branch"
    check_target_components "$main_root" "$head"

    if git show-ref --verify --quiet "refs/heads/$head"; then
        git worktree add "$target" "$head" >&2 || refuse "git worktree add failed"
    else
        GIT_TERMINAL_PROMPT=0 git fetch --no-tags origin "+refs/heads/${head}:refs/remotes/origin/${head}" >&2 \
            || refuse "the branch is neither local nor fetchable from origin"
        git worktree add --track -b "$head" "$target" "origin/$head" >&2 || refuse "git worktree add failed"
    fi

    found="$(canonical_dir "$target")" || refuse "the new worktree is not accessible"
    [ "$found" = "$target" ] || refuse "the new worktree did not land at the target path"
    verify_worktree "$found" "$head" "$common" || refuse "the new worktree failed verification"
    printf '%s\n' "$found"
}

main() {
    [ "$#" -ge 1 ] || usage_error "missing operation"
    case "$1" in
        resolve)
            shift
            cmd_resolve "$@"
            ;;
        *) usage_error "unknown operation" ;;
    esac
}

main "$@"
