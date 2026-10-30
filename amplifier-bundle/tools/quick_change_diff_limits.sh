#!/usr/bin/env bash
# quick_change_diff_limits.sh - decide whether a quick-change diff proceeds to
# a PR or is handed over to default-workflow (issue #1740).
#
# Usage:
#   quick_change_diff_limits.sh <worktree> [--base <sha>] [--committed] [--max-files N] [--max-lines N]
#   quick_change_diff_limits.sh --snapshot <worktree>
#
# The verdict is taken from git output alone, never from agent text. Line 1 of
# stdout is `QUICK_CHANGE_VERDICT=<proceed|handover|empty|stop> reason=<r>`;
# line 2 is JSON built with `jq -n`. The rules and their order are fixed by
# docs/reference/quick-change-recipe.md:
#    1 git configuration or hooks differ from the snapshot  stop     exit 4
#    2 a numstat record cannot be parsed                    handover
#    3 no changes                                           empty    exit 3
#    4 files > max-files, 5 lines > max-lines (lockfiles that already existed
#      are excluded from the line count), 6 agent commit, 7 public Rust
#      interface, 8 CLI attribute, 9 manifest, 10 sensitive path,
#      11 mode or symlink change                            handover
# Faults (bad arguments, a base that is not an ancestor of HEAD, a git
# failure, jq missing, a missing snapshot) exit 2.
#
# --committed re-measures BASE..HEAD after hook repair may have committed a
# fixing agent's changes. The worktree must be clean (else exit 2), rule 6
# is skipped because the recipe's own commits are expected, every other rule
# runs, and a handover verdict exits 5: it is too late to hand over, so the
# caller must not publish.
#
# --snapshot prints the git configuration and hooks snapshot that
# quick-change-record-base stores as $QUICK_CHANGE_STATE_DIR/git-config.snapshot.
# Each line is `<name>\t<checksum>`; names never carry configuration values,
# so a drift report can name what changed without printing it.
#
# Written for bash 3.2: no associative arrays, mapfile or ${v,,}.

set -euo pipefail
export GIT_PAGER=cat PAGER=cat LESS=FRX

LOCKFILES=" Cargo.lock package-lock.json yarn.lock pnpm-lock.yaml uv.lock poetry.lock "
PUB_RE='^[[:space:]]*pub[[:space:]]+(async[[:space:]]+)?(unsafe[[:space:]]+)?(fn|struct|enum|trait|type|const|static|mod|use)([^A-Za-z0-9_]|$)'
CLI_RE='#\[(arg|command|clap)'
# Keys that redirect the push or its transport, or run a program during
# git add, commit or push. Global and system values are covered here; the
# local and worktree files are also checksummed whole below.
CONFIG_KEY_RE='^(remote\.|url\.|core\.hookspath$|core\.sshcommand$|core\.fsmonitor$|credential\.|include\.|includeif\.|http\.|core\.gitproxy$|core\.askpass$|core\.attributesfile$|protocol\.|filter\.|gpg\.|commit\.gpgsign$)'

fault() {
  echo "quick_change_diff_limits.sh: $1" >&2
  exit 2
}

# git with fsmonitor, external diff drivers and textconv disabled, so a
# configuration change cannot run code while the diff is measured.
qgit() {
  git -C "$WT" -c core.fsmonitor=false -c core.quotepath=off "$@"
}

cksum_of() {
  cksum | awk '{ print $1 "-" $2 }'
}

# config_group <name> <regex>: one snapshot line for every config entry whose
# key matches <regex>. The checksum covers keys and values; neither is printed.
config_group() {
  printf '%s\t%s\n' "$1" "$(printf '%s\n' "$CONFIG_ENTRIES" | LC_ALL=C grep -E "$2" | LC_ALL=C sort | cksum_of)"
}

