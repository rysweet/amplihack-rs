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
# them (#2712, #1808).
unset AMPLIHACK_WORK_ROOT AMPLIHACK_WORK_ROOT_EXPLICIT AMPLIHACK_RUN_DIR AMPLIHACK_RUN_ID
