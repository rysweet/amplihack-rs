# shellcheck shell=bash
# git-repo-env.sh — the one shell copy of the variables that pick a git
# repository and of the variables that name an outer recipe run (#2033, #1984).
#
# Git reads GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE and the rest before it looks
# at the working directory, and sets several of them for every hook it runs. A
# test suite started with them inherited commits into the repository they
# name, not the checkout under test. Source this file, then call the clear
# functions before the first git, cargo, npm or pytest command:
#
#   . "<tools dir>/git-repo-env.sh"
#   amplihack_clear_git_repo_env       # sets AMPLIHACK_GIT_ENV_CLEARED
#   amplihack_clear_run_identity_env   # sets AMPLIHACK_RUN_ENV_CLEARED
#
# Each *_CLEARED variable holds the names that were set and are now unset, in
# list order, or `none`. Names only, never values. Call the functions directly,
# never inside $(...): a command substitution clears only its own subshell.
#
# The git list matches amplihack_git::REPOSITORY_ENV_VARS
# (crates/amplihack-git/src/lib.rs) name for name and in order; change both in
# one commit. tests/issue_1278_git_env_isolation.sh fails CI when they differ.
# AMPLIHACK_SESSION_DEPTH and AMPLIHACK_MAX_DEPTH are deliberately absent: they
# feed the recursion guard.
#
# Sourcing only defines the names below. Both lists are assigned
# unconditionally so the environment cannot shrink them, both functions are
# redefined over any exported import, and `builtin unset` ignores an exported
# `unset` function. That guards against accidental overrides only: an exported
# `builtin` function or BASH_ENV can still defeat it, and whoever can set those
# can replace git itself. bash 3.2 safe: no arrays, no function-local variables.

AMPLIHACK_GIT_REPO_ENV_VARS="GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR GIT_DIR GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_INDEX_FILE GIT_INDEX_VERSION GIT_NAMESPACE GIT_OBJECT_DIRECTORY GIT_PREFIX GIT_QUARANTINE_PATH GIT_WORK_TREE"
AMPLIHACK_RUN_IDENTITY_VARS="AMPLIHACK_RECIPE_RUN_ID AMPLIHACK_RUN_DIR AMPLIHACK_RUN_ID AMPLIHACK_SESSION_TREE_DIR AMPLIHACK_TREE_ID"

# _amplihack_clear_names <names>: unset each listed name that is set (even to
# the empty string) and leave the cleared names, or `none`, in
# _AMPLIHACK_CLEARED.
_amplihack_clear_names() {
    _AMPLIHACK_CLEARED=""
    for _amplihack_v in $1; do
        if [ -n "${!_amplihack_v+x}" ]; then
            builtin unset "$_amplihack_v"
            _AMPLIHACK_CLEARED="${_AMPLIHACK_CLEARED:+$_AMPLIHACK_CLEARED }$_amplihack_v"
        fi
    done
    builtin unset _amplihack_v
    [ -n "$_AMPLIHACK_CLEARED" ] || _AMPLIHACK_CLEARED="none"
    return 0
}

amplihack_clear_git_repo_env() {
    _amplihack_clear_names "$AMPLIHACK_GIT_REPO_ENV_VARS"
    # shellcheck disable=SC2034 # read by the caller that sourced this file
    AMPLIHACK_GIT_ENV_CLEARED="$_AMPLIHACK_CLEARED"
    builtin unset _AMPLIHACK_CLEARED
    return 0
}

amplihack_clear_run_identity_env() {
    _amplihack_clear_names "$AMPLIHACK_RUN_IDENTITY_VARS"
    # shellcheck disable=SC2034 # read by the caller that sourced this file
    AMPLIHACK_RUN_ENV_CLEARED="$_AMPLIHACK_CLEARED"
    builtin unset _AMPLIHACK_CLEARED
    return 0
}
