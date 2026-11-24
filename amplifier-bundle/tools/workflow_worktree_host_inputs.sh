#!/usr/bin/env bash
# workflow_worktree_host_inputs.sh: make the launch checkout's git-ignored
# Python environment folders available in a run's new worktree (#3102).
#
# A repository's hooks and checks often call a repo-local `.venv`, for example
# `.venv/bin/python`. The folder is git-ignored, so `git worktree add` does not
# copy it, and a run's commit in its new worktree then fails the hooks with
# "Executable `.venv/bin/python` not found". The worktree belongs to the same
# run as the launch checkout, so a symbolic link is enough.
#
#   workflow_worktree_host_inputs.sh link <launch> <worktree>
#   workflow_worktree_host_inputs.sh facts <launch> <worktree>
#
# Which names are considered: top-level names in <launch> that match
# ^\.venv(-[A-Za-z0-9._-]+)?$, are folders or symbolic links to folders, and
# are not tracked by git. A real folder must be reported ignored by
# `git -C <launch> check-ignore`. A symbolic link is never reported ignored by
# a rule written `.venv/`, because git applies such a rule to real folders
# only, so a link qualifies on what it points at instead: a folder inside the
# launch checkout that git ignores, or a folder outside it that holds
# `pyvenv.cfg` (a Python virtual environment). Anything else outside, such as
# a link to ~/.ssh, is skipped with a WARNING. Nothing else is linked: no
# node_modules and no caches.
#
# link: for each such name, <worktree>/<name> becomes a symbolic link to
# <launch>/<name>, only where nothing exists at that name; a folder, file or
# link already there is never replaced. A link git does not ignore in the
# worktree gets the line `/<name>` in the file `git rev-parse --git-path
# info/exclude` names (the common git folder for a linked worktree), unless
# that exact line is already there; if the name still is not ignored, the
# link is removed again, so a link is never staged or committed. When
# <launch> and <worktree> are the same folder, nothing is done. Stdout stays
# empty; stderr gets one line per link made and one WARNING per failure.
#
# facts: one JSON line on stdout:
#   {"launch":[names that qualify],
#    "worktree":[{"name":..., "kind":"dir|link|missing|other", "target":<link target or null>}],
#    "missing":[qualifying names with nothing at that name in the worktree]}
#
# Exit: 0, also when a link could not be made (the WARNING says why); 2 for a
# usage error (wrong argument count, unknown subcommand, or a path that is not
# an existing folder). No network, no timer, no retry.
# docs/reference/build-goal-loop.md#host-inputs.

set -u
export GIT_PAGER=cat PAGER=cat GIT_TERMINAL_PROMPT=0

PROG="workflow_worktree_host_inputs"
usage() {
  echo "usage: workflow_worktree_host_inputs.sh link|facts <launch checkout> <worktree>" >&2
  [ -z "${1:-}" ] || echo "ERROR: ${PROG}: $1" >&2
  exit 2
}
warn() { printf 'WARNING: %s: %s\n' "$PROG" "$*" >&2; }

[ "$#" -eq 3 ] || usage "expected a subcommand and two folders"
SUB="$1"
case "$SUB" in link|facts) ;; *) usage "unknown subcommand" ;; esac
[ -d "$2" ] || usage "the launch checkout is not an existing folder"
[ -d "$3" ] || usage "the worktree is not an existing folder"
LAUNCH="$(cd -P -- "$2" && pwd -P)" || usage "the launch checkout cannot be entered"
WT="$(cd -P -- "$3" && pwd -P)" || usage "the worktree cannot be entered"
command -v git >/dev/null 2>&1 || { warn "git is not on PATH; nothing was linked"; [ "$SUB" = link ] && exit 0; }

