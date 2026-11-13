#!/usr/bin/env bash
# autodrive_merge_ready_files.sh — find the merge-ready skill's SKILL.md and
# pr-description-template.md for the auto-drive merge round (issue #1517).
#
# The merge-ready skill sets `disable-model-invocation: true`, so an agent
# cannot invoke it; the merge round reads its files instead. This script is
# the deterministic lookup that step-00 of autodrive-merge-round.yaml runs, in
# the runner-pinned AMPLIHACK_HOME only. Repository and PR-worktree copies
# never supply instructions. The skill and its template come from one directory.
# The auto-drive-to-merge skill folder is taken only from the sibling of that
# directory, ../auto-drive-to-merge, so the prompts can give its absolute path
# (issue #2222). A missing sibling is a hard stop, like a missing template: the
# criteria and the skill the prompts point at are never taken from two places.
#
#   stdout  one line of JSON: skill_dir, skill_md, template, skill_md_sha,
#           auto_drive_skill_dir
#   exit 1  ERROR: merge-ready-template-not-found / merge-ready-skill-files-not-found /
#           auto-drive-skill-dir-not-found / auto-drive-skill-dir-unusable
#           on stderr, and nothing on stdout
#
# Read-only: it creates, changes and deletes nothing.
#
# Policy: nothing in the auto-drive-to-merge workflow passes a hook-skipping
# commit flag or a branch-protection bypass. See
# docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions.

set -u

# unsafe_path <path>: true when it could break JSON or look like a template. The trailing x stops $(...) from
# stripping a newline, the one control byte command substitution would otherwise hide. A copy of this line is in
# step-01-round-context of autodrive-crusty-round.yaml (#2222); keep the two identical.
unsafe_path() { case "$1" in *'"'* | *'\'* | *'{{'* | *'}}'*) return 0 ;; esac; [ "$(printf '%s' "$1" | LC_ALL=C tr -d '\040-\176\200-\377'; printf x)" != "x" ]; }

case "${AMPLIHACK_HOME:-}" in /*) ;; *) echo "ERROR: trusted AMPLIHACK_HOME is required for merge-ready criteria" >&2; exit 1 ;; esac
CANDIDATES=("${AMPLIHACK_HOME}/amplifier-bundle/skills/merge-ready")

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
  # Issue #2222: the auto-drive-to-merge folder is the sibling of the chosen
  # merge-ready folder and nothing else; no other root is searched for it.
  if [ ! -f "${DIR}/../auto-drive-to-merge/SKILL.md" ]; then
    echo "ERROR: auto-drive-skill-dir-not-found: ${DIR}/../auto-drive-to-merge/SKILL.md (the auto-drive-to-merge skill must sit beside ${DIR}; no other directory is used)" >&2
    exit 1
  fi
  ADS="$(cd -- "${DIR}/../auto-drive-to-merge" 2>/dev/null && pwd -P)" || ADS=""
  if [ -z "$ADS" ] || unsafe_path "$ADS"; then
    echo "ERROR: auto-drive-skill-dir-unusable: $(printf '%s' "${ADS:-${DIR}/../auto-drive-to-merge}" | LC_ALL=C tr -d '\000-\037\177') (a quote, backslash, control byte or template braces, or it cannot be entered)" >&2
    exit 1
  fi
  SHA="$(git hash-object --no-filters --stdin < "${DIR}/SKILL.md" 2>/dev/null || printf '')"
  echo "INFO: merge-ready criteria from ${DIR} (${SHA:-unhashed})" >&2
  # The only candidate is the runner-pinned AMPLIHACK_HOME, never the branch
  # under review, so there is no base comparison to make here. (The block that
  # made one read $TOP, which #2340 no longer sets; under set -u that ended the
  # script before it printed anything.)
  printf '{"skill_dir":"%s","skill_md":"%s","template":"%s","skill_md_sha":"%s","auto_drive_skill_dir":"%s"}\n' \
    "$DIR" "${DIR}/SKILL.md" "${DIR}/pr-description-template.md" "$SHA" "$ADS"
  exit 0
done

echo "ERROR: merge-ready-skill-files-not-found: searched $(printf '%s' "${SEARCHED:-nothing (AMPLIHACK_HOME, REPO_PATH and HOME are all empty, and no git toplevel was found)}" | LC_ALL=C tr -d '\000-\037\177')" >&2
exit 1