snapshot() {
  local hooks entry key value name matched cfg
  CONFIG_ENTRIES=""
  # A failed read must stop the run: two equally empty snapshots would
  # compare equal and hide drift. The list goes through a file because a
  # fault inside a process substitution would only end that subshell.
  cfg="$(mktemp)" || fault "mktemp failed"
  git -C "$WT" config -z --list > "$cfg" || { rm -f "$cfg"; fault "git config --list failed"; }
  # -z: each entry is `key\nvalue\0`, so values with newlines stay whole.
  while IFS= read -r -d '' entry; do
    key="${entry%%$'\n'*}"
    value="${entry#*$'\n'}"
    [ "$value" != "$entry" ] || value=""
    # Match case-insensitively in bash first: most entries do not match, and
    # forking tr per entry made the snapshot take seconds on a long config.
    shopt -s nocasematch
    if [[ "$key" =~ $CONFIG_KEY_RE ]]; then matched=1; else matched=0; fi
    shopt -u nocasematch
    [ "$matched" -eq 1 ] || continue
    key="$(printf '%s' "$key" | tr '[:upper:]' '[:lower:]')"
    CONFIG_ENTRIES="${CONFIG_ENTRIES}${key}=$(printf '%s' "$value" | cksum_of)"$'\n'
  done < "$cfg"
  rm -f "$cfg"
  {
    config_group remote-urls '^remote\.[^=]*\.(url|pushurl)='
    config_group remote-other '^remote\.'
    config_group url-rewrites '^url\.'
    config_group core.hookspath '^core\.hookspath='
    config_group core.sshcommand '^core\.sshcommand='
    config_group core.fsmonitor '^core\.fsmonitor='
    config_group credential '^credential\.'
    config_group include '^(include|includeif)\.'
    config_group http '^http\.'
    config_group transport-programs '^(core\.gitproxy|core\.askpass|protocol\.)'
    config_group filters-attributes '^(filter\.|core\.attributesfile=)'
    config_group signing '^(gpg\.|commit\.gpgsign=)'
    printf 'config-local\t%s\n' "$(git -C "$WT" config --local -z --list 2>/dev/null | cksum_of)"
    printf 'config-worktree\t%s\n' "$(git -C "$WT" config --worktree -z --list 2>/dev/null | cksum_of)"
    printf 'push-url\t%s\n' "$(git -C "$WT" remote get-url --push origin 2>/dev/null | cksum_of)"
    hooks="$(git -C "$WT" rev-parse --path-format=absolute --git-path hooks)" || fault "cannot resolve the hooks directory"
    printf 'hooks-dir\t%s\n' "$(printf '%s' "$hooks" | cksum_of)"
    if [ -d "$hooks" ]; then
      while IFS= read -r -d '' entry; do
        name="hook:$(printf '%s' "${entry#"$hooks"/}" | LC_ALL=C tr -c 'A-Za-z0-9._/-' '_')"
        if [ -L "$entry" ]; then
          value="link:$(readlink "$entry" | cksum_of)"
        else
          value="file:$(cksum_of < "$entry")"
          [ ! -x "$entry" ] || value="${value}:x"
        fi
        printf '%s\t%s\n' "$name" "$value"
      done < <(find "$hooks" -mindepth 1 \( -type f -o -type l \) -print0)
    fi
  } | LC_ALL=C sort
}

# emit <verdict> <reason> <exit> [changed-names]: print both lines and exit.
emit() {
  local changed_json="[]" code="$3"
  if [ -n "${4:-}" ]; then
    changed_json="$(printf '%s' "$4" | jq -R -s -c 'split("\n") | map(select(length > 0))')"
  fi
  if [ "$1" = handover ] && [ "$COMMITTED" -eq 1 ]; then
    echo "quick-change: hook repair grew the change beyond the quick-change limits; not publishing" >&2
    code=5
  fi
  printf 'QUICK_CHANGE_VERDICT=%s reason=%s\n' "$1" "$2"
  jq -n -c --arg verdict "$1" --arg reason "$2" --arg base "$BASE" \
    --argjson files "$FILES" --argjson lines "$LINES" --argjson lockfile_lines "$LOCK_LINES" \
    --argjson max_files "$MAX_FILES" --argjson max_lines "$MAX_LINES" --argjson changed "$changed_json" \
    '{verdict: $verdict, reason: $reason, files: $files, lines: $lines, lockfile_lines: $lockfile_lines,
      max_files: $max_files, max_lines: $max_lines, base: $base}
     + (if ($changed | length) > 0 then {changed: $changed} else {} end)'
  exit "$code"
}

# rust_lines_match <path> <regex>: true when an added or removed line of
# <path> matches <regex>. Only lines after the first hunk header are content.
rust_lines_match() {
  qgit --literal-pathspecs diff --no-renames --no-ext-diff --no-textconv --no-color -U0 "$BASE" -- "$1" \
    | LC_ALL=C awk -v re="$2" '
        /^diff --git / { in_hunk = 0; next }
        /^@@/ { in_hunk = 1; next }
        in_hunk && /^[-+]/ { if (substr($0, 2) ~ re) found = 1 }
        END { exit found ? 0 : 1 }'
}

