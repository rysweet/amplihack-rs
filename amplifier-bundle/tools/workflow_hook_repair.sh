#!/usr/bin/env bash
# shellcheck disable=SC2034  # the HOOK_REPAIR_* results are read by the caller
# workflow_hook_repair.sh — hook_repair_run, the repair loop for a git hook
# that rejects a recipe-owned commit or push (issue #1671).
#
#   hook_repair_run <commit|push> <step-id> [--fix-message <msg>] [--output <file>] -- <cmd...>
#
# Runs <cmd> in the current shell. When it fails because a pre-commit,
# commit-msg or pre-push hook rejected it, the hook's redacted output goes to
# a fixing agent (hook-rejection-repair.yaml), the agent's changes are staged
# and the same operation is retried. After every failed retry the
# loop-health evaluator decides whether another round is worth running. There
# is no iteration cap: ROUND is a label, as in autodrive_loop.sh (#1337).
#
# Hooks are never bypassed or weakened. This file never passes a hook-skipping
# flag, never sets or clears SKIP, PRE_COMMIT_*, GIT_*, HUSKY or LEFTHOOK*, and
# never writes git configuration. A round in which the agent changes hook
# configuration, a script a local hook runs, lint configuration, a
# secret-scanner allowlist, git configuration, HEAD, the branch's refs or the
# stash, adds an inline suppression, or untracks a file ends the loop as
# `refused`; a round whose refusal snapshot cannot be taken ends it as `stuck`.
# A round in which the agent reports not_fixable_in_code, changes nothing and
# the hook output repeats ends as `stuck` without asking the evaluator.
#
# Nothing is created under TMPDIR before a hook rejection is identified: the
# first attempt's output is kept in the git directory, so a first-try success
# needs nothing the bare command did not.
#
# Return codes: 0 ok, the command's own rc when the failure was not a hook
# rejection, 1 stuck / evaluator_done_unresolved / refused, 79 terminal, 2
# usage. HOOK_REPAIR_STATUS, HOOK_REPAIR_HOOK_IDS, HOOK_REPAIR_REASON,
# HOOK_REPAIR_ROUNDS and HOOK_REPAIR_EVIDENCE_DIR are set on every return.
# Nothing is written to stdout, no trap is installed, and all loop state is
# local.
#
# Contract: docs/reference/recipe-commit-hook-repair.md

HOOK_REPAIR_EXIT_TERMINAL=79
HOOK_REPAIR_TAIL_BYTES=16384
# The bundle this file was loaded from; recipes are taken from it, never from
# the worktree under repair (#1543).
HOOK_REPAIR_TOOLS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)"

# Credential-like key names, case-insensitive without relying on sed's I flag.
HOOK_REPAIR_SECRET_KEY='([Tt][Oo][Kk][Ee][Nn]|[Ss][Ee][Cc][Rr][Ee][Tt]|[Pp][Aa][Ss][Ss][Ww][Oo][Rr][Dd]|[Pp][Aa][Ss][Ss][Ww][Dd]|[Aa][Pp][Ii][_-]?[Kk][Ee][Yy]|[Aa][Cc][Cc][Ee][Ss][Ss][_-]?[Kk][Ee][Yy]|[Pp][Rr][Ii][Vv][Aa][Tt][Ee][_-]?[Kk][Ee][Yy]|[Cc][Rr][Ee][Dd][Ee][Nn][Tt][Ii][Aa][Ll])'

# hook_repair_redact [file]: stdin or <file> with credentials replaced by REDACTED.
hook_repair_redact() {
  local k="$HOOK_REPAIR_SECRET_KEY"
  LC_ALL=C awk '
    /-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----/ { print; skip = 1; next }
    skip && /-----END [A-Z0-9 ]*PRIVATE KEY-----/ { print "REDACTED"; print; skip = 0; next }
    !skip { print }' "${1:--}" \
    | LC_ALL=C sed -E \
      -e 's#([A-Za-z][A-Za-z0-9+.-]*://)[^@/[:space:]]+@#\1REDACTED@#g' \
      -e 's#(gh[pousr]_|github_pat_)[A-Za-z0-9_]+#\1REDACTED#g' \
      -e 's#([Aa]uthorization:).*#\1 REDACTED#' \
      -e 's#([Bb]earer[[:space:]]+)[^[:space:]]+#\1REDACTED#g' \
      -e 's#(^|[^A-Za-z0-9])sk-[A-Za-z0-9_-]{8,}#\1REDACTED#g' \
      -e 's#xox[abprs]-[A-Za-z0-9-]+#REDACTED#g' \
      -e 's#(A3T[A-Z0-9]|AKIA|ASIA|AGPA|AIDA|AROA|ANPA|ANVA|AIPA)[0-9A-Z]{16}#REDACTED#g' \
      -e 's#glpat-[A-Za-z0-9_-]{20,}#glpat-REDACTED#g' \
      -e 's#AIza[0-9A-Za-z_-]{35}#REDACTED#g' \
      -e 's#npm_[A-Za-z0-9]{36}#npm_REDACTED#g' \
      -e 's#eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]+#REDACTED#g' \
      -e 's#(([Aa]ccount|[Ss]hared[Aa]ccess)[Kk]ey[[:space:]]*=[[:space:]]*)[^;&"[:space:]]+#\1REDACTED#g' \
      -e 's#([?&]sig=)[^&"[:space:]]+#\1REDACTED#g' \
      -e 's#^([[:space:]]*(Secret|Match):).*#\1 REDACTED#' \
      -e "s#(\"[^\"]*${k}[^\"]*\"[[:space:]]*:[[:space:]]*)\"[^\"]*\"#\\1\"REDACTED\"#g" \
      -e "s#('[^']*${k}[^']*'[[:space:]]*:[[:space:]]*)'[^']*'#\\1'REDACTED'#g" \
      -e "s#([A-Za-z0-9_.-]*${k}[A-Za-z0-9_.-]*[[:space:]]*[=:][[:space:]]*)\"[^\"]*\"#\\1\"REDACTED\"#g" \
      -e "s#([A-Za-z0-9_.-]*${k}[A-Za-z0-9_.-]*[[:space:]]*[=:][[:space:]]*)'[^']*'#\\1'REDACTED'#g" \
      -e "s#([A-Za-z0-9_.-]*${k}[A-Za-z0-9_.-]*[[:space:]]*[=:][[:space:]]*)[^\"'[:space:]][^[:space:]]*#\\1REDACTED#g"
}

