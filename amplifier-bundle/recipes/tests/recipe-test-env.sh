# shellcheck shell=bash
# recipe-test-env.sh — sourced (never run) by the recipe shell test suites.
#
# No shebang and no execute bit, deliberately: unlike the sibling fixture
# expected-checks-fixtures.sh, which keeps a shebang while also being
# source-only, this file carries nothing that would read as a runnable suite.
#
# A recipe run exports AMPLIHACK_WORK_ROOT and AMPLIHACK_WORK_ROOT_EXPLICIT=1,
# and every process it starts inherits them. workflow_worktree_root.sh base
# then puts a suite's worktrees under <work root>/worktrees/<name>-<hash8>
# instead of under the suite's own scratch, where its EXIT trap would remove
# them (#2712, #1808). Those two names are this file's own contract.
#
# The names that identify the outer run are not restated here.
# amplifier-bundle/tools/git-repo-env.sh owns that list as
# AMPLIHACK_RUN_IDENTITY_VARS, keeps it in step with the Rust list through
# tests/issue_1278_git_env_isolation.sh, and documents which names are
# excluded and why (#2033, #1984). Sourcing it and calling
# amplihack_clear_run_identity_env clears all five, so AMPLIHACK_RUN_DIR and
# AMPLIHACK_RUN_ID cannot drift from AMPLIHACK_RECIPE_RUN_ID,
# AMPLIHACK_SESSION_TREE_DIR and AMPLIHACK_TREE_ID. Sourcing both files twice
# is safe and prints nothing.
_amplihack_test_env_git_env="$(dirname -- "${BASH_SOURCE[0]}")/../../tools/git-repo-env.sh"
[ -f "${_amplihack_test_env_git_env}" ] || {
    echo "HARNESS-ERROR: missing ${_amplihack_test_env_git_env}" >&2
    return 1
}
# shellcheck source-path=SCRIPTDIR
# shellcheck source=../../tools/git-repo-env.sh
. "${_amplihack_test_env_git_env}"
unset _amplihack_test_env_git_env
amplihack_clear_run_identity_env
unset AMPLIHACK_WORK_ROOT AMPLIHACK_WORK_ROOT_EXPLICIT
