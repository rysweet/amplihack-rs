#!/usr/bin/env bash
# recipe_context_value.sh - write one recipe context value to a file,
# whichever channel holds it (issue #1570).
#
# Usage (sourced or run directly):
#   write_context_value <KEY> <OUT>
#
# Lookup order:
#   1. The base variable RECIPE_VAR_<base>, used when set (even if empty).
#      <base> is the part of <KEY> before the first `.`, with `-` replaced by
#      `_`. An undotted key writes the value as it is. A dotted key `a.b.c`
#      parses RECIPE_VAR_a as JSON and walks the path `b`, `c` one object key
#      at a time (#1561); a base that is not exactly one JSON object, or
#      lacks a key on the path, exits 3. A nested name such as RECIPE_VAR_<a>__<b> is never
#      built or read: the runner writes those only for fields the current
#      object has, so one can hold a parent run's value.
#   2. Only when the base variable is unset: AMPLIHACK_CONTEXT_FILE, read with
#      jq at the path given by splitting <KEY> on `.`. A set base key is this
#      run's value and never falls back to the file. The file is trusted as
#      the runner's own spill of this run's context (#1570).
#   3. Neither: exit 3. An absent key is never written as an empty file.
#
# Strings are written raw; other values as compact JSON.
#
# <OUT> of `-` writes to stdout.
#
# Exit codes: 0 written, 1 read or write failure, 2 usage error,
# 3 key not available. No eval; the value never reaches argv.

# The body runs in a subshell so umask and traps never leak into a caller
# that sourced this file.
write_context_value() (
  set -u
  if [ "$#" -ne 2 ] || [ -z "$2" ]; then
    printf '%s\n' "usage: write_context_value <KEY> <OUT>" >&2
    exit 2
  fi
  key=$1
  out=$2
  if ! [[ "$key" =~ ^[A-Za-z0-9_][A-Za-z0-9_.-]*$ ]]; then
    printf '%s\n' "write_context_value: invalid context key (expected ^[A-Za-z0-9_][A-Za-z0-9_.-]*\$)" >&2
    exit 2
  fi
  base=${key%%.*}
  base=${base//-/_}
  var="RECIPE_VAR_${base}"
  path=
  if [ "$base" != "${key//-/_}" ]; then
    path=${key#*.}
  fi

  umask 077
  tmp=
  trap 'rm -f -- "$tmp"' EXIT
  if [ "$out" != "-" ]; then
    tmp=$(mktemp) || exit 1
  fi

  # Walk a dotted path one object key at a time so a missing key (exit 3)
  # is distinguished from a present null. The path reaches jq only as data.
  filter='reduce ($k | split("."))[] as $s (.;
            if type == "object" and has($s) then .[$s] else ("" | halt_error(3)) end)
          | if type == "string" then . else tojson end'

  if [ -n "${!var+set}" ] && [ -z "$path" ]; then
    if [ "$out" = "-" ]; then
      printf '%s' "${!var}" || exit 1
      exit 0
    fi
    printf '%s' "${!var}" > "$tmp" || exit 1
  elif [ -n "${!var+set}" ]; then
    command -v jq >/dev/null 2>&1 || {
      printf '%s\n' "write_context_value: jq is required to read dotted key '$key'" >&2
      exit 1
    }
    if ! printf '%s' "${!var}" | jq -e -s 'length == 1 and (.[0] | type == "object")' >/dev/null 2>&1; then
      printf '%s\n' "write_context_value: context key '$key' is not available ($var is set but is not a single JSON object)" >&2
      exit 3
    fi
    rc=0
    if [ "$out" = "-" ]; then
      # Buffer first so exit 3 prints nothing.
      val=$(printf '%s' "${!var}" | jq -j --arg k "$path" "$filter" && printf x) || rc=$?
    else
      printf '%s' "${!var}" | jq -j --arg k "$path" "$filter" > "$tmp" || rc=$?
    fi
    if [ "$rc" -eq 3 ]; then
      printf '%s\n' "write_context_value: context key '$key' is not available ($var has no field '$path')" >&2
      exit 3
    elif [ "$rc" -ne 0 ]; then
      printf '%s\n' "write_context_value: failed to read '$key' from $var" >&2
      exit 1
    fi
    if [ "$out" = "-" ]; then
      printf '%s' "${val%x}" || exit 1
      exit 0
    fi
  elif [ -n "${AMPLIHACK_CONTEXT_FILE:-}" ]; then
    ctx=$AMPLIHACK_CONTEXT_FILE
    if [ ! -r "$ctx" ] || [ ! -f "$ctx" ]; then
      printf '%s\n' "write_context_value: cannot read AMPLIHACK_CONTEXT_FILE $ctx" >&2
      exit 1
    fi
    command -v jq >/dev/null 2>&1 || {
      printf '%s\n' "write_context_value: jq is required to read AMPLIHACK_CONTEXT_FILE" >&2
      exit 1
    }
    rc=0
    if [ "$out" = "-" ]; then
      jq -j --arg k "$key" "$filter" "$ctx" || rc=$?
    else
      jq -j --arg k "$key" "$filter" "$ctx" > "$tmp" || rc=$?
    fi
    if [ "$rc" -eq 3 ]; then
      printf '%s\n' "write_context_value: context key '$key' is not available (not present in AMPLIHACK_CONTEXT_FILE and $var is unset)" >&2
      exit 3
    elif [ "$rc" -ne 0 ]; then
      printf '%s\n' "write_context_value: failed to read '$key' from AMPLIHACK_CONTEXT_FILE" >&2
      exit 1
    fi
    [ "$out" = "-" ] && exit 0
  else
    printf '%s\n' "write_context_value: context key '$key' is not available ($var is unset and AMPLIHACK_CONTEXT_FILE is not set)" >&2
    exit 3
  fi

  mv -f -- "$tmp" "$out" || {
    printf '%s\n' "write_context_value: cannot write $out" >&2
    exit 1
  }
  tmp=
  exit 0
)

# Run directly: behave like the function.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  write_context_value "$@"
  exit $?
fi