# hook_repair_jsafe_lines: stdin with `"`, `\` and control characters other
# than newline removed (SR-8).
hook_repair_jsafe_lines() {
  LC_ALL=C tr -d '"\134\000-\011\013-\037\177'
}

# hook_repair_jsafe: as hook_repair_jsafe_lines, newlines removed too.
hook_repair_jsafe() {
  hook_repair_jsafe_lines | LC_ALL=C tr -d '\n'
}

# hook_repair_hooks_dir: the absolute hooks directory; honours core.hooksPath (read only).
hook_repair_hooks_dir() {
  local dir
  dir="$(git rev-parse --git-path hooks 2>/dev/null)" || return 1
  case "$dir" in /*) ;; *) dir="$(pwd -P)/${dir}" ;; esac
  printf '%s' "$dir"
}

# hook_repair_ids <file>: sorted, de-duplicated, whitelisted `- hook id:` values,
# at most 20, comma-separated; `unknown` when none survive (SR-3, SR-4).
hook_repair_ids() {
  local ids
  ids="$(LC_ALL=C sed -n 's/^[[:space:]]*- hook id: \(.*\)$/\1/p' "$1" 2>/dev/null \
    | LC_ALL=C tr -d '\r' \
    | LC_ALL=C grep -E '^[A-Za-z0-9._-]{1,64}$' \
    | LC_ALL=C sort -u | head -n 20 | paste -sd, -)"
  printf '%s' "${ids:-unknown}"
}

# hook_repair_is_rejection <op> <rc> <output-file> <hooks-dir>: 0 when the
# failure is attributable to a hook (docs: Classifying a failure).
hook_repair_is_rejection() {
  local op="$1" rc="$2" out="$3" hooks="$4"
  [ "$rc" -ne 0 ] || return 1
  # git's own fatal error, or a commit that could not be signed, is not a
  # hook's doing whatever hooks exist. A hook that prints such a line itself
  # is not repaired, which fails safe.
  LC_ALL=C grep -E '^fatal:|gpg failed to sign|^error: gpg failed' "$out" >/dev/null 2>&1 && return 1
  case "$op" in
    commit)
      git diff --cached --quiet 2>/dev/null && return 1
      [ -x "${hooks}/pre-commit" ] && [ -f "${hooks}/pre-commit" ] && return 0
      [ -x "${hooks}/commit-msg" ] && [ -f "${hooks}/commit-msg" ] && return 0
      return 1
      ;;
    push)
      [ -x "${hooks}/pre-push" ] && [ -f "${hooks}/pre-push" ] || return 1
      grep -qF 'failed to push some refs' "$out" || return 1
      # Only lines starting as git's own do (fatal:, error:, remote: ...) are
      # searched. A pre-push hook writes to the same stream with no prefix git
      # adds, so its output cannot be told apart: a hook that prints such a
      # line, such as `error: ... timed out` from a test run, makes the push
      # count as an ordinary failure. It is not repaired, which fails safe.
      # The second grep reads all its input: with -q it would stop at the
      # first match, the first grep would die of SIGPIPE and pipefail would
      # make the test false.
      if LC_ALL=C grep -E '^[[:space:]]*(fatal:|error:|ssh:|remote:|! |To )' "$out" \
        | grep -E '\[rejected\]|\[remote rejected\]|non-fast-forward|fetch first|Authentication failed|Permission denied|Could not resolve host|unable to access|Connection refused|Connection reset|timed out|Could not read from remote repository' >/dev/null; then
        return 1
      fi
      return 0
      ;;
  esac
  return 1
}

# hook_repair_terminal_refusal <rc> <log>: exit 79, or the guard's marker outside
# input echoes. Copied from autodrive_loop.sh terminal_refusal, except that
# the last grep reads all its input (no -q), so a marker followed by a large
# log is not lost to SIGPIPE under pipefail.
hook_repair_terminal_refusal() {
  [ "${1:-0}" = "$HOOK_REPAIR_EXIT_TERMINAL" ] && return 0
  [ -f "${2:-}" ] && awk 'NR > 1 { if (prev ~ /^Recipe: / && $0 ~ /^(Run ID|Status): /) { cut = 1; exit } print prev } { prev = $0 } END { if (NR > 0 && !cut) print prev }' "$2" 2>/dev/null \
    | grep -vE '^[[:space:]]*(\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] \[amplihack:|amplihack\.recipe\.log_pointer \{)' \
    | grep -F 'BLOCKED_TERMINAL orchestration_unavailable' >/dev/null && return 0
  return 1
}

# hook_repair_hash <path>: sha256 of a file.
hook_repair_hash() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -- "$1" | cut -d' ' -f1
  else
    shasum -a 256 -- "$1" | cut -d' ' -f1
  fi
}

# hook_repair_entry <key> <path>: one snapshot line, `key<TAB>state`. Fails
# when a file cannot be hashed, so an unreadable hook is never recorded as an
# empty hash that would hide an edit.
hook_repair_entry() {
  local h
  if [ -L "$2" ]; then
    # The link and what it resolves to: an edit to a linked hook script (for
    # example .git/hooks/pre-commit -> scripts/pre-commit.sh) is a change too.
    if [ -f "$2" ]; then
      h="$(hook_repair_hash "$2")" && [ -n "$h" ] || return 1
      printf '%s\tlink:%s:sha256:%s\n' "$1" "$(readlink -- "$2")" "$h"
    else
      printf '%s\tlink:%s\n' "$1" "$(readlink -- "$2")"
    fi
  elif [ -f "$2" ]; then
    h="$(hook_repair_hash "$2")" && [ -n "$h" ] || return 1
    printf '%s\tsha256:%s\n' "$1" "$h"
  elif [ -e "$2" ]; then
    printf '%s\tother\n' "$1"
  else
    printf '%s\tabsent\n' "$1"
  fi
}

# Lint configuration that decides what a hook reports, at any depth.
HOOK_REPAIR_LINT_CONFIG_RE='(^|/)(\.?clippy\.toml|\.?rustfmt\.toml|\.cargo/config|\.cargo/config\.toml)$'

# hook_repair_local_entries: in the worktree top, the first word of each
# `entry:` of a `repo: local` hook in .pre-commit-config.yaml, one per line,
# when it names a path inside the worktree. Block style YAML only; flow style
# (`- {repo: local, ...}`) is not read.
hook_repair_local_entries() {
  local top word
  [ -f .pre-commit-config.yaml ] || return 0
  top="$(pwd -P)"
  LC_ALL=C awk -v q="'" '
    /^[[:space:]]*-[[:space:]]*repo:/ {
      v = $0; sub(/^[[:space:]]*-[[:space:]]*repo:[[:space:]]*/, "", v); sub(/[[:space:]]#.*$/, "", v)
      gsub(/[[:space:]"]/, "", v); gsub(q, "", v); loc = (v == "local"); next }
    loc && /^[[:space:]]*(-[[:space:]]*)?entry:/ {
      v = $0; sub(/^[[:space:]]*(-[[:space:]]*)?entry:[[:space:]]*/, "", v)
      split(v, w, /[[:space:]]+/); gsub(/"/, "", w[1]); gsub(q, "", w[1])
      if (w[1] != "") print w[1] }' .pre-commit-config.yaml \
    | while IFS= read -r word; do
        case "$word" in "$top"/*) word="${word#"$top"/}" ;; /*|-*) continue ;; esac
        word="${word#./}"
        case "/${word}/" in */../*) continue ;; esac
        # A bare command name (cargo, bash) is a path only when such a file exists.
        case "$word" in */*) ;; *) [ -e "$word" ] || continue ;; esac
        printf '%s\n' "$word"
      done
}

# hook_repair_cargo_lints <Cargo.toml>: its [lints], [lints.*],
# [workspace.lints] and [workspace.lints.*] tables, and any dotted lints key.
hook_repair_cargo_lints() {
  LC_ALL=C awk '
    /^[[:space:]]*\[/ { h = $0; gsub(/[[:space:]]/, "", h); on = (h ~ /^\[(workspace\.)?lints(\.[^]]*)?\]/) }
    on || /^[[:space:]]*(workspace\.)?lints[[:space:]]*[.=]/ { print }' "$1"
}

# hook_repair_snapshot <worktree>: hook configuration, the scripts local hooks
# run, lint configuration, allowlists, the hooks directory, git configuration,
# HEAD, the branch's refs and the stash, one sorted, unique line per entry.
# Taken at the worktree top whatever the caller's directory. The hooks
# directory is resolved afresh each time, and the hook-related configuration
# is read at every scope, so a core.hooksPath or hook.* change in the global
# config shows too. Lines are unique because the refusal check compares with
# `uniq -u`, which would lose a changed entry recorded twice.
hook_repair_snapshot() {
  local entries
  # Collected first so that a failed find fails the snapshot: a hook file that
  # cannot be read would otherwise be missing from both snapshots and its
  # change would go unseen. The caller treats a failed snapshot as stuck.
  entries="$( cd "$1" || exit 1
    set -o pipefail
    local hooks path files lints head_ref
    hooks="$(hook_repair_hooks_dir 2>/dev/null)" || hooks=""
    printf 'hooks-dir\t%s\n' "${hooks:-unresolved}"
    for path in .pre-commit-config.yaml .pre-commit-hooks.yaml .secrets.baseline .gitleaksignore \
      .gitleaks.toml .trufflehogignore .talismanrc clippy.toml .clippy.toml rustfmt.toml .rustfmt.toml \
      .cargo/config.toml .cargo/config; do
      hook_repair_entry "$path" "$path" || exit 1
    done
    find . -mindepth 1 -maxdepth 1 \( -name .husky -o -name 'lefthook*.yml' -o -name 'lefthook*.yaml' \
      -o -name '.lefthook*' -o -name '.lintstagedrc*' -o -name 'lint-staged.config.*' \) -print0 \
      | xargs -0 -I{} find {} \( -type f -o -type l \) -print \
      | while IFS= read -r path; do hook_repair_entry "${path#./}" "$path" || exit 1; done || exit 1
    if [ -n "$hooks" ] && [ -d "$hooks" ]; then
      find -H "$hooks" -mindepth 1 \( -type f -o -type l \) -print \
        | while IFS= read -r path; do hook_repair_entry "hooks/${path#"$hooks"/}" "$path" || exit 1; done || exit 1
    fi
    hook_repair_local_entries | while IFS= read -r path; do hook_repair_entry "$path" "$path" || exit 1; done || exit 1
    # Tracked and untracked, non-ignored files: lint configuration in any
    # directory, and the lint tables of every Cargo.toml.
    files="$(git ls-files -z -co --exclude-standard | LC_ALL=C tr '\0' '\n')" || exit 1
    printf '%s\n' "$files" | { LC_ALL=C grep -E "$HOOK_REPAIR_LINT_CONFIG_RE" || true; } \
      | while IFS= read -r path; do hook_repair_entry "$path" "$path" || exit 1; done || exit 1
    printf '%s\n' "$files" | { LC_ALL=C grep -E '(^|/)Cargo\.toml$' || true; } \
      | while IFS= read -r path; do
          [ -f "$path" ] || { printf '%s[lints]\tabsent\n' "$path"; continue; }
          lints="$(hook_repair_cargo_lints "$path")" || exit 1
          printf '%s[lints]\tcksum:%s\n' "$path" "$(printf '%s' "$lints" | cksum)"
        done || exit 1
    printf 'git-config-local\t%s\n' "$(git config --local --list 2>&1 | cksum)"
    printf 'git-config-worktree\t%s\n' "$(git config --worktree --list 2>&1 | cksum)"
    # Filtered so unrelated global changes, such as a credential helper set by
    # another session, do not refuse. --show-origin puts a tab before each key.
    # The grep finding nothing is fine: the snapshot's status is sort's.
    printf 'git-config-hooks\t%s\n' "$(git config --show-origin --list 2>&1 \
      | LC_ALL=C grep -iE $'\t''(core\.hookspath|core\.fsmonitor|hook\.|include\.path|includeif\..*\.path|push\.)' | cksum)"
    printf 'HEAD\t%s\n' "$(git rev-parse -q --verify HEAD 2>/dev/null || echo none)"
    # The current branch, its remote-tracking refs, upstream and push refs: a
    # push, update-ref or reset made during the round moves one of them. Other
    # branches are left out because every worktree of the repository shares
    # them and parallel sessions move them.
    head_ref="$(git symbolic-ref -q HEAD 2>/dev/null)" || head_ref=""
    printf 'git-refs\t%s\n' "$( {
      [ -z "$head_ref" ] || git for-each-ref --format='%(refname) %(objectname)' "$head_ref" "refs/remotes/*/${head_ref#refs/heads/}"
      git rev-parse -q --verify '@{upstream}' 2>/dev/null || true
      git rev-parse -q --verify '@{push}' 2>/dev/null || true; } | cksum)"
    printf 'git-stash\t%s\n' "$( { git for-each-ref --format='%(objectname)' refs/stash; git stash list --format='%H' 2>/dev/null || true; } | cksum)"
  )" || return 1
  printf '%s\n' "$entries" | LC_ALL=C sort -u
}

