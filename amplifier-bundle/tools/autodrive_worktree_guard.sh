#!/usr/bin/env bash
# autodrive_worktree_guard.sh — the checks that keep the pull request under
# review from supplying its own reviewer, and keep rounds committing only in
# that pull request's linked worktree (issue #1528).
#
# Phases 2 and 3 work in the PR's linked worktree (`--repo` of
# autodrive_loop.sh, `repo_path` of a round). Three things must still come from
# OUTSIDE that tree, because the branch under review would otherwise judge
# itself, and a round's fix-up commit could rewrite the next round's reviewer:
#
#   - the round and evaluator recipes. `recipe run <name> --working-dir DIR`
#     looks the name up in DIR's own amplifier-bundle/recipes first
#     (recipe_search_dirs, resolve.rs), so the loop runs them by absolute path
#     in the bundle that holds this script. A recipe run by path searches its
#     own directory first for sub-recipes (build_search_dirs, run/mod.rs), so
#     loop-evidence-collector and autodrive-merge-evidence come from there too;
#   - the judging agents' working directory. The agent CLI loads AGENTS.md,
#     CLAUDE.md and .claude hooks from it, so the crusty review, the
#     merge-ready assessment and the loop-health analyzer start in the review
#     dir (the main checkout of the preflight repo's repository), never in or under
#     the worktree;
#   - these tools. autodrive_loop.sh hands its own directory to each round as
#     `autodrive_launch_tools`; a round never looks for this file in the tree
#     it judges.
#
# Usage (executed, never sourced):
#   autodrive_worktree_guard.sh review-dir <repo-path>
#       The loops' step-02: print the real path of the MAIN checkout of
#       <repo-path>'s repository (workflow_worktree_root.sh root, beside this
#       script). It never comes from the step's working directory, because
#       step-02 enters repo_path first, and repo_path may itself be the PR
#       worktree (an operator passes `-c repo_path=<worktree>`, or runs
#       `-c repo_path=.` from inside it). A linked worktree is never the main
#       checkout, so the review dir is never the tree under review. Exit 1 with
#       a STOP line when <repo-path> is not in a git work tree, or the answer
#       is not that repository's main checkout (see "The review dir" below):
#       for a bare repository the resolver answers the directory beside the
#       bare git dir, which is no checkout at all.
#   autodrive_worktree_guard.sh loop-preflight <loop> <repo> <review-dir> <round-recipe> [<context-key>]
#       autodrive_loop.sh's check, before round 1. On success prints three
#       lines: the review dir's real path, the round recipe's absolute path in
#       the launch-side bundle, and loop-health-evaluator's (an empty third
#       line when it does not resolve; the loop reports that as
#       MISSING_DEPENDENCY). Otherwise exit 1 with one JSON line on stdout,
#       loop_result UNTRUSTED_REVIEW_DIR or UNTRUSTED_RECIPE, when:
#         - <context-key> is given: review_dir or autodrive_launch_tools came
#           in as a round --context, and only the loop sets those;
#         - <review-dir> (default: the current directory) is not a directory,
#           is <repo> or inside it, or is not the main checkout of <repo>'s
#           repository (see "The review dir" below);
#         - <round-recipe> is a path, is not in the bundle beside this script,
#           or that bundle lies inside <repo> (this script was itself picked
#           up from the tree under review).
#   autodrive_worktree_guard.sh round-guard
#       A round's step-00: the review-dir check on $REVIEW_DIR and $REPO_PATH,
#       with an empty $REVIEW_DIR refused (there is no default; "." would be
#       the worktree). Exit 1 with a STOP line. On success prints a stamp of
#       the review dir's index and HEAD (index-stamp) for index-check.
#       Before stamping, it refuses a review dir whose index holds a run
#       worktree as a gitlink (one under worktrees/, or any registered
#       worktree of the repository), staged or committed: the rule
#       `amplihack hygiene artifact-guard` calls run-worktree-gitlink. A
#       stamp records the main checkout as found, so without this a gitlink
#       an earlier run left staged would become the round's baseline and
#       every later check would pass against it.
#   autodrive_worktree_guard.sh index-stamp <dir>
#       Print one hash of <dir>'s HEAD, current branch and staged entries
#       (`git ls-files --stage`). It reads; it writes no object.
#   autodrive_worktree_guard.sh index-check <dir> <stamp> <step>
#       After an agent step: exit 1 with a STOP line naming <step> unless
#       index-stamp <dir> still prints <stamp>. The rounds run it after the
#       judging step (step-02b). The loop stamps the review dir before each
#       round and runs it after the whole round, whatever the round's exit
#       status (so step-04, the agent that stages and commits, is covered even
#       when it fails or dies), and again after loop-health-evaluator; a
#       change there stops the loop. All are full-tool agents kept out of the
#       main checkout by their prompts alone, and #1528 was an agent staging a
#       forced gitlink there. A change is caught by the round that made it,
#       not by the next human commit there. A person committing in the main
#       checkout meanwhile trips it too; the STOP line says so. Exit 3, not 1,
#       when <dir> cannot be stamped again: nothing was compared, so no change
#       is claimed.
#   autodrive_worktree_guard.sh commit-dir <pr>
#       The round commit block's check, run in the block's shell after it
#       enters repo_path: exit 1 with a STOP line unless the current directory
#       is the top of a LINKED worktree (its git dir differs from the absolute
#       common dir; never the main checkout) on PR <pr>'s head branch.
#
# The review dir is the main checkout of the given repo's repository: the top of a
# work tree whose git dir IS the common dir, and whose common dir resolves to
# the same real path as that repo's. Not "outside the worktree" alone: a
# directory in another repository would have its judging agent load that
# repository's AGENTS.md and read the other repository's PR of the same number
# with `gh pr diff N`, and a bare repository's neighbour directory is no
# checkout at all.
#
# Every refusal names #1528. Exit 2 is a usage error, exit 3 an index-check
# that could not stamp again. Nothing here writes.

