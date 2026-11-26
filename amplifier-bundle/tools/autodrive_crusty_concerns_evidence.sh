#!/usr/bin/env bash
# autodrive_crusty_concerns_evidence.sh — the body of `step-05-verify-concerns-addressed` in amplifier-bundle/recipes/autodrive-crusty-round.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The step runs it with `bash`, so it reads the
# environment the recipe runner exported and writes the stdout the step's
# `output:` captures. It is a file rather than an inline block so the recipe
# stays under its line budget; compressing the recipe instead is the one thing
# the brick rule forbids.
#
# The head and the commit count are measured in `repo_path`, the PR's linked
# worktree where step-04 commits, and never in the folder the step started in
# or in `REPO_PATH` (#2519).
#
# The base is `reviewed_head_sha`, the commit the loop measured with
# `git rev-parse` before the round started (autodrive_pr_goal.sh, crusty_round),
# as the round record uses it (#2814). The round context's agent-written
# `head_sha` is read only when no such measurement reached this step, and then
# it must be a hex commit id, never a git expression such as HEAD~1.
#
# When the measurement cannot be made, the step exits 1 with a
# `step05_refusal=<reason>` line and a `STOP: step-05` line on stderr and nothing
# on stdout, rather than an unmeasured `commits: 0`. The reason codes are
# stable; the prose after them may be reworded. Warnings use the same form,
# `step05_warning=<reason>`: `no-commit` when the count is 0, and
# `head-not-descendant` when HEAD does not descend from the base, which the JSON
# also reports as `head_descends_from_base`.
# A failed step fails the round, and a failed round does not end the run: the
# goal loop's `act review` (autodrive_pr_goal.sh, do_act_review) records the
# turn as `review-failed` with the round's whole output, this STOP line
# included, in the turn folder's review.log. The next turn's agent reads that
# result as `previous` and judges the cause (docs/reference/autodrive-goal-loop.md,
# "Failures inside a turn"). Test:
# step05_stop_reaches_the_next_turn_as_a_failed_review in
# crates/amplihack-cli/tests/issue_2519_crusty_step05_worktree.rs.

set -uo pipefail
TOOLS="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
START="$(pwd -P)" RP="${RECIPE_VAR_repo_path:-}" PR="${RECIPE_VAR_pr_number:-}"
WHERE="(step started in $START); commit evidence is not measured in any other checkout"
die() { local r="$1"; shift; printf 'step05_refusal=%s\n' "$r" >&2; echo "STOP: step-05 $* (#2519, #1528)" >&2; exit 1; }
[ -n "$RP" ] || die no-repo-path "has no repo_path $WHERE"
case "$PR" in ''|*[!0-9]*) die no-pr-number "has no numeric pr_number for repo_path $RP $WHERE" ;; esac
cd -- "$RP" 2>/dev/null || die repo-path-unreachable "cannot enter repo_path $RP $WHERE"
unset GH_REPO
# shellcheck source=amplifier-bundle/tools/git-repo-env.sh
if ! . "$TOOLS/git-repo-env.sh"; then die helper-missing "cannot load git-repo-env.sh from $TOOLS $WHERE"; fi
amplihack_clear_git_repo_env
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die not-a-git-repo "requires a git repo at repo_path $RP; git init or clone it and rerun from a checkout $WHERE"
bash "$TOOLS/autodrive_worktree_guard.sh" commit-dir "$PR" \
  || die not-pr-worktree "repo_path $RP is not PR #${PR}'s linked worktree on its head branch $WHERE"
HEAD_IN="${RECIPE_VAR_reviewed_head_sha:-}" SRC="loop-measured reviewed_head_sha"
case "${#HEAD_IN}:$HEAD_IN" in
  40:*[!0-9a-f]*|64:*[!0-9a-f]*) HEAD_IN="" ;;
  40:*|64:*) ;;
  *) HEAD_IN="" ;;
esac
if [ -z "$HEAD_IN" ]; then
  SRC="round context head_sha"
  HEAD_IN="$(printf '%s' "${RECIPE_VAR_crusty_round_context:-}" \
    | amplihack orch helper extract-json --require-field head_sha \
    | amplihack orch helper extract-field --field head_sha --default '')"
  # The reviewed head is a commit id, never a git expression such as HEAD~1.
  case "$HEAD_IN" in
    ''|*[!0-9a-fA-F]*) die no-usable-base "no loop-measured reviewed_head_sha, and round context head_sha '${HEAD_IN//[![:print:]]/?}' is not a hex commit id (repo_path $RP)" ;;
  esac
fi
BASE="$(git rev-parse --verify --quiet "${HEAD_IN}^{commit}")" || die base-not-a-commit "$SRC $HEAD_IN is not a commit in repo_path $RP"
NOW="$(git rev-parse --verify --quiet HEAD)" || die head-unreadable "cannot read HEAD in repo_path $RP"
COMMITS="$(git rev-list --count "${BASE}..HEAD")" || die count-failed "cannot count commits from $BASE in repo_path $RP"
git -c core.fsmonitor=false diff --quiet --no-ext-diff --no-textconv; rc=$?
case $rc in 0) DIRTY=false ;; 1) DIRTY=true ;; *) die worktree-state-unreadable "cannot read worktree state in repo_path $RP (git exit $rc)" ;; esac
git merge-base --is-ancestor "$BASE" HEAD 2>/dev/null; rc=$?
case $rc in 0) DESC=true ;; 1) DESC=false ;; *) die ancestry-unreadable "cannot compare HEAD with $BASE in repo_path $RP (git exit $rc)" ;; esac
if [ "$DESC" = false ]; then
  printf 'step05_warning=head-not-descendant\n' >&2
  echo "WARNING: HEAD $NOW does not descend from the reviewed head $BASE in repo_path $RP; the branch was rewound or replaced." >&2
fi
if [ "$COMMITS" = "0" ]; then
  printf 'step05_warning=no-commit\n' >&2
  echo "WARNING: the round addressed concerns but produced NO commit. That is measured evidence of a round that did nothing; the loop evaluator will see it." >&2
fi
printf '{"base_sha":"%s","head_sha":"%s","commits":%s,"dirty_worktree":"%s","git_observed":"true","head_descends_from_base":"%s"}\n' \
  "$BASE" "$NOW" "$COMMITS" "$DIRTY" "$DESC"