# qualifies <name>: 0 when <launch>/<name> is a Python environment folder that
# git does not track and treats as ignored (see the header).
qualifies() {
  local name="$1" p="$LAUNCH/$1" real rel
  [[ "$name" =~ ^\.venv(-[A-Za-z0-9._-]+)?$ ]] || return 1
  [ -d "$p" ] || return 1
  [ -z "$(git -C "$LAUNCH" ls-files -- "$name" 2>/dev/null)" ] || return 1
  if [ ! -L "$p" ]; then
    git -C "$LAUNCH" check-ignore -q -- "$name" 2>/dev/null
    return
  fi
  real="$(cd -P -- "$p" 2>/dev/null && pwd -P)" || return 1
  case "$real" in
    "$LAUNCH"/*)
      rel="${real#"$LAUNCH"/}"
      git -C "$LAUNCH" check-ignore -q -- "$rel" 2>/dev/null ;;
    *)
      [ -f "$real/pyvenv.cfg" ] && return 0
      [ "$SUB" != link ] || warn "${name} in the launch checkout links outside it to a folder with no pyvenv.cfg, so it is not a Python environment and was not linked"
      return 1 ;;
  esac
}

# The candidates, in a fixed order.
NAMES=()
shopt -s nullglob
for p in "$LAUNCH"/.venv "$LAUNCH"/.venv-*; do
  [ -e "$p" ] || [ -L "$p" ] || continue
  n="${p##*/}"
  qualifies "$n" && NAMES+=("$n")
done
shopt -u nullglob

if [ "$SUB" = facts ]; then
  command -v jq >/dev/null 2>&1 || { echo "ERROR: ${PROG}: jq is not on PATH" >&2; exit 1; }
  ENTRIES='[]'
  for n in ${NAMES[@]+"${NAMES[@]}"}; do
    q="$WT/$n"; kind=other; target=""
    if [ -L "$q" ]; then kind="link"; target="$(readlink -- "$q" 2>/dev/null)"
    elif [ -d "$q" ]; then kind=dir
    elif [ ! -e "$q" ]; then kind=missing
    fi
    ENTRIES="$(jq -c --arg n "$n" --arg k "$kind" --arg t "$target" \
      '. + [{name:$n, kind:$k, target:(if $k == "link" then $t else null end)}]' <<<"$ENTRIES")"
  done
  printf '%s\n' "${NAMES[@]+"${NAMES[@]}"}" | jq -R -s -c --argjson w "$ENTRIES" \
    '{launch:(split("\n") | map(select(length > 0))), worktree:$w, missing:[$w[] | select(.kind == "missing") | .name]}'
  exit 0
fi

# --- link -------------------------------------------------------------------
[ "$LAUNCH" != "$WT" ] || exit 0
EXCLUDE=""
exclude_file() {
  [ -z "$EXCLUDE" ] || { printf '%s' "$EXCLUDE"; return 0; }
  local f
  f="$(git -C "$WT" rev-parse --git-path info/exclude 2>/dev/null)" && [ -n "$f" ] || return 1
  case "$f" in /*) ;; *) f="$WT/$f" ;; esac
  EXCLUDE="$f"; printf '%s' "$f"
}
# ensure_ignored <name>: git ignores <worktree>/<name>, through info/exclude
# when needed. Returns 1 when it cannot be made so.
ensure_ignored() {
  local name="$1" f
  git -C "$WT" check-ignore -q -- "$name" 2>/dev/null && return 0
  f="$(exclude_file)" || { warn "the worktree's info/exclude file could not be found"; return 1; }
  mkdir -p -- "${f%/*}" 2>/dev/null || { warn "the folder of ${f} could not be made"; return 1; }
  if ! { [ -f "$f" ] && grep -Fxq -- "/${name}" "$f"; }; then
    printf '/%s\n' "$name" >> "$f" 2>/dev/null || { warn "/${name} could not be added to ${f}"; return 1; }
  fi
  git -C "$WT" check-ignore -q -- "$name" 2>/dev/null || { warn "git still does not ignore ${name} in the worktree after /${name} was added to ${f}"; return 1; }
}

for n in ${NAMES[@]+"${NAMES[@]}"}; do
  q="$WT/$n"
  if [ -L "$q" ]; then
    # A link this tool made earlier only has its ignore entry checked.
    [ "$(readlink -- "$q" 2>/dev/null)" = "$LAUNCH/$n" ] && { ensure_ignored "$n" || warn "${q} links to ${LAUNCH}/${n} but git may stage it"; }
    continue
  fi
  [ ! -e "$q" ] || continue
  if ! ln -s -- "$LAUNCH/$n" "$q" 2>/dev/null; then
    warn "${q} could not be linked to ${LAUNCH}/${n}"
    continue
  fi
  if ! ensure_ignored "$n"; then
    rm -f -- "$q"
    warn "${q} was not kept, because git would not ignore it and the link must never be committed"
    continue
  fi
  printf '%s: linked %s to %s, the launch checkout'"'"'s git-ignored environment folder (#3102)\n' "$PROG" "$q" "$LAUNCH/$n" >&2
done
exit 0