set -uo pipefail
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
# As autodrive_pr_worktree.sh does: an inherited repository-selecting variable
# (a hook runs with GIT_DIR set, githooks(5)) would answer every git call below
# for another repository, whatever directory it is run in.
unset GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR \
    GIT_DIR GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_INDEX_FILE GIT_INDEX_VERSION \
    GIT_NAMESPACE GIT_OBJECT_DIRECTORY GIT_PREFIX GIT_QUARANTINE_PATH \
    GIT_WORK_TREE CDPATH

stop() { echo "STOP: $* (#1528)" >&2; exit 1; }
real() { [ -n "${1:-}" ] && (cd -- "$1" 2>/dev/null && pwd -P); }
# repo_of <dir>: the real path of <dir>'s repository (its common git dir).
repo_of() { real "$(git -C "$1" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)"; }

# not_main_checkout <review-dir-real> <repo>: print why <review-dir-real> is
# not the main checkout of <repo>'s repository; print nothing when it is.
not_main_checkout() {
  local rd="$1" want have top
  want="$(repo_of "$2")" || want=""
  [ -n "$want" ] || { echo "repo_path '$2' is not in a git work tree"; return; }
  top="$(git -C "$rd" rev-parse --show-toplevel 2>/dev/null)" && [ "$(real "$top")" = "$rd" ] \
    || { echo "review dir ${rd} is not the top of a git checkout; a bare repository has no main checkout to start the judging agents in"; return; }
  have="$(repo_of "$rd")" || have=""
  [ -n "$have" ] && [ "$have" = "$want" ] \
    || { echo "review dir ${rd} belongs to another repository (${have:-unknown}), not repo_path's (${want}); its gh pr diff would read another pull request"; return; }
  [ "$(real "$(git -C "$rd" rev-parse --absolute-git-dir 2>/dev/null)")" = "$have" ] \
    || echo "review dir ${rd} is a linked worktree of ${want}, not its main checkout"
}