# Inline markers that tell a secret scanner to ignore the line they are on.
HOOK_REPAIR_SCANNER_MARKERS='pragma:[[:space:]]*allowlist|gitleaks:allow|trufflehog:ignore|talisman:ignore|nosemgrep|ggignore'
# Inline markers that silence a linter or skip a test. Bracket expressions,
# not backslashes, because awk -v would process the escapes.
HOOK_REPAIR_LINT_MARKERS='#!?[[]allow[(]|#!?[[]expect[(]|#[[]ignore|#[[:space:]]*noqa|eslint-disable|@ts-ignore|@ts-nocheck|shellcheck[[:space:]]+disable'

# hook_repair_path_list: paths on stdin as one sorted, JSON-safe,
# comma-separated list.
hook_repair_path_list() {
  LC_ALL=C sort -u | hook_repair_jsafe_lines | paste -sd, - | sed 's/,/, /g'
}

# hook_repair_diff_check <worktree> <tree-before> <tree-after>: prints the
# refusal reason when the agent's round added an inline secret-scanner or
# linter suppression, or took a tracked file out of the index while leaving
# it in the worktree (`git rm --cached`). A file counts as gaining a
# suppression when its added lines carry more markers than its removed lines,
# so editing a line that already had one is not refused. Without both trees
# the whole staged diff against HEAD is searched, which fails closed.
hook_repair_diff_check() {
  local wt="$1" diff removed found files path kept=""
  # A diff that cannot be produced refuses the round rather than passing it.
  if [ -n "$2" ] && [ -n "$3" ]; then
    diff="$(git diff -U0 --no-color "$2" "$3")" || { printf 'scanner-suppression-check-failed'; return 0; }
    removed="$(git diff --no-renames --name-only --diff-filter=D -z "$2" "$3" | LC_ALL=C tr '\0' '\n')" \
      || { printf 'scanner-suppression-check-failed'; return 0; }
  else
    diff="$(git diff -U0 --no-color --cached)" || { printf 'scanner-suppression-check-failed'; return 0; }
    removed="$(git diff --cached --no-renames --name-only --diff-filter=D -z | LC_ALL=C tr '\0' '\n')" \
      || { printf 'scanner-suppression-check-failed'; return 0; }
  fi
  found="$(printf '%s\n' "$diff" | LC_ALL=C awk -v s="$HOOK_REPAIR_SCANNER_MARKERS" -v l="$HOOK_REPAIR_LINT_MARKERS" '
    /^diff --git / { next }
    /^--- / { next }
    /^\+\+\+ / { f = substr($0, 7); next }
    /^[-+]/ { d = (substr($0, 1, 1) == "+") ? 1 : -1
      if ($0 ~ s) sn[f] += d; else if ($0 ~ l) ln[f] += d }
    END { for (k in sn) if (sn[k] > 0) print "S\t" k; for (k in ln) if (ln[k] > 0) print "L\t" k }')"
  files="$(printf '%s\n' "$found" | LC_ALL=C awk -F '\t' '$1 == "S" { print $2 }')"
  if [ -n "$files" ]; then printf 'scanner-suppression-added: %s' "$(printf '%s\n' "$files" | hook_repair_path_list)"; return 0; fi
  files="$(printf '%s\n' "$found" | LC_ALL=C awk -F '\t' '$1 == "L" { print $2 }')"
  if [ -n "$files" ]; then printf 'lint-suppression-added: %s' "$(printf '%s\n' "$files" | hook_repair_path_list)"; return 0; fi
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    if [ -e "${wt}/${path}" ] || [ -L "${wt}/${path}" ]; then kept="${kept}${path}"$'\n'; fi
  done <<<"$removed"
  [ -n "$kept" ] || return 1
  printf 'tracked-file-untracked: %s' "$(printf '%s' "$kept" | hook_repair_path_list)"
}

