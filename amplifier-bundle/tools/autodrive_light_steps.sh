#!/usr/bin/env bash
# autodrive_light_steps.sh - the auto-drive-light branch of
# autodrive-crusty-round.yaml step-03 (issues #1802 and #1982).
#
# Sourced, never executed, from the round's validated tool root,
# AUTODRIVE_LAUNCH_TOOLS, the same root as autodrive_light_round.sh; this
# file only defines functions. The round recipe is at its 400-line brick
# budget, so the branch lives here and step-03 calls it in one line. Step-03
# does not source this file without the light condition, so a round with no
# light-mode marker runs exactly as before #1802.
#
#   autodrive_light_step03   step-03, after the verdict, CONCERNS and the
#                            .findings file are written, and only when the
#                            state dir holds a light-mode marker that is a
#                            regular file. For a crusty CONCERNS verdict it
#                            runs `autodrive_light_round.sh check` from the
#                            launch-side tools, which measures a moved head
#                            again and escalates when the change is no
#                            longer light. The tool's stdout is discarded,
#                            and VERDICT, SOURCE and COUNT are never changed:
#                            the #1979 classify branch that follows decides
#                            whether the round is minor-only. Returns 79 when
#                            the tool exits 79 (final), else 0.
#
# Contract: docs/reference/auto-drive-light.md

# autodrive_light_step03: reads VERDICT SOURCE RECORD; changes no variable
# the step reads. Values come from RECIPE_VAR_* only.
autodrive_light_step03() {
  local rc
  [ "${VERDICT:-}" = "CONCERNS" ] && [ "${SOURCE:-}" = "crusty" ] && [ -n "${RECORD:-}" ] || return 0
  bash "/${AUTODRIVE_LAUNCH_TOOLS#/}/autodrive_light_round.sh" check --record "$RECORD" \
    --pr "$(_autodrive_light_ctx pr)" --reviewed-head "$(_autodrive_light_ctx head_sha)" \
    --repo "${RECIPE_VAR_repo_path:-.}" </dev/null >/dev/null
  rc=$?
  if [ "$rc" -eq 79 ]; then
    echo "ERROR: autodrive_light_round.sh exited 79; that code is final and is not retried" >&2
    return 79
  fi
  if [ "$rc" -ne 0 ]; then
    echo "WARNING: autodrive_light_round.sh check exited ${rc}; the round is classified without re-measurement." >&2
  fi
  return 0
}

# _autodrive_light_ctx <field>: a field of step-01's round context.
_autodrive_light_ctx() {
  printf '%s' "${RECIPE_VAR_crusty_round_context:-}" \
    | amplihack orch helper extract-json --require-field "$1" \
    | amplihack orch helper extract-field --field "$1" --default ''
}
