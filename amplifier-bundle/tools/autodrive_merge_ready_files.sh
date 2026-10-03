#!/usr/bin/env bash
# autodrive_merge_ready_files.sh — find the merge-ready skill's SKILL.md and
# pr-description-template.md for the auto-drive merge round (issue #1517).
#
# The merge-ready skill sets `disable-model-invocation: true`, so an agent
# cannot invoke it; the merge round reads its files instead. This script is
# the deterministic lookup that step-00 of autodrive-merge-round.yaml runs, in
# the order the other auto-drive steps use to find bundle files:
#
#   1. $AMPLIHACK_HOME/amplifier-bundle/skills/merge-ready
#   2. $REPO_PATH/amplifier-bundle/skills/merge-ready
#   3. <git toplevel of $REPO_PATH>/amplifier-bundle/skills/merge-ready
#   4. $HOME/.copilot/skills/merge-ready              (flat layout)
#   5. $HOME/.amplihack/amplifier-bundle/skills/merge-ready
#
# A candidate whose variable is empty is skipped. The first directory whose
# SKILL.md is a regular file wins, and the template must be in that same
# directory: the two files are never taken from two installs.
#
#   stdout  one line of JSON: skill_dir, skill_md, template, skill_md_sha
#   exit 1  ERROR: merge-ready-template-not-found / merge-ready-skill-files-not-found
#           on stderr, and nothing on stdout
#
# Read-only: it creates, changes and deletes nothing.
#
# Policy: nothing in the auto-drive-to-merge workflow passes a hook-skipping
# commit flag or a branch-protection bypass. See
# docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions.

set -u

unsafe_path() { # unsafe_path <path>: true when it could break JSON or look like a template
  case "$1" in
    *'"'* | *'\'* | *'{{'* | *'}}'*) return 0 ;;
  esac
  [ -n "$(printf '%s' "$1" | LC_ALL=C tr -d '\040-\176\200-\377')" ]
}

TOP="$(git -C "${REPO_PATH:-.}" rev-parse --show-toplevel 2>/dev/null || printf '')"
CANDIDATES=()
[ -n "${AMPLIHACK_HOME:-}" ] && CANDIDATES+=("${AMPLIHACK_HOME}/amplifier-bundle/skills/merge-ready")
[ -n "${REPO_PATH:-}" ] && CANDIDATES+=("${REPO_PATH}/amplifier-bundle/skills/merge-ready")
[ -n "$TOP" ] && CANDIDATES+=("${TOP}/amplifier-bundle/skills/merge-ready")
[ -n "${HOME:-}" ] && CANDIDATES+=("${HOME}/.copilot/skills/merge-ready" "${HOME}/.amplihack/amplifier-bundle/skills/merge-ready")

SEARCHED=""
# ${arr[@]+...}: an empty array under `set -u` is an error in bash 3.2 (macOS).
for d in ${CANDIDATES[@]+"${CANDIDATES[@]}"}; do
  SEARCHED="${SEARCHED}${SEARCHED:+ }${d}"
  [ -f "${d}/SKILL.md" ] || continue
  DIR="$(cd -- "$d" 2>/dev/null && pwd -P)" || DIR=""
  if [ -z "$DIR" ] || unsafe_path "$d" || unsafe_path "$DIR"; then
    echo "WARNING: skipping merge-ready candidate with an unusable path (a quote, backslash, control byte or template braces): $(printf '%s' "$d" | LC_ALL=C tr -d '\000-\037\177')" >&2
    continue
  fi
  if [ ! -f "${DIR}/pr-description-template.md" ]; then
    echo "ERROR: merge-ready-template-not-found: ${DIR}/pr-description-template.md (SKILL.md is in ${DIR}; the template is never taken from another directory)" >&2
    exit 1
  fi
  SHA="$(git hash-object --no-filters --stdin < "${DIR}/SKILL.md" 2>/dev/null || printf '')"
  echo "INFO: merge-ready criteria from ${DIR} (${SHA:-unhashed})" >&2
  # In a repository that ships amplifier-bundle/, the criteria may come from
  # the branch under review. Say so when they differ from the base; this
  # never fails the step, because the merge gate re-measures criteria 1 and 3.
  if [ -n "$TOP" ]; then
    case "$DIR" in
      "$TOP"/*)
        BASE="$(git -C "$TOP" symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null || printf 'origin/main')"
        if git -C "$TOP" rev-parse --verify --quiet "${BASE}^{commit}" >/dev/null 2>&1 \
           && ! git -C "$TOP" --no-optional-locks diff --quiet "$BASE" -- "${DIR}/SKILL.md" 2>/dev/null; then
          echo "WARNING: merge-ready criteria come from the branch under review and differ from ${BASE}" >&2
        fi
        ;;
    esac
  fi
  printf '{"skill_dir":"%s","skill_md":"%s","template":"%s","skill_md_sha":"%s"}\n' \
    "$DIR" "${DIR}/SKILL.md" "${DIR}/pr-description-template.md" "$SHA"
  exit 0
done

echo "ERROR: merge-ready-skill-files-not-found: searched ${SEARCHED:-nothing (AMPLIHACK_HOME, REPO_PATH and HOME are all empty, and no git toplevel was found)}" >&2
exit 1
