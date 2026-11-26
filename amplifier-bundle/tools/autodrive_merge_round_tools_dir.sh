#!/usr/bin/env bash
# autodrive_merge_round_tools_dir.sh — the body of `step-00-tools-dir` in amplifier-bundle/recipes/autodrive-merge-round.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: the step runs it with
# `bash`, so it reads the same environment the recipe runner exported and
# writes the same stdout the step's `output:` captures. It is a file rather
# than an inline block so the recipe stays under its line budget; compressing
# the recipe instead is the one thing the brick rule forbids.

set -uo pipefail
cd "${REPO_PATH:-.}" 2>/dev/null || { echo "ERROR: cannot cd to REPO_PATH" >&2; exit 1; }
SEARCHED=""
NEED="autodrive_round_word.sh autodrive_platform_facts.sh autodrive_round_evidence.sh"
NEED="${NEED} autodrive_state.sh autodrive_trust.sh git-identity.sh git-repo-env.sh"
for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do
  [ -n "$c" ] || continue
  D="$c/amplifier-bundle/tools"; SEARCHED="${SEARCHED}${SEARCHED:+, }${D}"; OK="true"
  for t in $NEED; do [ -f "$D/$t" ] || OK="false"; done
  [ "$OK" = "true" ] && D="$(cd -- "$D" 2>/dev/null && pwd -P)" && [ -n "$D" ] || continue
  # The path reaches step-04's prompt in a double-quoted shell word: no quote,
  # backslash, $, backtick, template brace or control byte (x keeps a newline).
  case "$D" in *[\"\\\$\`]* | *'{{'* | *'}}'*) echo "WARNING: skipping an auto-drive tools path with a quote, backslash, \$, backtick or template brace" >&2; continue ;; esac
  [ "$(printf '%s' "$D" | LC_ALL=C tr -d '\040-\176\200-\377'; printf x)" = "x" ] \
    || { echo "WARNING: skipping an auto-drive tools path that holds a control byte" >&2; continue; }
  echo "INFO: auto-drive tools from ${D}" >&2
  printf '%s\n' "$D"
  exit 0
done
echo "ERROR: autodrive-tools-not-found: no amplifier-bundle/tools holds all of ${NEED} (searched ${SEARCHED:-no root})" >&2
exit 1