is_sensitive() {
  case "$1" in
    .github/*|*/.github/*|build.rs|*/build.rs|.cargo/config*|*/.cargo/config*) return 0 ;;
    rust-toolchain|rust-toolchain.toml|*/rust-toolchain|*/rust-toolchain.toml) return 0 ;;
    .pre-commit-config.yaml|.husky/*|amplifier-bundle/tools/*|amplifier-bundle/recipes/*) return 0 ;;
    .gitmodules|.gitattributes|*/.gitattributes) return 0 ;;
  esac
  return 1
}

# --- arguments -------------------------------------------------------------
[ $# -ge 1 ] || fault "usage: <worktree> [--base <sha>] [--committed] [--max-files N] [--max-lines N] | --snapshot <worktree>"
command -v jq >/dev/null 2>&1 || fault "jq is not on PATH"

if [ "$1" = "--snapshot" ]; then
  [ $# -eq 2 ] || fault "usage: --snapshot <worktree>"
  WT="$2"
  git -C "$WT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || fault "'$WT' is not a git work tree"
  snapshot
  exit 0
fi

WT="$1"; shift
BASE=""; MAX_FILES=3; MAX_LINES=150; COMMITTED=0
FILES=0; LINES=0; LOCK_LINES=0
while [ $# -gt 0 ]; do
  if [ "$1" = "--committed" ]; then COMMITTED=1; shift; continue; fi
  [ $# -ge 2 ] || fault "$1 needs a value"
  case "$1" in
    --base) BASE="$2" ;;
    --max-files) MAX_FILES="$2" ;;
    --max-lines) MAX_LINES="$2" ;;
    *) fault "unknown argument '$1'" ;;
  esac
  shift 2
done
for limit in "$MAX_FILES" "$MAX_LINES"; do
  [[ "$limit" =~ ^[0-9]{1,6}$ ]] || fault "limits must match ^[0-9]{1,6}\$"
done
MAX_FILES=$((10#$MAX_FILES)); MAX_LINES=$((10#$MAX_LINES))
git -C "$WT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || fault "'$WT' is not a git work tree"

if [ -z "$BASE" ]; then
  default_ref="$(git -C "$WT" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  [ -n "$default_ref" ] || default_ref="origin/main"
  BASE="$(qgit merge-base "$default_ref" HEAD)" || fault "no merge base with $default_ref"
fi
[[ "$BASE" =~ ^[0-9a-f]{40}$ ]] || fault "--base must be a 40-character hex SHA"
qgit merge-base --is-ancestor "$BASE" HEAD 2>/dev/null || fault "--base $BASE is not an ancestor of HEAD"

# --- rule 1: git configuration and hooks drift -------------------------------
# Compared before any diff runs, so a changed fsmonitor or diff driver never
# executes during measurement.
if [ -n "${QUICK_CHANGE_STATE_DIR:-}" ]; then
  SNAPSHOT_FILE="$QUICK_CHANGE_STATE_DIR/git-config.snapshot"
  [ -f "$SNAPSHOT_FILE" ] && [ ! -L "$SNAPSHOT_FILE" ] || fault "snapshot $SNAPSHOT_FILE is missing"
  CURRENT="$(snapshot)"
  if [ "$CURRENT" != "$(cat "$SNAPSHOT_FILE")" ]; then
    CHANGED="$(LC_ALL=C comm -3 <(printf '%s\n' "$CURRENT") <(LC_ALL=C sort "$SNAPSHOT_FILE") \
      | LC_ALL=C awk -F '\t' '{ sub(/^\t/, ""); print $1 }' | LC_ALL=C sort -u)"
    echo "quick-change: git configuration or hooks changed during the agent step; stopping without hand-over or push" >&2
    emit stop git-config-drift 4 "$CHANGED"
  fi
else
  echo "WARNING: QUICK_CHANGE_STATE_DIR is unset; git configuration drift is not checked" >&2
fi

# In --committed mode a clean worktree makes `git diff BASE` equal BASE..HEAD.
if [ "$COMMITTED" -eq 1 ]; then
  STATUS="$(qgit status --porcelain --untracked-files=normal)" || fault "git status failed"
  [ -z "$STATUS" ] || fault "worktree not clean in --committed mode"
fi

# --- measure -----------------------------------------------------------------
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Untracked, non-ignored files are marked intent-to-add so the diff counts
# them, in one git call however many there are.
qgit ls-files -z -o --exclude-standard > "$WORK/untracked" || fault "git ls-files failed"
if [ -s "$WORK/untracked" ]; then
  qgit --literal-pathspecs add -N --pathspec-from-file="$WORK/untracked" --pathspec-file-nul \
    || fault "git add -N failed"
fi
qgit diff --no-renames --no-ext-diff --no-textconv --numstat -z "$BASE" > "$WORK/numstat" || fault "git diff --numstat failed"
qgit diff --no-renames --no-ext-diff --raw -z "$BASE" > "$WORK/raw" || fault "git diff --raw failed"

# Paths of new files, and whether any mode change or symlink is present, from
# --raw: records alternate `:<old> <new> <sha> <sha> <status>` and a path.
NEW_PATHS=$'\n'; MODE_CHANGE=0; meta=""
while IFS= read -r -d '' field; do
  if [ -z "$meta" ]; then meta="$field"; continue; fi
  # shellcheck disable=SC2086
  set -- ${meta#:}
  case "${5:-}" in A*) NEW_PATHS="${NEW_PATHS}${field}"$'\n' ;; esac
  if [ "${1:-}" = 120000 ] || [ "${2:-}" = 120000 ]; then MODE_CHANGE=1
  elif [ "${1:-}" != 000000 ] && [ "${2:-}" != 000000 ] && [ "${1:-}" != "${2:-}" ]; then MODE_CHANGE=1
  fi
  meta=""
done < "$WORK/raw"

UNPARSEABLE=0; PATHS_FILE="$WORK/paths"; : > "$PATHS_FILE"
rec=""
while IFS= read -r -d '' rec; do
  added="${rec%%$'\t'*}"; rest="${rec#*$'\t'}"
  deleted="${rest%%$'\t'*}"; path="${rest#*$'\t'}"
  if [ "$added" = "$rec" ] || [ "$deleted" = "$rest" ] || [ -z "$path" ] \
    || ! [[ "$added" =~ ^([0-9]+|-)$ ]] || ! [[ "$deleted" =~ ^([0-9]+|-)$ ]]; then
    UNPARSEABLE=1; break
  fi
  [ "$added" != "-" ] || added=0
  [ "$deleted" != "-" ] || deleted=0
  FILES=$((FILES + 1))
  base_name="${path##*/}"
  case "$NEW_PATHS" in *$'\n'"$path"$'\n'*) is_new=1 ;; *) is_new=0 ;; esac
  if [ "$is_new" -eq 0 ] && [[ "$LOCKFILES" == *" $base_name "* ]]; then
    LOCK_LINES=$((LOCK_LINES + added + deleted))
  else
    LINES=$((LINES + added + deleted))
  fi
  printf '%s\0' "$path" >> "$PATHS_FILE"
  rec=""