# run_worktree_gitlinks <dir>: print each gitlink in <dir>'s index that is a
# run worktree, as artifact_guard/gitlink.rs decides it: the first path
# component is "worktrees" (any case), or the path is a registered worktree.
run_worktree_gitlinks() {
  local dir="$1" wts entry path first r
  wts="$(git -C "$dir" worktree list --porcelain | sed -n 's/^worktree //p' \
    | while IFS= read -r p; do real "$p"; done)" || return 1
  git -C "$dir" ls-files --stage -z | while IFS= read -r -d '' entry; do
    case "$entry" in 160000\ *) ;; *) continue ;; esac
    path="${entry#*$'\t'}"; first="$(printf '%s' "${path%%/*}" | tr '[:upper:]' '[:lower:]')"
    r="$(real "${dir}/${path}")" || r=""
    if [ "$first" = worktrees ] || { [ -n "$r" ] && printf '%s\n' "$wts" | grep -qxF -- "$r"; }; then
      printf '%s\n' "$path"
    fi
  done
}

cmd_index_stamp() {
  local dir="${1:-}"
  [ -n "$dir" ] && git -C "$dir" rev-parse --is-inside-work-tree >/dev/null 2>&1 || stop "index-stamp: '${dir}' is not a git work tree"
  # HEAD and its branch may legitimately be absent (unborn, detached); the
  # staged entries may not, so an unreadable index (pipefail) fails the stamp
  # rather than hashing to a value that would later read as a change.
  { git -C "$dir" rev-parse -q --verify HEAD; git -C "$dir" symbolic-ref -q HEAD; git -C "$dir" ls-files --stage -z; } 2>/dev/null \
    | git hash-object --stdin || stop "index-stamp: git could not read the index of '${dir}' (a lock, a corrupt index or another git error)"
}

# inside <path> <tree>: <path> is <tree> or lies under it (both real paths).
inside() { [ -n "$2" ] && case "$1/" in "$2"/*) return 0 ;; esac; return 1; }

# launch_recipe <name> <repo-real>: stdout the launch-side path, or fail.
launch_recipe() {
  local name="$1" bundle path
  case "$name" in
    ''|*/*|*.yaml|*.yml)
      echo "ERROR: recipe '${name}' must be a bare name; it is resolved in the launch-side bundle, never by path (#1528)." >&2
      return 1 ;;
  esac
  bundle="$(real "$(dirname -- "${BASH_SOURCE[0]}")/../recipes")"
  [ -n "$bundle" ] || { echo "ERROR: no amplifier-bundle/recipes next to $(dirname -- "${BASH_SOURCE[0]}") (#1528)." >&2; return 1; }
  path="${bundle}/${name}.yaml"
  [ -f "$path" ] || { echo "ERROR: required recipe '${name}' is not in the launch-side bundle (${bundle}) (#1528)." >&2; return 1; }
  if inside "$path" "$2"; then
    echo "ERROR: recipe '${name}' resolved to ${path}, inside the tree under review ($2). The reviewed tree never supplies its own reviewer or stop rule (#1528)." >&2
    return 1
  fi
  printf '%s\n' "$path"
}

cmd_loop_preflight() {
  local loop="${1:-}" repo="${2:-.}" dir="${3:-}" round="${4:-}" key="${5:-}" repo_real rd round_path why
  refuse() { # refuse <loop_result> <reason>
    echo "ERROR: loop '${loop}': $2. The reviewed tree never supplies its reviewer's instructions (#1528)." >&2
    printf '{"loop":"%s","loop_result":"%s","round_label":"preflight","reason":"%s"}\n' "$loop" "$1" "$(printf '%s' "$2" | tr -d '"\\')"
    exit 1
  }
  case "$key" in
    '') ;;
    review_dir) refuse UNTRUSTED_REVIEW_DIR "review_dir is set only by --review-dir, never by --context" ;;
    *) refuse UNTRUSTED_RECIPE "${key} is set only by the loop, never by --context" ;;
  esac
  repo_real="$(real "$repo")"
  [ -n "$dir" ] || dir="$(pwd -P)"
  rd="$(real "$dir")"
  [ -n "$rd" ] || refuse UNTRUSTED_REVIEW_DIR "review dir '${dir}' is not a directory"
  inside "$rd" "$repo_real" \
    && refuse UNTRUSTED_REVIEW_DIR "review dir ${rd} is the tree under review (${repo_real}) or inside it; pass --review-dir the main checkout (the loop recipes derive it from the preflight repo with review-dir), and keep the pull request in a linked worktree, never the main checkout"
  why="$(not_main_checkout "$rd" "$repo")"
  [ -z "$why" ] || refuse UNTRUSTED_REVIEW_DIR "${why}; pass --review-dir the main checkout of the pull request's repository"
  round_path="$(launch_recipe "$round" "$repo_real")" \
    || refuse UNTRUSTED_RECIPE "round recipe ${round} does not resolve in the launch-side bundle"
  printf '%s\n%s\n%s\n' "$rd" "$round_path" "$(launch_recipe loop-health-evaluator "$repo_real" 2>/dev/null)"
}

