#!/usr/bin/env bash
# shellcheck disable=SC2034  # VERDICT, SOURCE, COUNT and SUFFIX are read by the calling step.
# autodrive_light_steps.sh - the auto-drive-light branch of
# autodrive-crusty-round.yaml step-03 and step-06 (issue #1802).
#
# Sourced, never executed, from AMPLIHACK_HOME; this file only defines
# functions. The round recipe is at its 400-line brick budget, so the branch
# lives here and each step calls it in one line. Neither step sources this
# file without the light condition, so a round with no light-mode marker
# runs exactly as before #1802.
#
#   autodrive_light_step03   step-03, after the verdict, CONCERNS and the
#                            .findings file are written, and only when the
#                            state dir holds a light-mode marker that is a
#                            regular file. Sends the concerns to
#                            autodrive_light_round.sh from the launch-side
#                            tools. On minor-deferred it sets VERDICT=CLEAN,
#                            SOURCE=light-minor-rule and COUNT from the tool.
#                            Any other answer leaves them unchanged. Returns
#                            79 when the tool exits 79 (final), else 0.
#   autodrive_light_step06   step-06, when the verdict names light-minor-rule;
#                            takes step-03's verdict JSON as its argument.
#                            Validates <record>.light.json, <record>.concerns.tsv
#                            and <record>.findings and sets SUFFIX to the
#                            record's light fields, built by jq. Returns 1
#                            when a check fails; the step then records
#                            CONCERNS with no suffix.
#
# Contract: docs/reference/auto-drive-light.md

# autodrive_light_step03: reads VERDICT SOURCE CONCERNS RECORD, may set
# VERDICT SOURCE COUNT. Values come from RECIPE_VAR_* only.
autodrive_light_step03() {
  local out rc count
  [ "${VERDICT:-}" = "CONCERNS" ] && [ "${SOURCE:-}" = "crusty" ] && [ -n "${RECORD:-}" ] || return 0
  if ! command -v jq >/dev/null 2>&1; then
    echo "WARNING: light-mode marker present but jq is unavailable; the minor-only rule is not applied and CONCERNS stands." >&2
    return 0
  fi
  out="$(printf '%s' "${CONCERNS:-[]}" | bash "/${AUTODRIVE_LAUNCH_TOOLS#/}/autodrive_light_round.sh" defer --record "$RECORD" \
    --pr "$(_autodrive_light_ctx pr)" --reviewed-head "$(_autodrive_light_ctx head_sha)" --repo "${RECIPE_VAR_repo_path:-.}")"
  rc=$?
  if [ "$rc" -eq 79 ]; then
    echo "ERROR: autodrive_light_round.sh exited 79; that code is final and is not retried" >&2
    return 79
  fi
  count=""
  [ "$rc" -ne 0 ] || count="$(printf '%s' "$out" \
    | jq -er 'select(type == "object" and .light_exit == "minor-deferred") | .concern_count' 2>/dev/null)"
  if [[ "$count" =~ ^[0-9]{1,6}$ ]]; then
    VERDICT="CLEAN"; SOURCE="light-minor-rule"; COUNT="$count"
  else
    echo "INFO: light-mode marker present; the minor-only rule did not apply (exit ${rc}); CONCERNS stands." >&2
  fi
  return 0
}

# _autodrive_light_ctx <field>: a field of step-01's round context.
_autodrive_light_ctx() {
  printf '%s' "${RECIPE_VAR_crusty_round_context:-}" \
    | amplihack orch helper extract-json --require-field "$1" \
    | amplihack orch helper extract-field --field "$1" --default ''
}

# _autodrive_light_private <file>: a regular file owned by this user, not a
# symlink, with no group or world write bit.
_autodrive_light_private() {
  [ -f "$1" ] && [ ! -L "$1" ] && [ -O "$1" ] \
    && [ -z "$(find "$1" -maxdepth 0 \( -perm -0020 -o -perm -0002 \) -print 2>/dev/null)" ]
}

# autodrive_light_step06 <verdict json>: reads VERDICT RECORD, sets SUFFIX. Returns 0 with an empty SUFFIX when the verdict source is not
# light-minor-rule, 0 with the suffix when every check passes, else 1.
autodrive_light_step06() {
  local src lj
  SUFFIX=""
  command -v jq >/dev/null 2>&1 || return 1
  src="$(printf '%s' "${1:-}" | jq -r '.verdict_source // ""' 2>/dev/null)"
  [ "$src" = "light-minor-rule" ] || return 0
  [ "${VERDICT:-}" = "CLEAN" ] || return 1
  _autodrive_light_private "${RECORD}.light.json" && _autodrive_light_private "${RECORD}.concerns.tsv" || return 1
  [ -f "${RECORD}.findings" ] && [ ! -L "${RECORD}.findings" ] && [ ! -s "${RECORD}.findings" ] || return 1
  awk -F '\t' '$2 != "minor" { bad = 1 } END { exit bad }' "${RECORD}.concerns.tsv" || return 1
  lj="$(jq -ec --rawfile tsv "${RECORD}.concerns.tsv" '
    ($tsv | split("\n") | map(select(length > 0) | split("\t")[0])) as $ok
    | select(type == "object" and (keys == ["deferred_ids", "follow_up_issue"])
        and (.follow_up_issue | type == "string" and test("\\A[0-9]{1,10}\\z"))
        and (.deferred_ids | type == "array" and length > 0
          and all(.[]; type == "string" and test("\\A[A-Za-z0-9._-]{1,64}\\z")
                       and ((. as $i | $ok | index([$i])) != null))))
    | {verdict_source: "light-minor-rule", follow_up_issue, deferred_ids}' "${RECORD}.light.json" 2>/dev/null)" || return 1
  [ -n "$lj" ] || return 1
  SUFFIX=",${lj#\{}"
  SUFFIX="${SUFFIX%\}}"
  return 0
}