# hook_repair_refusal_check <before> <after>: prints the refusal reason, if any.
hook_repair_refusal_check() {
  local changed paths refs
  changed="$(LC_ALL=C sort "$1" "$2" | uniq -u | cut -f1 | LC_ALL=C sort -u)"
  [ -n "$changed" ] || return 1
  paths="$(printf '%s\n' "$changed" | grep -vxE 'HEAD|git-refs|git-stash' | hook_repair_jsafe_lines | paste -sd, - | sed 's/,/, /g')"
  refs="$(printf '%s\n' "$changed" | grep -xE 'git-refs|git-stash' | paste -sd, - | sed 's/,/, /g')"
  if [ -n "$paths" ]; then
    printf 'hook-config-modified: %s' "$paths"
  elif printf '%s\n' "$changed" | grep -qx HEAD; then
    printf 'agent-moved-head'
  else
    printf 'agent-changed-refs: %s' "$refs"
  fi
  return 0
}

# hook_repair_attempt <out> <output-file> <cmd...>: runs <cmd> and keeps only
# its redacted combined output, in <out> and <output-file> if one is given.
# The raw output lives beside <out>, private, only until it is redacted.
# Returns the command's exit code.
hook_repair_attempt() {
  local out="$1" copy="$2" rc=0
  shift 2
  ( umask 077 && : >"${out}.raw" ) 2>/dev/null || true
  "$@" >"${out}.raw" 2>&1 || rc=$?
  hook_repair_redact "${out}.raw" >"$out"; rm -f -- "${out}.raw"
  [ -z "$copy" ] || cat -- "$out" >"$copy"
  return "$rc"
}

