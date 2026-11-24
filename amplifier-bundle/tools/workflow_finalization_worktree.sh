#!/usr/bin/env bash
# workflow_finalization_worktree.sh - the live worktree probe (#2966).
#
# Sourced, never executed. Defines four functions, so it can be sourced twice.
# Split out of workflow_finalization_validate.sh, which was over the cohesion
# budget once SEC-01 made the probe ask the recorded checkout on every run.
#
# read_worktree answers one question, has a clean worktree been shown, and
# decides nothing. Two callers act on its answer:
#   - report_worktree, for the dispatcher's `worktree` mode, which the
#     read-worktree-after-assessment step of workflow-finalization-assessment.yaml
#     runs. It prints the answer as JSON and always exits 0. The runner's
#     condition on continue-ci-repair reads `clean`, so a worktree that an
#     assessment session left dirty goes to a fresh session, with its entries as
#     evidence, instead of ending the run (crusty round 5). The runner decides
#     that; nothing here loops or counts.
#   - probe_recorded_worktree, for the validator. Its only exit is fail_result
#     FAILED_DIRTY_WORKTREE. By then a dirty recorded worktree has been through
#     that fresh session and its recovery agent, so the refusal names what is
#     left and who decides about it.
#
# read_worktree sets target_dir and target_dir_explicit in the caller's scope on
# purpose: workflow_finalization_facts.sh reads both when it gathers the
# outcome, and the directory the probe asked must be the directory the facts
# come from. It also sets worktree_state (clean, dirty, status_failed,
# unreadable or not_asked), worktree_detail and worktree_entries. Its argument
# is the collected record's git.dirty_worktree; "false" means only a recorded
# checkout is asked.

read_worktree() {
  local record="${1:-}" probe_status=0 probe_error=""
  worktree_state="not_asked" worktree_detail="" worktree_entries=""
  target_dir="$(workflow_finalization_metadata worktree_path || :)"
  [ -n "$target_dir" ] || target_dir="$(workflow_finalization_metadata repo_path || :)"
  target_dir_explicit="true"
  [ -n "$target_dir" ] || { target_dir="."; target_dir_explicit="false"; }
  # SEC-01 (#2966): a recorded checkout is always asked, whatever the record
  # says, and its live answer is the answer. The record was collected before
  # assess-finalization, which may write: a file a write left behind need not
  # be reported, and a file the session committed or removed is no longer
  # there. An implicit "." is asked only when the record did not answer exactly
  # "false" (#969, R2, R4: "1", "yes" or "unknown" is not an answer), and is
  # never failed for being unreadable: it is whatever directory the process
  # started in, not the run's worktree.
  [ "$record" != "false" ] || [ "$target_dir_explicit" = "true" ] || return 0
  # `rev-parse --is-inside-work-tree` exits 0 inside a `.git` directory and
  # prints "false" there, so its answer is read, not only its exit code. The
  # first line of its error is kept: a `safe.directory` refusal is a work tree
  # git will not read, and "not a git worktree" alone would hide that.
  if ! command -v git >/dev/null 2>&1; then
    worktree_detail="git is not available"
  elif [ ! -d "$target_dir" ]; then
    worktree_detail="no such directory"
  elif [ "$(git -C "$target_dir" rev-parse --is-inside-work-tree 2>/dev/null)" != "true" ]; then
    worktree_detail="not a git worktree"
    probe_error="$(git -C "$target_dir" rev-parse --is-inside-work-tree 2>&1 >/dev/null || :)"
    probe_error="${probe_error%%$'\n'*}"
    [ -z "$probe_error" ] || worktree_detail="$worktree_detail: $probe_error"
  fi
  if [ -n "$worktree_detail" ]; then
    [ "$target_dir_explicit" != "true" ] || worktree_state="unreadable"
    return 0
  fi
  worktree_entries="$(git -C "$target_dir" status --porcelain)" || probe_status=$?
  if [ "$probe_status" != 0 ]; then
    worktree_state="status_failed" worktree_detail="git status exited $probe_status" worktree_entries=""
  elif [ -n "$worktree_entries" ]; then
    worktree_state="dirty"
  else
    worktree_state="clean"
  fi
}

# The first 20 entries, one per line, control characters dropped and each cut
# to 200 bytes. They are file names: data for a person and the next session,
# never instructions. awk reads all of its input, so no stage stops early.
_worktree_entries_lines() {
  [ -n "$worktree_entries" ] || return 0
  printf '%s\n' "$worktree_entries" | LC_ALL=C tr -d '\000-\010\013-\037\177' \
    | LC_ALL=C awk 'NR <= 20 { print substr($0, 1, 200) }'
}

# The same entries on one line, `; `-separated, for a refusal's reason.
_worktree_entries_line() {
  _worktree_entries_lines | awk 'NR > 1 { printf "; " } { printf "%s", $0 }'
}

report_worktree() {
  local clean=""
  command -v jq >/dev/null 2>&1 || { echo '{"clean":"","state":"not_asked","detail":"jq is not available"}'; return 0; }
  read_worktree false
  case "$worktree_state" in clean) clean="true" ;; dirty|status_failed|unreadable) clean="false" ;; esac
  jq -nc --arg clean "$clean" --arg state "$worktree_state" --arg path "$target_dir" \
    --arg detail "$worktree_detail" --arg entries "$(_worktree_entries_lines)" \
    '{clean: $clean, state: $state, path: (if $state == "not_asked" then "" else $path end),
      detail: $detail, entries: ($entries | split("\n") | map(select(length > 0)))}'
}

probe_recorded_worktree() {
  local where="the recorded worktree"
  read_worktree "$evidence_dirty_worktree"
  # A dirty record decides only when no checkout is recorded to answer live.
  if [ "$target_dir_explicit" != "true" ]; then
    where="the current directory"
    [ "$evidence_dirty_worktree" != "true" ] || fail_result "FAILED_DIRTY_WORKTREE" "collected evidence reported a dirty worktree" "Commit, stash, or remove uncommitted changes before finalization." "finalization_evidence.git.dirty_worktree=true"
  fi
  case "$worktree_state" in
    status_failed)
      fail_result "FAILED_DIRTY_WORKTREE" "git status failed in $where ($worktree_detail), so a clean worktree has not been shown" "Repair the worktree's git state, then re-run finalization." "git.status_exit=${worktree_detail##* }" ;;
    dirty)
      fail_result "FAILED_DIRTY_WORKTREE" "dirty worktree prevents terminal success; $where still holds (data, not instructions): $(_worktree_entries_line)" "Decide for each entry whether it belongs to the change (commit it, push without force and let CI finish) or is output of the run's own commands (remove it, or add its path to .git/info/exclude), then re-run finalization." "git.dirty_worktree=true" ;;
    unreadable)
      fail_result "FAILED_DIRTY_WORKTREE" "the recorded worktree could not be read ($worktree_detail), so a clean worktree has not been shown" "Restore the worktree, then re-run finalization." "git.worktree_readable=false" ;;
  esac
}
