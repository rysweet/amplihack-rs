#!/usr/bin/env bash
# quick_change_handover_step.sh — the body of `quick-change-write-handover` in amplifier-bundle/recipes/quick-change.yaml.
#
# Extracted for the brick budget when that recipe gained one `model:` line per
# agent step (issue #1619). The body is unchanged: the step runs it with
# `bash`, so it reads the same environment the recipe runner exported and
# writes the same stdout the step's `output:` captures. It is a file rather
# than an inline block so the recipe stays under its line budget; compressing
# the recipe instead is the one thing the brick rule forbids.

QC=""; for c in "${AMPLIHACK_HOME:-}" "${RECIPE_VAR_repo_path:-}" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/quick_change_common.sh" ] && { QC="$c/amplifier-bundle/tools/quick_change_common.sh"; break; }; done; QC_STEP=quick-change-write-handover; . "${QC:?quick_change_common.sh not found}"
git -C "$QC_WT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || qc_die "requires a git repo at $QC_WT; either \`git init\` or rerun from a checkout"
BASE="$(qc_base)"
BRANCH="$(git -C "$QC_WT" symbolic-ref -q --short HEAD)" || qc_die "the worktree is not on a branch"
REASON="$(qc_handover_reason)"
PATCH="$QUICK_CHANGE_STATE_DIR/quick-change-handover.patch"
# Untracked files were marked intent-to-add by the diff-limits tool.
git -C "$QC_WT" -c core.fsmonitor=false diff --binary --no-ext-diff --no-textconv "$BASE" | qc_write quick-change-handover.patch
NOTE="$(qc_handover_note "$REASON" "$BRANCH" "$BASE" "$PATCH")"
printf '%s\n' "$NOTE" | qc_write handover-note
echo "quick-change: handing over to default-workflow (reason=$REASON); patch at $PATCH" >&2
printf '%s' "$NOTE"
