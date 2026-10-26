#!/usr/bin/env bash
# recipe_var.sh - read this run's own recipe values (issues #1561, #1607).
#
# Sourced, never executed. Defines two functions and changes nothing else.
#
# The recipe runner exports every declared context key as RECIPE_VAR_<key>
# and replaces it for each run. It also exports bare upper-case names and,
# for JSON objects, nested RECIPE_VAR_<key>__<field> names, but only for what
# the current value has, so either can still hold a parent run's value.
# These functions read the base RECIPE_VAR_<key> only.
#
#   rv_field <key> <field>
#       Print <field> of the JSON object in RECIPE_VAR_<key>. Prints nothing
#       for an unset or empty key, a value that is not exactly one JSON
#       object, or a missing or null field; `false` and `0` print as values.
#       A set value that is not one object also prints a WARNING to stderr.
#       A <key> holding "__" (a nested name) prints nothing. Callers treat
#       empty as "not reported". Needs jq: without it the
#       function prints an ERROR and returns 2.
#
#   no_merge_flag <label>
#       Print `true` when merging must be suppressed, else `false`. The
#       recipe's own RECIPE_VAR_no_merge decides. A bare NO_MERGE that is
#       truthy also suppresses merging, and prints an INFO line naming
#       <label>; a bare NO_MERGE can never turn merging back on. Truthy is
#       true, 1, yes or on, in any case, with whitespace ignored.

rv_field() {
  local key="$1" field="$2" var out
  [[ "$key" =~ ^[A-Za-z0-9_]+$ && "$key" != *__* && "$field" =~ ^[A-Za-z0-9_]+$ ]] || return 0
  var="RECIPE_VAR_${key}"
  [ -n "${!var:-}" ] || return 0
  command -v jq >/dev/null 2>&1 \
    || { echo "ERROR: rv_field: jq is required to read RECIPE_VAR_${key}" >&2; return 2; }
  out="$(printf '%s' "${!var}" | jq -r -s --arg f "$field" '
      if length == 1 and (.[0] | type) == "object" then .[0] else error("not one object") end
      | if has($f) and .[$f] != null then .[$f] | tostring else empty end' 2>/dev/null)" \
    || { echo "WARNING: rv_field: RECIPE_VAR_${key} is not a single JSON object; treated as missing (#1561)" >&2; return 0; }
  printf '%s' "$out"
}

# Normalise one flag value to `true` or `false`.
_rv_truthy() {
  local flag
  flag="$(printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
  case "$flag" in true|1|yes|on) printf 'true\n' ;; *) printf 'false\n' ;; esac
}

no_merge_flag() {
  local label="${1:-unknown step}"
  if [ "$(_rv_truthy "${RECIPE_VAR_no_merge:-}")" = "true" ]; then
    printf 'true\n'
  elif [ "$(_rv_truthy "${NO_MERGE:-}")" = "true" ]; then
    echo "INFO: ${label}: no_merge=true from bare NO_MERGE env (override, #1561)" >&2
    printf 'true\n'
  else
    printf 'false\n'
  fi
}