# hook_repair_redact_in_place <file>: replaces <file> with its redacted text,
# or removes it when that fails, so no raw nested-run log is kept.
hook_repair_redact_in_place() {
  [ -f "$1" ] || return 0
  if hook_repair_redact "$1" >"${1}.redacted"; then
    mv -f -- "${1}.redacted" "$1"
  else
    rm -f -- "$1" "${1}.redacted"
  fi
}

# hook_repair_evidence <file> <op> <step> <rc> <ids> <output-file>: one round's
# evidence, redacted as a whole.
hook_repair_evidence() {
  {
    printf 'operation: %s\nstep: %s\nexit_code: %s\nhook_ids: %s\n' "$2" "$3" "$4" "$5"
    printf '\n--- staged (git diff --cached --name-status) ---\n'
    git --no-pager diff --cached --name-status 2>&1
    printf '\n--- status (git status --porcelain) ---\n'
    git --no-pager status --porcelain 2>&1
    printf '\n--- output (combined stdout and stderr, redacted) ---\n'
    cat -- "$6"
  } | hook_repair_redact >"$1"
}

# hook_repair_agent_status <log>: the agent's reported status from a closed set.
hook_repair_agent_status() {
  local found
  found="$(LC_ALL=C grep -oE '"hook_repair"[[:space:]]*:[[:space:]]*\{[[:space:]]*"status"[[:space:]]*:[[:space:]]*"(fixed|not_fixable_in_code)"[[:space:]]*[,}]' "$1" 2>/dev/null \
    | tail -n 1)"
  case "$found" in
    *'"not_fixable_in_code"'*) printf 'not_fixable_in_code' ;;
    *'"fixed"'*) printf 'fixed' ;;
    *) printf 'unreported' ;;
  esac
}

# hook_repair_health_line <log>: the line straight after the LAST completed
# step-04 status line, as autodrive_loop.sh reads it (format.rs, #1512).
hook_repair_health_line() {
  LC_ALL=C awk '
    pend { rec = $0; pend = 0 }
    /^  [^ ]+ step-[^ :(]+( \([^)]*\))?: / {
      rec = ""; pend = 0
      if ($0 ~ /^  ✓ step-04-enforce-loop-verdict( \([^)]*\))?: completed( \[[^]]*\])?$/) pend = 1
    }
    END { print rec }' "$1" 2>/dev/null
}

# hook_repair_result_line <status> <op> <step> <ids> <verdict> <reasoning> <rounds> <dir>
hook_repair_result_line() {
  local ids_json="" id
  for id in ${4//,/ }; do
    ids_json="${ids_json:+${ids_json},}\"$(printf '%s' "$id" | hook_repair_jsafe)\""
  done
  printf '{"hook_repair_result":{"version":1,"status":"%s","operation":"%s","step":"%s","hook_ids":[%s],"loop_verdict":"%s","reasoning":"%s","rounds":%d,"evidence_dir":"%s"}}\n' \
    "$(printf '%s' "$1" | hook_repair_jsafe)" "$(printf '%s' "$2" | hook_repair_jsafe)" \
    "$(printf '%s' "$3" | hook_repair_jsafe)" "${ids_json:-\"unknown\"}" \
    "$(printf '%s' "$5" | hook_repair_jsafe)" "$(printf '%s' "$6" | hook_repair_redact | hook_repair_jsafe)" \
    "$7" "$(printf '%s' "$8" | hook_repair_jsafe)" >&2
}

# hook_repair_step15_reason <op>: step-15's commit_result reason after a failed
# hook_repair_run; returns 1 when the failure was not a hook rejection. Built
# from the status, the operation and the sanitised ids only.
hook_repair_step15_reason() {
  case "${HOOK_REPAIR_STATUS:-}" in
    terminal) printf 'orchestration-unavailable' ;;
    stuck|refused|evaluator_done_unresolved)
      printf 'hook-rejected-%s:%s:%s' "$HOOK_REPAIR_STATUS" "$1" "${HOOK_REPAIR_HOOK_IDS:-unknown}" ;;
    *) return 1 ;;
  esac
}

# hook_repair_push_commit <step> <message>: commits the staged fix of a push
# round as a new commit, never an amend, through a nested commit repair. It is
# a plain `git commit`, as step-15's own commit is. Returns the nested
# repair's code; HOOK_REPAIR_* then describe the nested repair.
hook_repair_push_commit() {
  if declare -F amplihack_prepare_git_commit_identity >/dev/null; then
    amplihack_prepare_git_commit_identity || true
  fi
  hook_repair_run commit "$1" -- git commit -m "$2"
}

