#!/usr/bin/env bash
# workflow_stage_changes.sh — stage_workflow_changes, the replacement for
# `git add -A` in the checkpoint and publish steps (issue #1671).
#
# Stages tracked modifications and deletions, then every untracked,
# non-ignored file except build and packaging artefacts: a path with a
# component node_modules or __pycache__, or a component ending in .egg-info,
# at any depth; a `target` directory at the top or one holding Cargo's
# CACHEDIR.TAG, at any depth (so source such as src/target/mod.rs is still
# staged); and amplihack's own runtime state at the top: .claude/runtime/ and
# .amplihack/. An agent run in the worktree (a hook-repair round) writes both.
# Lockfiles (uv.lock, Cargo.lock, package-lock.json ...) are source and are
# staged. A tracked file under the two runtime directories is still staged
# when modified. Excluded paths are reported on stderr.
#
# Untracked paths are passed as literal pathspecs, so a file named `*` or
# `:(glob)x` is a name, not a pattern (SR-5). Writes nothing to stdout,
# installs no traps, and running it twice stages the same set.
#
# Contract: docs/reference/recipe-commit-hook-repair.md#staging-rules

# workflow_stage_filter <top>: NUL-separated untracked paths (relative to
# <top>) on stdin, the ones that are not build artefacts or runtime state on
# stdout, NUL-separated. Skipped paths are reported on stderr. Also used by
# the TDD checkpoint, whose first add is scoped to the paths it captured (#1672).
workflow_stage_filter() {
  local top="${1-}" path rest prefix cargo_dir=""
  [ -n "$top" ] || { echo 'stage_workflow_changes: empty filter root' >&2; return 2; }
  # The checkpoint recipe supplies ".". Resolve it before checking tags,
  # while refusing empty or failed canonicalisation results.
  top="$(cd -- "$top" && pwd -P)" || return 2
  [[ "$top" == /* && "$top" != / && -d "$top" ]] \
    || { echo 'stage_workflow_changes: invalid canonical filter root' >&2; return 2; }
  # The artefact test is inline in this loop, not a per-path function call:
  # one call per untracked path was most of the cost on a large un-ignored node_modules or target tree.
  # The two runtime directories are matched at the top only.
  while IFS= read -r -d '' path; do
    case "/${path}/" in
      */node_modules/*|*/__pycache__/*|*.egg-info/*|/.claude/runtime/*|/.amplihack/*|/target/*)
        printf 'stage_workflow_changes: skipped %s\n' "$path" >&2; continue ;;
      */target/*)
        # The last Cargo directory found is remembered, so a large build tree
        # costs one test per file, not one per directory level.
        if [ -n "$cargo_dir" ] && [ "${path#"$cargo_dir"/}" != "$path" ]; then
          printf 'stage_workflow_changes: skipped %s\n' "$path" >&2; continue
        fi
        rest="$path"; prefix=""
        while :; do
          case "$rest" in */*) ;; *) break ;; esac
          prefix="${prefix:+${prefix}/}${rest%%/*}"; rest="${rest#*/}"
          case "/${prefix}" in
            */target) [ -f "${top}/${prefix}/CACHEDIR.TAG" ] && { cargo_dir="$prefix"; break; } ;;
          esac
        done
        if [ -n "$cargo_dir" ] && [ "${path#"$cargo_dir"/}" != "$path" ]; then
          printf 'stage_workflow_changes: skipped %s\n' "$path" >&2; continue
        fi ;;
    esac
    printf '%s\0' "$path"
  done
}

stage_workflow_changes() {
  local top git_dir scratch physical list add_help rc=0
  top="$(git rev-parse --show-toplevel)" || return $?
  git_dir="$(git rev-parse --absolute-git-dir)" || return $?
  case "$top" in /*) ;; *) echo 'stage_workflow_changes: empty or relative worktree path' >&2; return 2 ;; esac
  case "$git_dir" in /*) ;; *) echo 'stage_workflow_changes: empty or relative Git metadata path' >&2; return 2 ;; esac
  top="$(cd -- "$top" && pwd -P)" || return 2
  git_dir="$(cd -- "$git_dir" && pwd -P)" || return 2
  [[ "$top" == /* && "$top" != / && -d "$top" && "$git_dir" == /* && "$git_dir" != / && -d "$git_dir" ]] \
    || { echo 'stage_workflow_changes: invalid canonical repository paths' >&2; return 2; }
  # Keep both lists in one private allocation, never in the tree being staged.
  # Do not consume or clean up a failed/invalid allocator result.
  scratch="$(umask 077 && mktemp -d "${git_dir:?}/stage-workflow-changes.XXXXXX")" || return $?
  case "$scratch" in "${git_dir:?}/stage-workflow-changes."*) ;; *) echo 'stage_workflow_changes: invalid scratch allocation' >&2; return 2 ;; esac
  [[ -d "$scratch" && ! -L "$scratch" && -O "$scratch" ]] || { echo 'stage_workflow_changes: scratch is not an owned directory' >&2; return 2; }
  physical="$(cd -- "$scratch" && pwd -P)" || return 2
  [[ -n "$physical" && "$physical" == "$scratch" && "${scratch%/*}" == "$git_dir" ]] \
    || { echo 'stage_workflow_changes: scratch escaped Git metadata' >&2; return 2; }
  list="${scratch:?}/paths"
  # No pathspec: `add -u` covers the whole tree, and unlike `-- :/` it does
  # not fail in a repository with nothing tracked yet.
  git -C "$top" add -u || { rc=$?; rm -rf -- "${scratch:?}"; return "$rc"; }
  # --full-name with `:/` lists paths from the top whatever the cwd.
  git -C "$top" ls-files --others --exclude-standard --full-name -z -- :/ >"${list}.all" || {
    rc=$?
    rm -rf -- "${scratch:?}"
    return "$rc"
  }
  workflow_stage_filter "$top" <"${list}.all" >"$list" || {
    rc=$?; rm -rf -- "${scratch:?}"; return "$rc"
  }
  if [ -s "$list" ]; then
    # `git add -h` exits 129, so its text is tested with case, not a pipe:
    # under pipefail the pipe would always be false.
    add_help="$(git add -h 2>&1)" || true
    case "$add_help" in
      *--pathspec-from-file*)
        git -C "$top" --literal-pathspecs add --pathspec-from-file="$list" --pathspec-file-nul >&2 || rc=$? ;;
      # git older than 2.25 has no --pathspec-from-file.
      *) xargs -0 git -C "$top" --literal-pathspecs add -- <"$list" >&2 || rc=$? ;;
    esac
  fi
  rm -rf -- "${scratch:?}"
  return "$rc"
}
