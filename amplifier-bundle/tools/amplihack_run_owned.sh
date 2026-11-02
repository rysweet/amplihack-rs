#!/usr/bin/env bash
# amplihack_run_owned.sh — record a path the current recipe run created
# (issue #1808).
#
# `amplihack cleanup` removes a worktree or cargo target only when the run that
# created it recorded it in `$AMPLIHACK_RUN_DIR/owned-paths`. Each line holds
# three tab-separated fields, kind, path and branch; a target has an empty
# branch, so its line ends with a tab.
#
# CONTRACT
#   amplihack_run_owned.sh record <worktree|target> <absolute path> [branch]
#     exit 0 : recorded, or nothing to do (outside a run, or already recorded).
#     exit 1 : refused or failed (relative path, control characters, unknown
#              kind, symlinked owned-paths file, write failure).
#     exit 2 : usage error.
#   amplihack_run_owned.sh adopt worktree <absolute path> [branch]
#     For a worktree this run reuses: records it, as `record` does, only when
#     another run under the same runs folder already recorded that path. A
#     reused worktree then stays owned by every run that used it, and cleanup
#     keeps it while any of them is active. A worktree no run created (one
#     made by hand) is never recorded. Exit codes as for `record`.
#   amplihack_run_owned.sh setup-worktree <setup state> <worktree path> [branch]
#     For a recipe's worktree setup step: resolves <worktree path> to its
#     physical path, then runs `adopt` when <setup state> is
#     reuse-clean-worktree and `record` otherwise. Outside a run it does
#     nothing. On any failure it prints
#     `WARNING: could not record <path> for cleanup (issue #1808)` and exits 1;
#     callers ignore the status (`|| true`), so a failed record never fails
#     the step. Exit 2 is a usage error.
#
# A recipe's worktree setup step calls `setup-worktree` and nothing else:
# `bash amplihack_run_owned.sh setup-worktree <state> <path> <branch> || true`.
# It records only after a new `git worktree add` succeeded, adopts after
# reusing one, and prints its own warning on failure. `record` and `adopt`
# remain for callers that are not a worktree setup step.

set -u
umask 077

log() { printf 'amplihack_run_owned: %s\n' "$*" >&2; }

usage() {
    log "usage: amplihack_run_owned.sh record <worktree|target> <absolute path> [branch]"
    log "       amplihack_run_owned.sh adopt worktree <absolute path> [branch]"
    log "       amplihack_run_owned.sh setup-worktree <setup state> <worktree path> [branch]"
}

has_control_chars() {
    # Any byte below 0x20 or DEL. LC_ALL=C makes the class byte-based.
    LC_ALL=C
    case "$1" in
        *[[:cntrl:]]*) return 0 ;;
    esac
    return 1
}

cmd_record() {
    local kind="${1:-}" path="${2:-}" branch="${3:-}"
    if [ -z "$kind" ] || [ -z "$path" ] || [ "$#" -gt 3 ]; then
        usage
        return 2
    fi
    case "$kind" in
        worktree|target) ;;
        *) log "unknown kind '$kind' (expected worktree or target)"; return 1 ;;
    esac
    case "$path" in
        /*) ;;
        *) log "refusing relative path '$path'"; return 1 ;;
    esac
    if has_control_chars "$path" || has_control_chars "$branch"; then
        log "refusing a path or branch with control characters"
        return 1
    fi
    local run_dir="${AMPLIHACK_RUN_DIR:-}"
    if [ -z "$run_dir" ]; then
        # Outside a run there is nothing to record into.
        return 0
    fi
    case "$run_dir" in
        /*) ;;
        *) log "AMPLIHACK_RUN_DIR '$run_dir' is not absolute"; return 1 ;;
    esac
    if [ ! -d "$run_dir" ] || [ -L "$run_dir" ]; then
        log "run folder '$run_dir' is missing or a symbolic link"
        return 1
    fi
    local owned="$run_dir/owned-paths"
    if [ -L "$owned" ]; then
        log "refusing symbolic link '$owned'"
        return 1
    fi
    local line
    line="$(printf '%s\t%s\t%s' "$kind" "$path" "$branch")"
    if [ -f "$owned" ] && grep -qxF -- "$line" "$owned" 2>/dev/null; then
        return 0
    fi
    if ! printf '%s\t%s\t%s\n' "$kind" "$path" "$branch" >> "$owned"; then
        log "cannot append to '$owned'"
        return 1
    fi
    chmod 600 "$owned" 2>/dev/null || true
    return 0
}

# Records a reused worktree only when another run recorded it first.
cmd_adopt() {
    local kind="${1:-}" path="${2:-}"
    if [ "$kind" != worktree ] || [ -z "$path" ] || [ "$#" -gt 3 ]; then
        usage
        return 2
    fi
    local run_dir="${AMPLIHACK_RUN_DIR:-}"
    [ -n "$run_dir" ] || return 0
    if has_control_chars "$path"; then
        log "refusing a path or branch with control characters"
        return 1
    fi
    local other want
    want="$(printf 'worktree\t%s' "$path")"
    for other in "$(dirname -- "$run_dir")"/*/owned-paths; do
        [ -f "$other" ] && [ ! -L "$other" ] || continue
        [ "$other" != "$run_dir/owned-paths" ] || continue
        if cut -f1,2 -- "$other" 2>/dev/null | grep -qxF -- "$want"; then
            cmd_record "$@"
            return $?
        fi
    done
    return 0
}

# The worktree setup step's single call: pick record or adopt from the setup
# state and turn any failure into one warning line.
cmd_setup_worktree() {
    local state="${1:-}" path="${2:-}" branch="${3:-}" real op=record
    if [ -z "$state" ] || [ -z "$path" ] || [ "$#" -gt 3 ]; then
        usage
        return 2
    fi
    [ -n "${AMPLIHACK_RUN_DIR:-}" ] || return 0
    [ "$state" != reuse-clean-worktree ] || op=adopt
    if real="$(cd -P -- "$path" 2>/dev/null && pwd -P)" \
        && "cmd_$op" worktree "$real" "$branch"; then
        return 0
    fi
    printf 'WARNING: could not record %s for cleanup (issue #1808)\n' "$path" >&2
    return 1
}

main() {
    local op="${1:-}"
    case "$op" in
        record) shift; cmd_record "$@" ;;
        adopt) shift; cmd_adopt "$@" ;;
        setup-worktree) shift; cmd_setup_worktree "$@" ;;
        -h|--help|help) usage; return 0 ;;
        *) usage; return 2 ;;
    esac
}

main "$@"
