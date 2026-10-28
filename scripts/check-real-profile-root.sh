#!/usr/bin/env bash
# Issue #1699: notice when a test run changes a profile settings.json under a
# real profile root.
#
#   check-real-profile-root.sh snapshot <snapshot-file>   before the tests
#   check-real-profile-root.sh compare  <snapshot-file>   after them
#
# Roots, from ${GUARD_HOME:-$HOME}, are the union of:
#   - $AMPLIHACK_CLAUDE_PROFILE_ROOT
#   - the first line of $H/.amplihack/profile-root (read, never evaluated)
#   - $H/.amplihack/claude-profiles
#   - $H/.claude-profiles
# Only roots that exist are used; each is resolved through symlinks and a
# root reached twice is counted once. compare watches the roots recorded in
# the snapshot plus any that exist now.
#
# Watched files: <root>/settings.json and <root>/*/settings.json, recorded as
# path, size and SHA-256 (no mtime). Credential files and
# ~/.claude/settings.json are not watched: a live rotator and Claude Code
# change them legitimately.
#
# This script only reads files under the roots; the one file it writes is the
# snapshot, which must be outside every root.
#
# Exit: 0 no change or nothing to check, 1 a settings.json changed, appeared
# or disappeared, 2 bad usage or a snapshot file inside a watched root.
set -euo pipefail
umask 077

NAME=check-real-profile-root
H=${GUARD_HOME:-${HOME:-}}

usage() {
  echo "usage: check-real-profile-root.sh snapshot|compare <snapshot-file>" >&2
  exit 2
}

[ "$#" -eq 2 ] || usage
mode=$1
snap=$2
case "$mode" in snapshot | compare) ;; *) usage ;; esac
[ -n "$snap" ] || usage

# resolve DIR: the physical path of an existing directory, or nothing.
resolve() {
  [ -n "$1" ] && [ -d "$1" ] || return 0
  (cd -P -- "$1" 2>/dev/null && pwd -P) || true
}

# add_root PATH: appends PATH's resolved form to $roots unless already there.
roots=""
add_root() {
  local r line
  r=$(resolve "$1")
  [ -n "$r" ] || return 0
  while IFS= read -r line; do
    [ "$line" = "$r" ] && return 0
  done <<<"$roots"
  if [ -z "$roots" ]; then roots=$r; else roots=$roots$'\n'$r; fi
}

current_roots() {
  local saved=""
  add_root "${AMPLIHACK_CLAUDE_PROFILE_ROOT:-}"
  if [ -n "$H" ] && [ -f "$H/.amplihack/profile-root" ]; then
    IFS= read -r saved <"$H/.amplihack/profile-root" || true
    add_root "$saved"
  fi
  if [ -n "$H" ]; then
    add_root "$H/.amplihack/claude-profiles"
    add_root "$H/.claude-profiles"
  fi
}

hash_of() {
  local h _rest
  if command -v sha256sum >/dev/null 2>&1; then
    read -r h _rest < <(sha256sum <"$1")
  else
    read -r h _rest < <(shasum -a 256 <"$1")
  fi
  printf '%s' "$h"
}

# listing: one "path<TAB>size<TAB>sha256" line per watched file, sorted.
listing() {
  local root f size nl=$'\n' tab=$'\t'
  while IFS= read -r root; do
    [ -n "$root" ] || continue
    while IFS= read -r -d '' f; do
      case "$f" in
        *"$nl"* | *"$tab"*)
          printf '%s: skipping a settings.json whose path holds a tab or newline under "%s"\n' \
            "$NAME" "$root" >&2
          continue
          ;;
      esac
      size=$(wc -c <"$f")
      size=${size//[[:space:]]/}
      printf '%s\t%s\t%s\n' "$f" "$size" "$(hash_of "$f")"
    done < <(find "$root" -mindepth 1 -maxdepth 2 -name settings.json -type f -print0)
  done <<<"$roots" | LC_ALL=C sort
}

if [ "$mode" = snapshot ]; then
  current_roots
  snap_dir=$(dirname -- "$snap")
  snap_real=$(resolve "$snap_dir")
  if [ -z "$snap_real" ]; then
    printf '%s: snapshot directory "%s" does not exist\n' "$NAME" "$snap_dir" >&2
    exit 2
  fi
  while IFS= read -r root; do
    [ -n "$root" ] || continue
    case "$snap_real/" in
      "$root"/*)
        printf '%s: refusing snapshot file "%s" inside watched root "%s"\n' \
          "$NAME" "$snap" "$root" >&2
        exit 2
        ;;
    esac
  done <<<"$roots"
  {
    while IFS= read -r root; do
      if [ -n "$root" ]; then printf 'root\t%s\n' "$root"; fi
    done <<<"$roots"
    if [ -n "$roots" ]; then
      while IFS= read -r line; do
        if [ -n "$line" ]; then printf 'file\t%s\n' "$line"; fi
      done < <(listing)
    fi
  } >"$snap"
  exit 0
fi

# compare
if [ ! -f "$snap" ]; then
  printf '%s: no snapshot file "%s"\n' "$NAME" "$snap" >&2
  exit 2
fi
old_files=""
while IFS=$'\t' read -r kind rest; do
  case "$kind" in
    root) add_root "$rest" ;;
    file) old_files=$old_files$rest$'\n' ;;
  esac
done <"$snap"
current_roots

if [ -z "$roots" ]; then
  echo "$NAME: nothing to check"
  exit 0
fi

new_files=$(listing)
[ -z "$new_files" ] || new_files=$new_files$'\n'

# lookup PATH LIST: the "size<TAB>sha" recorded for PATH in LIST, or nothing.
lookup() {
  local p s h
  while IFS=$'\t' read -r p s h; do
    if [ "$p" = "$1" ]; then
      printf '%s\t%s' "$s" "$h"
      return 0
    fi
  done <<<"$2"
}

changes=0
count=0
while IFS=$'\t' read -r p s h; do
  [ -n "$p" ] || continue
  count=$((count + 1))
  now=$(lookup "$p" "$new_files")
  if [ -z "$now" ]; then
    printf 'REMOVED %s\n' "$p"
    changes=$((changes + 1))
  elif [ "$now" != "$s"$'\t'"$h" ]; then
    printf 'CHANGED %s\n' "$p"
    changes=$((changes + 1))
  fi
done <<<"$old_files"
while IFS=$'\t' read -r p s h; do
  [ -n "$p" ] || continue
  if [ -z "$(lookup "$p" "$old_files")" ]; then
    printf 'ADDED   %s\n' "$p"
    changes=$((changes + 1))
  fi
done <<<"$new_files"

if [ "$changes" -gt 0 ]; then
  exit 1
fi
echo "$NAME: $count settings.json files unchanged"