# hook_repair_evaluate <hr> <round> <output> <bin> <recipe> <step> <op> <wt>
#   <ids> <prev-ids> <baseline> <rc> <agent-status>
# Runs the loop-health evaluator on the round just finished and prints
# `<verdict><TAB><reasoning>`, where the verdict is CONTINUE, DONE, STUCK or
# TERMINAL. A missing or malformed verdict, or a non-zero exit other than 79,
# is STUCK. Returns 79 for TERMINAL, otherwise 0.
hook_repair_evaluate() {
  local hr="$1" round="$2" out="$3" bin="$4" recipe="$5" step="$6" op="$7" wt="$8"
  local ids="$9" prev_ids="${10}" baseline="${11}" rc="${12}" agent_status="${13}"
  local log="${hr}/health-${round}.log" erc=0 verdict="STUCK" body="" line
  local verdict_re='^(CONTINUE|DONE|STUCK)( |$)'
  # The evaluator starts in HR_DIR, never in the worktree (#1528). The hook
  # output and the history go in through files in the private evidence
  # directory, not argv, where any local user could read them with ps.
  tail -c "$HOOK_REPAIR_TAIL_BYTES" "$out" >"${hr}/eval-${round}.output"
  tail -n 20 "${hr}/history" >"${hr}/eval-${round}.history"
  if ( cd "$hr" && exec "$bin" recipe run "$recipe" --working-dir "$hr" \
    -c "loop_name=hook-repair:${step}:${op}" \
    -c "loop_round_label=${round}" \
    -c "loop_repo_path=${wt}" \
    -c "loop_agent_working_dir=${hr}" \
    -c "loop_health_enforce=false" \
    --context-from-file "loop_last_round_output=${hr}/eval-${round}.output" \
    -c "loop_findings_current=${ids}" \
    -c "loop_findings_previous=${prev_ids}" \
    -c "loop_baseline_ref=${baseline}" \
    -c "loop_child_exit_code=${rc}" \
    -c "loop_round_verdict=${agent_status}" \
    -c "loop_clean_token=HOOK_PASSED" \
    --context-from-file "loop_history=${hr}/eval-${round}.history" ) >"$log" 2>&1; then erc=0; else erc=$?; fi
  if hook_repair_terminal_refusal "$erc" "$log"; then
    hook_repair_redact_in_place "$log"
    echo "ERROR: hook_repair_run: loop-health-evaluator returned the exit-${HOOK_REPAIR_EXIT_TERMINAL} terminal policy refusal; not retried (#1327)." >&2
    printf 'TERMINAL\t'
    return "$HOOK_REPAIR_EXIT_TERMINAL"
  fi
  if [ "$erc" -eq 0 ]; then
    line="$(hook_repair_health_line "$log")"
    case "$line" in
      "    Output: LOOP_HEALTH: "*) body="${line#    Output: LOOP_HEALTH: }" ;;
      "LOOP_HEALTH: "*) body="${line#LOOP_HEALTH: }" ;;
    esac
    if [[ "$body" =~ $verdict_re ]]; then verdict="${BASH_REMATCH[1]}"; else body=""; fi
  fi
  hook_repair_redact_in_place "$log"
  printf '%s\t%s' "$verdict" "$(printf '%s' "${body#"$verdict"}" | sed -E 's/^[[:space:]]*(-|—)?[[:space:]]*//')"
}

hook_repair_usage() {
  echo "ERROR: hook_repair_run: $1" >&2
  echo "  usage: hook_repair_run <commit|push> <step-id> [--fix-message <msg>] [--output <file>] -- <cmd...>" >&2
  HOOK_REPAIR_STATUS="usage"
  return 2
}

