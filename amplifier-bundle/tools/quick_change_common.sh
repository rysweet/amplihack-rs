#!/usr/bin/env bash
# quick_change_common.sh - shared preamble for the bash steps of
# quick-change.yaml (issue #1740). Sourced, never executed:
#
#   . <bundle>/amplifier-bundle/tools/quick_change_common.sh
#
# Each step finds this file from AMPLIHACK_HOME, RECIPE_VAR_repo_path or the
# two $HOME install locations, never from the current directory. Sourcing it:
#   - sets `-euo pipefail` and the pager variables;
#   - sets QC_TOOLS to the directory this file is in;
#   - sets QC_WT to this run's worktree, read through worktree_setup_path.sh;
#   - refuses (exit 1) when QC_TOOLS is inside QC_WT, so no tool is ever run
#     from the tree the agent edited (S1);
#   - sets and exports QUICK_CHANGE_STATE_DIR, an absolute directory outside
#     the worktree created with umask 077 (S9).
# The caller sets QC_STEP to its step id first. qc_branch and qc_write are
# defined for the publish steps; qc_base for every step after the agent.

set -euo pipefail
export GIT_PAGER=cat PAGER=cat LESS=FRX

QC_STEP="${QC_STEP:-quick-change}"
QC_TOOLS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

qc_die() {
  echo "ERROR: $QC_STEP: $1" >&2
  exit 1
}

[ -f "$QC_TOOLS/worktree_setup_path.sh" ] || qc_die "worktree_setup_path.sh is missing from $QC_TOOLS"
QC_WT="$(bash "$QC_TOOLS/worktree_setup_path.sh" "$QC_STEP")" || qc_die "no usable worktree in worktree_setup"
case "$QC_TOOLS/" in
  "$QC_WT"/*) qc_die "refusing tools from inside the agent's worktree ($QC_TOOLS)" ;;
esac

# State directory: one per worktree path, under the user's amplihack home.
[ -n "${HOME:-}" ] || qc_die "HOME is unset; cannot place the state directory"
QC_STATE_ROOT="$HOME/.amplihack/state/quick-change"
QUICK_CHANGE_STATE_DIR="$QC_STATE_ROOT/$(printf '%s' "$QC_WT" | cksum | awk '{ print $1 }')"
case "$QUICK_CHANGE_STATE_DIR/" in
  /*) ;;
  *) qc_die "state directory is not absolute" ;;
esac
case "$QUICK_CHANGE_STATE_DIR/" in
  "$QC_WT"/*) qc_die "state directory $QUICK_CHANGE_STATE_DIR is inside the worktree" ;;
esac
( umask 077 && mkdir -p "$QUICK_CHANGE_STATE_DIR" ) || qc_die "cannot create $QUICK_CHANGE_STATE_DIR"
[ -d "$QUICK_CHANGE_STATE_DIR" ] && [ ! -L "$QUICK_CHANGE_STATE_DIR" ] || qc_die "state directory is not a plain directory"
export QUICK_CHANGE_STATE_DIR

# qc_write <name>: stdin to <state dir>/<name> through mktemp and mv, so a
# symlink planted at the target is replaced, not followed.
qc_write() {
  local tmp
  tmp="$(umask 077 && mktemp "$QUICK_CHANGE_STATE_DIR/.$1.XXXXXX")" || qc_die "mktemp failed for $1"
  cat > "$tmp"
  mv -f "$tmp" "$QUICK_CHANGE_STATE_DIR/$1"
}

# qc_base: print the base pinned by quick-change-record-base. The state
# directory is outside the worktree but still writable by the agent and by
# check code, so the stored value must equal the runner's in-memory output
# (RECIPE_VAR_quick_change_base). Step 2 always sets that output, so an
# empty value means it was not passed on, and the step stops (S2).
qc_base() {
  local base
  [ -f "$QUICK_CHANGE_STATE_DIR/base" ] && [ ! -L "$QUICK_CHANGE_STATE_DIR/base" ] || qc_die "pinned base is missing"
  base="$(cat "$QUICK_CHANGE_STATE_DIR/base")"
  [[ "$base" =~ ^[0-9a-f]{40}$ ]] || qc_die "pinned base is not a 40-character hex SHA"
  [ -n "${RECIPE_VAR_quick_change_base:-}" ] || qc_die "runner did not provide quick_change_base"
  [ "$base" = "$RECIPE_VAR_quick_change_base" ] || qc_die "pinned base in the state directory differs from the recorded base; not continuing"
  printf '%s' "$base"
}

# qc_branch: print the worktree's branch after checking that it is valid,
# starts with quick/ and is not a default or protected name (S4).
qc_branch() {
  local branch default
  branch="$(git -C "$QC_WT" symbolic-ref -q --short HEAD)" || qc_die "the worktree is not on a branch"
  git check-ref-format --branch "$branch" >/dev/null 2>&1 || qc_die "branch '$branch' fails check-ref-format"
  case "$branch" in
    quick/*) ;;
    *) qc_die "branch '$branch' does not start with quick/" ;;
  esac
  default="$(git -C "$QC_WT" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  case "$branch" in
    main|master|develop|trunk|"${default#origin/}") qc_die "refusing to publish to protected branch '$branch'" ;;
  esac
  printf '%s' "$branch"
}

# qc_title: the task text as one line with control characters and backticks
# removed and leading markdown markers (#, >, *, _, -) stripped, cut to 72
# characters (S7).
qc_title() {
  local t
  t="$(printf '%s' "${RECIPE_VAR_task_description:-}" | LC_ALL=C tr '[:cntrl:]`' ' ' \
    | sed -E -e 's/^[#>*_[:space:]-]+//' -e 's/  +/ /g' -e 's/ $//')"
  t="${t:0:72}"
  printf '%s' "${t:-quick change}"
}

# qc_request: the task text with control characters other than newline and
# tab removed, cut to 4000 bytes on a UTF-8 boundary (S7).
qc_request() {
  local s
  s="$(printf '%s' "${RECIPE_VAR_task_description:-}" | LC_ALL=C tr -d '\000-\010\013-\037\177')"
  s="$(LC_ALL=C; printf '%s' "${s:0:4000}")"
  # iconv -c drops a character split by the cut and then exits 1; its output
  # is still the wanted text, so only that status is ignored.
  printf '%s' "$s" | { iconv -c -f UTF-8 -t UTF-8 2>/dev/null || true; }
}

# qc_issue: the issue number from issue_number, else the first #N in the
# task text; empty unless it matches ^[1-9][0-9]{0,8}$ (S7).
qc_issue() {
  local n="${RECIPE_VAR_issue_number:-}" re='#([1-9][0-9]{0,8})([^0-9]|$)'
  if [ -z "$n" ] && [[ "${RECIPE_VAR_task_description:-}" =~ $re ]]; then
    n="${BASH_REMATCH[1]}"
  fi
  [[ "$n" =~ ^[1-9][0-9]{0,8}$ ]] && printf '%s' "$n"
  return 0
}
