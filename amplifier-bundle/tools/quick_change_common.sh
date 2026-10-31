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
# defined for the publish steps; qc_base for every step after the agent;
# qc_handover_reason, qc_checks_summary and qc_checks_table read state files
# only after checking them against fixed forms or the runner's outputs;
# qc_handover_note writes the note default-workflow receives on hand-over;
# qc_require_identity runs before the agent; qc_pr_warnings opens the PR body.

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

# qc_handover_reason: the reason from line 1 of the stored diff-limits
# verdict, checked against the fixed list of hand-over reasons, so that text
# written to the state directory never reaches the hand-over prompt.
qc_handover_reason() {
  local file="$QUICK_CHANGE_STATE_DIR/diff-limits" line1 reason
  local re='^(unparseable-path|files>[0-9]{1,6}|lines>[0-9]{1,6}|agent-commit|public-interface|cli-definition|manifest|sensitive-path|mode-or-symlink)$'
  [ -f "$file" ] && [ ! -L "$file" ] || qc_die "stored diff-limits verdict is missing"
  line1="$(sed -n 1p "$file")"
  case "$line1" in
    "QUICK_CHANGE_VERDICT=handover reason="*) reason="${line1#QUICK_CHANGE_VERDICT=handover reason=}" ;;
    *) qc_die "stored diff-limits verdict is not a hand-over" ;;
  esac
  [[ "$reason" =~ $re ]] || qc_die "hand-over reason in the state directory is not a known reason"
  printf '%s' "$reason"
}

# qc_handover_note <reason> <branch> <base> <patch>: the hand-over note,
# fixed wording and these values only. Whether the agent committed is read
# from git, not from the reason: agent-commit always means commits after the
# base, and unparseable-path, files>N and lines>N, which rules 2, 4 and 5
# decide before rule 6, can come with commits too. `git diff <base>` shows
# the whole change in both cases; `git status` misses committed work.
qc_handover_note() {
  local reason="$1" branch="$2" base="$3" patch="$4" commits
  commits="$(git -C "$QC_WT" rev-list --count "$base..HEAD")" || qc_die "git rev-list failed"
  [[ "$commits" =~ ^[0-9]+$ ]] || qc_die "git rev-list printed '$commits'"
  printf '%s\n' "Hand-over from quick-change: the change needs the full workflow (reason=$reason)."
  if [ "$commits" -eq 0 ]; then
    printf '%s\n' "The changes are uncommitted in this worktree on branch $branch."
  else
    printf '%s\n' "The changes are on branch $branch in this worktree: $commits commit(s) after the base commit, and possibly uncommitted changes as well."
  fi
  printf '%s\n' "Base commit: $base" \
    "Patch of the whole change against the base commit: $patch" \
    "Before you start, run \`git diff $base\` to see the whole change, committed and uncommitted; \`git status\` does not show committed changes." \
    "Apply the patch only if that diff is empty: git apply --index $patch" \
    'Continue from these changes; do not start again from the base commit.'
}

# qc_checks_summary: the checks summary from the runner's output of
# quick-change-run-checks (RECIPE_VAR_quick_change_checks), after checking
# its form and that the stored copy is the same. The runner's copy decides
# draft or ready, so a rewritten file cannot turn failed into passed.
qc_checks_summary() {
  local summary="${RECIPE_VAR_quick_change_checks:-}" file="$QUICK_CHANGE_STATE_DIR/checks-summary"
  local re='^QUICK_CHANGE_CHECKS=(passed|failed|none) ran=[0-9]{1,4} failed=[0-9]{1,4} skipped=[0-9]{1,4}$'
  [ -n "$summary" ] || qc_die "runner did not provide quick_change_checks"
  [[ "$summary" =~ $re ]] || qc_die "checks summary is not in the expected form"
  [ -f "$file" ] && [ ! -L "$file" ] && [ "$(cat "$file")" = "$summary" ] \
    || qc_die "checks summary in the state directory differs from the runner's output"
  printf '%s' "$summary"
}

# qc_checks_table <summary>: print the stored checks table after checking
# every row against the form quick_change_checks.sh writes (a code-span label
# with no backtick, pipe or control character, then `exit N` or a skip
# reason from the closed set) and the row counts against <summary>. Any
# other content stops the step, so the PR body gets nothing else from it.
qc_checks_table() {
  local file="$QUICK_CHANGE_STATE_DIR/checks-table.md" line ran=0 failed=0 skipped=0 want
  local skip_re='skipped [(]((cargo|pre-commit|npm|jq) not on PATH|no [.]rs files changed|no "(lint|test)" script)[)]'
  local row_re='^[|] `[^`|[:cntrl:]]*` [|] (exit [0-9]{1,3}|'"$skip_re"') [|]$'
  [ -f "$file" ] && [ ! -L "$file" ] || qc_die "checks table is missing"
  while IFS= read -r line || [ -n "$line" ]; do
    [[ "$line" =~ $row_re ]] || qc_die "checks table row is not in the expected form"
    case "$line" in
      *" | exit 0 |") ran=$((ran + 1)) ;;
      *" | exit "*) ran=$((ran + 1)); failed=$((failed + 1)) ;;
      *) skipped=$((skipped + 1)) ;;
    esac
  done < "$file"
  want="${1#QUICK_CHANGE_CHECKS=* }"
  [ "ran=$ran failed=$failed skipped=$skipped" = "$want" ] || qc_die "checks table does not match the checks summary"
  cat "$file"
}

# qc_pr_warnings <summary> <repaired>: the lines that open the PR body, one
# for each reason the PR is a draft; empty when it can be ready for review.
qc_pr_warnings() {
  case "$1" in
    QUICK_CHANGE_CHECKS=failed*) printf '%s\n\n' 'Checks failed' ;;
    QUICK_CHANGE_CHECKS=none*) printf '%s\n\n' 'No checks ran' ;;
  esac
  [ "$2" = no ] || printf '%s\n' 'Hook repair changed the code after the checks ran; the checks table describes the pre-repair tree.'
}

# qc_require_identity: stop unless a safe commit identity resolves, so a run
# that could never commit ends before the agent and checks do their work. The
# commit step resolves it again and exports it. This resolves in a subshell,
# in the worktree, because the resolver reads the repository's configuration.
qc_require_identity() {
  [ -f "$QC_TOOLS/git-identity.sh" ] || qc_die "git-identity.sh is missing from $QC_TOOLS"
  ( cd "$QC_WT" && . "$QC_TOOLS/git-identity.sh" && amplihack_resolve_git_identity >/dev/null ) \
    || qc_die "no safe commit identity; set one before running, as the commit step would fail after the agent's work"
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