done < "$WORK/numstat"
# A record left over after the loop had no NUL terminator.
[ -z "$rec" ] || UNPARSEABLE=1

# --- rules 2 to 11 -------------------------------------------------------------
[ "$UNPARSEABLE" -eq 0 ] || emit handover unparseable-path 0
if [ "$FILES" -eq 0 ]; then
  echo "quick-change: no changes produced" >&2
  emit empty empty-diff 3
fi
[ "$FILES" -le "$MAX_FILES" ] || emit handover "files>$MAX_FILES" 0
[ "$LINES" -le "$MAX_LINES" ] || emit handover "lines>$MAX_LINES" 0
if [ "$COMMITTED" -eq 0 ]; then
  COMMITS="$(qgit rev-list --count "$BASE..HEAD")" || fault "git rev-list failed"
  [ "$COMMITS" -eq 0 ] || emit handover agent-commit 0
fi

PUBLIC=0; CLI=0; MANIFEST=0; SENSITIVE=0
while IFS= read -r -d '' path; do
  case "$path" in
    *.rs)
      if rust_lines_match "$path" "$PUB_RE"; then PUBLIC=1; fi
      if rust_lines_match "$path" "$CLI_RE"; then CLI=1; fi ;;
  esac
  case "${path##*/}" in Cargo.toml|package.json|pyproject.toml) MANIFEST=1 ;; esac
  if is_sensitive "$path"; then SENSITIVE=1; fi
done < "$PATHS_FILE"

[ "$PUBLIC" -eq 0 ] || emit handover public-interface 0
[ "$CLI" -eq 0 ] || emit handover cli-definition 0
[ "$MANIFEST" -eq 0 ] || emit handover manifest 0
[ "$SENSITIVE" -eq 0 ] || emit handover sensitive-path 0
[ "$MODE_CHANGE" -eq 0 ] || emit handover mode-or-symlink 0
emit proceed within-limits 0