cmd_review_dir() {
  local repo="${1:-}" root why
  [ -n "$repo" ] || stop "review-dir needs the preflight repo"
  root="$(bash "$(dirname -- "${BASH_SOURCE[0]}")/workflow_worktree_root.sh" root "$repo")" && root="$(real "$root")" \
    && [ -n "$root" ] || stop "cannot resolve the main checkout of the repository holding '${repo}'; the review dir is never taken from the working directory"
  why="$(not_main_checkout "$root" "$repo")"
  [ -z "$why" ] || stop "${why}; auto-drive's judging agents start only in the main checkout of the pull request's repository"
  printf '%s\n' "$root"
}

cmd_round_guard() {
  local rd repo_real links
  [ -n "${REVIEW_DIR:-}" ] || stop "review_dir is required; the judging agent never starts in the tree under review"
  rd="$(real "$REVIEW_DIR")" || rd=""
  [ -n "$rd" ] || stop "review_dir '${REVIEW_DIR}' is not a directory"
  repo_real="$(real "${REPO_PATH:-.}")" || repo_real=""
  [ -n "$repo_real" ] || stop "cannot enter repo_path '${REPO_PATH:-.}'"
  inside "$rd" "$repo_real" \
    && stop "review_dir ${rd} is the tree under review (${repo_real}) or inside it; the reviewed tree never instructs its reviewer"
  why="$(not_main_checkout "$rd" "$repo_real")"
  [ -z "$why" ] || stop "$why"
  links="$(run_worktree_gitlinks "$rd")" \
    || stop "could not read the index or worktree list of the review dir ${rd}, so it was not checked for a staged run worktree"
  [ -z "$links" ] || stop "the index of the review dir ${rd} records a run worktree as a gitlink ($(printf '%s' "$links" | tr '\n' ' ')). An earlier step staged it there; no round starts from that state. Drop it from the index only (git -C '${rd}' rm --cached -f -- <path>; the worktree and its files stay) and run auto-drive again"
  echo "INFO: judging agent starts in ${rd}; commits happen only in ${repo_real}" >&2
  cmd_index_stamp "$rd"
}

cmd_index_check() {
  local dir="${1:-}" want="${2:-}" step="${3:-the judging step}" now
  [ -n "$want" ] || stop "${step}: no index stamp of the review dir was taken before it ran; nothing shows its index is unchanged"
  now="$(cmd_index_stamp "$dir")" || { echo "STOP: ${step}: the review dir ${dir} could not be stamped again, so its index was not compared and no change is claimed; fix the git error above and run the round again (#1528)" >&2; exit 3; }
  [ "$now" = "$want" ] || stop "${step} changed the index or HEAD of the review dir ${dir} (git -C '${dir}' status; git -C '${dir}' reflog -n 5). Auto-drive agents never stage or commit there; undo the change there before another round. If a person committed there meanwhile, run the round again"
}

cmd_commit_dir() {
  local pr="${1:-}" gd cd pr_head
  case "$pr" in ''|*[!0-9]*) stop "commit-dir needs the PR number" ;; esac
  # In the main checkout the git dir and the common dir are the same directory.
  gd="$(git rev-parse --absolute-git-dir)" && cd="$(git rev-parse --path-format=absolute --git-common-dir)" \
    && [ "$(git rev-parse --show-toplevel)" = "$(pwd -P)" ] \
    && case "$cd" in /*) [ "$gd" != "$cd" ] ;; *) false ;; esac \
    || stop "not a linked worktree; never stage or commit in the main checkout or another checkout"
  # An empty answer never matches.
  pr_head="$(gh pr view "$pr" --json headRefName --jq .headRefName)" && [ -n "$pr_head" ] \
    && [ "$(git branch --show-current)" = "$pr_head" ] \
    || stop "this worktree is not on the PR head branch"
}

sub="${1:-}"; [ $# -gt 0 ] && shift
case "$sub" in
  loop-preflight) cmd_loop_preflight "$@" ;;
  review-dir) cmd_review_dir "$@" ;;
  round-guard) cmd_round_guard ;;
  index-stamp) cmd_index_stamp "$@" ;;
  index-check) cmd_index_check "$@" ;;
  commit-dir) cmd_commit_dir "$@" ;;
  *) echo "usage: autodrive_worktree_guard.sh loop-preflight <loop> <repo> <review-dir> <round-recipe> [<context-key>] | review-dir <repo-path> | round-guard | index-stamp <dir> | index-check <dir> <stamp> <step> | commit-dir <pr>" >&2; exit 2 ;;
esac
