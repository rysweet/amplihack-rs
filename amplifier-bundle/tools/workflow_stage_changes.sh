#!/usr/bin/env bash
# workflow_stage_changes.sh — stage_workflow_changes, the replacement for
# `git add -A` in the checkpoint and publish steps (issue #1671).
#
# Staging requires versioned requirements and reviewed-design authority with
# matching task identities. Only exact authorised candidate filenames are staged;
# existing user-staged entries remain protected. Runtime paths never gain
# authority, even when already tracked. Diagnostics go to stderr.
#
# The collection filter below is retained for older callers; its output is an
# inventory and cannot extend task authority. Checkpoints stage and commit in
# one private-index transaction rather than consuming a prior stage operation.
#
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
  amplihack hygiene workflow stage --repo . >&2
}