hook_repair_run() {
  local op="${1:-}" step="${2:-}" fix_message="" output_file="" have_fix=0 have_cmd=0
  HOOK_REPAIR_STATUS="usage"; HOOK_REPAIR_HOOK_IDS="unknown"; HOOK_REPAIR_REASON=""
  HOOK_REPAIR_ROUNDS=0; HOOK_REPAIR_EVIDENCE_DIR=""
  case "$op" in
    commit|push) ;;
    *) hook_repair_usage "the operation must be commit or push, not '${op}'"; return 2 ;;
  esac
  [[ "$step" =~ ^[a-z0-9-]{1,64}$ ]] || { hook_repair_usage "the step id must match ^[a-z0-9-]{1,64}\$"; return 2; }
  shift 2
  while [ $# -gt 0 ]; do
    case "$1" in
      --fix-message) [ $# -ge 2 ] || { hook_repair_usage "--fix-message needs a value"; return 2; }
        fix_message="$2"; have_fix=1; shift 2 ;;
      --output) [ $# -ge 2 ] || { hook_repair_usage "--output needs a value"; return 2; }
        output_file="$2"; shift 2 ;;
      --) shift; have_cmd=1; break ;;
      *) hook_repair_usage "unexpected argument '$1' before --"; return 2 ;;
    esac
  done
  [ "$have_cmd" -eq 1 ] && [ $# -gt 0 ] || { hook_repair_usage "no command after --"; return 2; }
  if [ "$op" = "push" ]; then
    [ "$have_fix" -eq 1 ] && [ -n "$fix_message" ] || { hook_repair_usage "push requires --fix-message"; return 2; }
    case "$fix_message" in
      *[[:cntrl:]]*) hook_repair_usage "--fix-message must be one line with no control characters"; return 2 ;;
    esac
  elif [ "$have_fix" -eq 1 ]; then
    hook_repair_usage "--fix-message applies to push only"; return 2
  fi

  local wt gd hr hooks bin round_recipe health_recipe
  wt="$(git rev-parse --show-toplevel 2>/dev/null)" || { hook_repair_usage "not inside a git work tree"; return 2; }
  wt="$(cd "$wt" && pwd -P)"
  bin="${AMPLIHACK_BIN:-amplihack}"
  round_recipe="${HOOK_REPAIR_ROUND_RECIPE:-${HOOK_REPAIR_TOOLS_DIR%/*}/recipes/hook-rejection-repair.yaml}"
  health_recipe="${HOOK_REPAIR_HEALTH_RECIPE:-${HOOK_REPAIR_TOOLS_DIR%/*}/recipes/loop-health-evaluator.yaml}"

  # --- first attempt ----------------------------------------------------------
  # Its output is kept in the git directory, which a commit or push needs to
  # be able to write anyway; TMPDIR is not touched until a hook has rejected
  # the operation.
  local rc=0 out
  gd="$(git rev-parse --absolute-git-dir 2>/dev/null)" || gd=""
  out="$([ -n "$gd" ] && umask 077 && mktemp "${gd}/hook-repair-attempt.XXXXXX" 2>/dev/null)" || out=""
  if [ -z "$out" ]; then
    # With nowhere to keep the output the failure cannot be classified: the
    # command runs as it is and a failure is returned unrepaired.
    if "$@" >&2; then HOOK_REPAIR_STATUS="ok"; return 0; else rc=$?; fi
    HOOK_REPAIR_STATUS="not_hook"; return "$rc"
  fi
  if hook_repair_attempt "$out" "$output_file" "$@"; then rc=0; else rc=$?; fi
  if [ "$rc" -eq 0 ]; then
    rm -f -- "$out"; HOOK_REPAIR_STATUS="ok"; return 0
  fi
  # Resolved only once something has failed; the success path skips it. An
  # unresolvable directory leaves hooks empty, so hook_repair_is_rejection
  # finds no hook and returns not_hook: the loop never runs without one.
  hooks="$(hook_repair_hooks_dir)" || hooks=""
  if ! hook_repair_is_rejection "$op" "$rc" "$out" "$hooks"; then
    rm -f -- "$out"; HOOK_REPAIR_STATUS="not_hook"; return "$rc"
  fi

  # A hook rejected the operation: from here on everything goes in a private
  # evidence directory outside the worktree.
  hr="$(umask 077 && mktemp -d "${TMPDIR:-/tmp}/hook-repair.XXXXXX")" \
    || { rm -f -- "$out"; hook_repair_usage "could not create a private evidence directory under ${TMPDIR:-/tmp}"; return 2; }
  hr="$(cd "$hr" && pwd -P)"
  case "$hr/" in
    "$wt"/*)
      rm -rf -- "$hr"; rm -f -- "$out"
      hook_repair_usage "the evidence directory ${hr} lies inside the worktree ${wt}; set TMPDIR outside it (SR-14)"
      return 2 ;;
  esac
  mv -f -- "$out" "${hr}/attempt.out" \
    || { rm -rf -- "$hr"; rm -f -- "$out"; hook_repair_usage "could not move the first attempt's output into ${hr}"; return 2; }
  out="${hr}/attempt.out"

  local ids prev_ids="" verdict="" reasoning="" status="" round=0 prev_hash agent_status evaluation
  ids="$(hook_repair_ids "$out")"
  HOOK_REPAIR_HOOK_IDS="$ids"
  prev_hash="$(hook_repair_hash "$out" | cut -c1-12)"
  if ! "$bin" recipe show "$health_recipe" >/dev/null 2>&1; then
    echo "ERROR: hook_repair_run: the loop-health evaluator ${health_recipe} does not resolve; it is the loop's only terminator, so no repair round runs (#1337)." >&2
    status="stuck"; reasoning="loop-health-evaluator does not resolve"
  fi

  # --- the rounds: ROUND is a label, never compared against a limit -----------
  local baseline before after tree_before tree_after evidence agent_log terminal
  local refusal diffstat out_hash changed inner_rc inner_rounds=0
  while [ -z "$status" ]; do
    round=$((round + 1))
    HOOK_REPAIR_ROUNDS=$((round + inner_rounds))
    baseline="$(git rev-parse -q --verify HEAD 2>/dev/null || true)"
    before="${hr}/round-${round}.before"; after="${hr}/round-${round}.after"
    # A valid snapshot always holds the HEAD and git-config-* lines, so an
    # empty one failed. Without it the refusal check would pass unseen.
    if ! hook_repair_snapshot "$wt" >"$before" || [ ! -s "$before" ]; then
      echo "ERROR: hook_repair_run: the refusal snapshot ${before} could not be taken; not retried." >&2
      status="stuck"; reasoning="refusal snapshot could not be taken"; break
    fi
    tree_before="$(git write-tree 2>/dev/null || true)"
    evidence="${hr}/round-${round}.evidence"
    hook_repair_evidence "$evidence" "$op" "$step" "$rc" "$ids" "$out" \
      || echo "WARNING: hook_repair_run: the evidence file ${evidence} may be incomplete." >&2
    agent_log="${hr}/agent-${round}.log"
    echo "hook_repair_run: ${op} in ${step} rejected by hook(s) ${ids}; repair round ${round}" >&2
    if "$bin" recipe run "$round_recipe" --working-dir "$wt" \
      -c "hook_repair_worktree=${wt}" \
      -c "hook_repair_evidence_file=${evidence}" \
      -c "hook_repair_operation=${op}" \
      -c "hook_repair_step=${step}" \
      -c "hook_repair_hook_ids=${ids}" >"$agent_log" 2>&1; then inner_rc=0; else inner_rc=$?; fi
    terminal=0
    hook_repair_terminal_refusal "$inner_rc" "$agent_log" && terminal=1
    agent_status="$(hook_repair_agent_status "$agent_log")"
    { printf '\n--- agent report ---\n'; LC_ALL=C grep -E '"hook_repair"' "$agent_log" | tail -n 1 | hook_repair_redact; } >>"$evidence" \
      || echo "WARNING: hook_repair_run: the agent report was not appended to ${evidence}." >&2
    # Everything needed from the raw log has been read; only its redacted text is kept.
    hook_repair_redact_in_place "$agent_log"
    if [ "$terminal" -eq 1 ]; then
      echo "ERROR: hook_repair_run: the fixing agent returned the exit-${HOOK_REPAIR_EXIT_TERMINAL} terminal policy refusal; not retried (#1327)." >&2
      status="terminal"; break
    fi
    if ! stage_workflow_changes; then
      status="stuck"; reasoning="staging the agent's changes failed"; break
    fi
    # The step ran the guard before the first attempt; the agent's round adds
    # files after it, so what is now staged is checked again before a retry.
    if ! "$bin" hygiene artifact-guard --repo "$wt" --mode pre-publish >&2; then
      echo "ERROR: hook_repair_run: Artifact Guard blocked the files staged after repair round ${round}; not retried." >&2
      status="stuck"; reasoning="artifact guard blocked the agent's changes"; break
    fi
    if ! hook_repair_snapshot "$wt" >"$after" || [ ! -s "$after" ]; then
      echo "ERROR: hook_repair_run: the refusal snapshot ${after} could not be taken; not retried." >&2
      status="stuck"; reasoning="refusal snapshot could not be taken"; break
    fi
    if refusal="$(hook_repair_refusal_check "$before" "$after")"; then
      echo "ERROR: hook_repair_run: ${refusal}; the agent may not change hook or lint configuration, HEAD, refs or the stash. Not retried." >&2
      status="refused"; reasoning="$refusal"; verdict=""; break
    fi
    tree_after="$(git write-tree 2>/dev/null || true)"
    if refusal="$(hook_repair_diff_check "$wt" "$tree_before" "$tree_after")"; then
      echo "ERROR: hook_repair_run: ${refusal}; the agent may not suppress a scanner or linter, or untrack a file. Not retried." >&2
      status="refused"; reasoning="$refusal"; verdict=""; break
    fi
    diffstat="no diff"
    if [ -z "$tree_before" ] || [ -z "$tree_after" ]; then
      diffstat="unknown"
    elif [ "$tree_before" != "$tree_after" ]; then
      diffstat="$(git diff --shortstat "$tree_before" "$tree_after" 2>/dev/null | sed 's/^ *//')"
    fi

    # Push: the agent's fix becomes a new commit, never an amend (D7).
    if [ "$op" = "push" ] && ! git diff --cached --quiet 2>/dev/null; then
      if hook_repair_push_commit "$step" "${fix_message//\{hook_ids\}/"$ids"}"; then inner_rc=0; else inner_rc=$?; fi
      inner_rounds=$((inner_rounds + HOOK_REPAIR_ROUNDS))
      HOOK_REPAIR_ROUNDS=$((round + inner_rounds)); HOOK_REPAIR_HOOK_IDS="$ids"
      case "$HOOK_REPAIR_STATUS" in
        ok) ;;
        terminal) status="terminal"; break ;;
        not_hook)
          echo "ERROR: hook_repair_run: committing the agent's fix failed (exit ${inner_rc}) and was not a hook rejection." >&2
          status="stuck"; reasoning="the fix commit failed (exit ${inner_rc})"; break ;;
        *)
          status="$HOOK_REPAIR_STATUS"; verdict=""
          reasoning="the fix commit was not repaired: ${HOOK_REPAIR_REASON}"; break ;;
      esac
    fi

    # Retry the same operation.
    if hook_repair_attempt "$out" "$output_file" "$@"; then rc=0; else rc=$?; fi
    if [ "$rc" -eq 0 ]; then
      status="ok"; break
    fi
    if ! hook_repair_is_rejection "$op" "$rc" "$out" "$hooks"; then
      status="not_hook"; break
    fi
    prev_ids="$ids"
    ids="$(hook_repair_ids "$out")"
    HOOK_REPAIR_HOOK_IDS="$ids"
    out_hash="$(hook_repair_hash "$out" | cut -c1-12)"
    changed="yes"; [ "$out_hash" != "$prev_hash" ] || changed="no"
    prev_hash="$out_hash"
    # The full history stays in the evidence; the evaluator gets the last 20
    # lines, so its input stays bounded however many rounds run (D3).
    printf 'round %s: output_sha256=%s output_changed=%s agent=%s diff=%s\n' \
      "$round" "$out_hash" "$changed" "$agent_status" "${diffstat:-no diff}" >>"${hr}/history"

    # A round that changed nothing at all is a fact, not a judgement: the agent
    # says the cause is not in the code, the staged tree is the same and the
    # hook said the same thing again. This is not an iteration cap (#1337).
    if [ "$agent_status" = "not_fixable_in_code" ] && [ "$changed" = "no" ] \
      && [ -n "$tree_before" ] && [ "$tree_before" = "$tree_after" ]; then
      echo "hook_repair_run: round ${round} made no progress: the agent reported not_fixable_in_code, changed nothing, and the hook output repeated; not retried." >&2
      status="stuck"; verdict=""
      reasoning="no progress: the agent reported not_fixable_in_code, the staged tree did not change and the hook output was identical to the previous attempt"
      break
    fi

    evaluation="$(hook_repair_evaluate "$hr" "$round" "$out" "$bin" "$health_recipe" "$step" "$op" "$wt" \
      "$ids" "$prev_ids" "$baseline" "$rc" "$agent_status")" || true
    verdict="${evaluation%%$'\t'*}"; reasoning="${evaluation#*$'\t'}"
    case "$verdict" in
      CONTINUE) reasoning="" ;;
      TERMINAL) status="terminal"; verdict="" ;;
      DONE) status="evaluator_done_unresolved"
        reasoning="evaluator ended the loop while the hook still rejects${reasoning:+: ${reasoning}}" ;;
      *) status="stuck"; verdict="STUCK"
        reasoning="${reasoning:-loop-health-evaluator returned STUCK or no readable verdict}" ;;
    esac
  done

  HOOK_REPAIR_REASON="$(printf '%s' "$reasoning" | hook_repair_redact)"
  HOOK_REPAIR_STATUS="$status"
  case "$status" in
    ok) rm -rf -- "$hr"; return 0 ;;
    not_hook) rm -rf -- "$hr"; HOOK_REPAIR_HOOK_IDS="unknown"; return "$rc" ;;
    terminal) HOOK_REPAIR_EVIDENCE_DIR="$hr"; return "$HOOK_REPAIR_EXIT_TERMINAL" ;;
  esac
  HOOK_REPAIR_EVIDENCE_DIR="$hr"
  echo "ERROR: hook_repair_run: ${op} in ${step} still rejected (${status}); last attempt's output follows. Evidence: ${hr}" >&2
  cat -- "$out" >&2
  hook_repair_result_line "$status" "$op" "$step" "$HOOK_REPAIR_HOOK_IDS" "$verdict" "$HOOK_REPAIR_REASON" \
    "$HOOK_REPAIR_ROUNDS" "$hr"
  return 1
}
