#!/usr/bin/env bash
# workflow_finalization_worktree.sh - the live worktree probe (#2966).
#
# Sourced, never executed. Defines one function, so it can be sourced twice.
# Split out of workflow_finalization_validate.sh, which was over the cohesion
# budget once SEC-01 made the probe ask the recorded checkout on every run.
#
# probe_recorded_worktree answers one question: has a clean worktree been
# shown? It decides no terminal state of its own; the only exit it takes is
# fail_result FAILED_DIRTY_WORKTREE, which is a blocker about the local
# checkout and not a verdict about the run.
#
# It sets target_dir and target_dir_explicit in the caller's scope on purpose:
# workflow_finalization_facts.sh reads both when it gathers the outcome, and
# the directory the probe asked must be the directory the facts come from.
# It reads evidence_dirty_worktree, and calls fail_result from
# workflow_finalization_result.sh, so both are required before it runs.

probe_recorded_worktree() {
  local probe_where="" probe_status=0 probe_output="" probe_unreadable=""
  target_dir="$(workflow_finalization_metadata worktree_path || :)"
  [ -n "$target_dir" ] || target_dir="$(workflow_finalization_metadata repo_path || :)"
  target_dir_explicit="true"
  [ -n "$target_dir" ] || { target_dir="."; target_dir_explicit="false"; }
  # Live worktree probe unless the collected record answered the question, and
  # it answers it only by saying exactly "true" (handled above) or exactly
  # "false" (#969, R2, R4). Anything else is not an answer and never stands in
  # for one: empty, "unknown", "yes", "1", a JSON number rendered as "0". R4
  # reproduced the cost of treating one of those as a negative -- a worktree
  # with uncommitted changes whose record read "1" published EXISTING_OPEN_PR
  # -- so a record that does not say true or false sends the question to the
  # worktree, which can always answer it.
  # SEC-01 (#2966): a recorded checkout is always asked, whatever the record
  # says. The record was collected before assess-finalization, which may write,
  # and a file a write left behind need not be reported: the assessment's own
  # `action_taken` decides nothing here. A recorded checkout that is missing or
  # not a work tree, or whose `git status` fails, has not been shown clean, so
  # it fails too. An implicit "." is asked only when the record did not answer,
  # and is never failed for being unreadable: it is whatever directory the
  # process started in, not the run's worktree.
  # `rev-parse --is-inside-work-tree` exits 0 inside a `.git` directory and
  # prints "false" there, so its answer is read, not only its exit code.
  if [ "$evidence_dirty_worktree" != "false" ] || [ "$target_dir_explicit" = "true" ]; then
    probe_unreadable=""
    if ! command -v git >/dev/null 2>&1; then
      probe_unreadable="git is not available"
    elif [ ! -d "$target_dir" ]; then
      probe_unreadable="no such directory"
    elif [ "$(git -C "$target_dir" rev-parse --is-inside-work-tree 2>/dev/null)" != "true" ]; then
      probe_unreadable="not a git worktree"
    fi
    if [ -z "$probe_unreadable" ]; then
      probe_where="the recorded worktree"
      [ "$target_dir_explicit" = "true" ] || probe_where="the current directory"
      probe_status=0
      probe_output="$(git -C "$target_dir" status --porcelain)" || probe_status=$?
      if [ "$probe_status" != 0 ]; then
        fail_result "FAILED_DIRTY_WORKTREE" "git status failed in $probe_where (exit $probe_status), so a clean worktree has not been shown" "Repair the worktree's git state, then re-run finalization." "git.status_exit=$probe_status"
      fi
      if [ -n "$probe_output" ]; then
        fail_result "FAILED_DIRTY_WORKTREE" "dirty worktree prevents terminal success" "Commit, stash, or remove uncommitted changes before finalization." "git.dirty_worktree=true"
      fi
    elif [ "$target_dir_explicit" = "true" ]; then
      fail_result "FAILED_DIRTY_WORKTREE" "the recorded worktree could not be read ($probe_unreadable), so a clean worktree has not been shown" "Restore the worktree, then re-run finalization." "git.worktree_readable=false"
    fi
  fi
}
