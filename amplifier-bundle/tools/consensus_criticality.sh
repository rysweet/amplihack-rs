#!/usr/bin/env bash
# consensus_criticality.sh - the consensus is_critical_code contract (#1966).
# Sourced after recipe_context_value.sh by the three consensus checkpoints.
# Contract: docs/reference/consensus-criticality-contract.md.
#
#   cc_validate_input  0 when RECIPE_VAR_is_critical_code is exactly 'true' or
#                      'false'; 1 when it is missing (unset or empty) or invalid.
#   cc_require_route   0 when exactly one complete route ran, with CC_ROUTE set
#                      to 'standard' or 'nversion'; 1 when the input is missing
#                      or invalid, no route ran, the selected route is
#                      incomplete, or 'false' left N-version outputs; 2 when a
#                      route output cannot be read.
#
# Checks run in this order: input, read failure, no route, incomplete, both.
# The value is matched exactly: no trimming and no case folding. Echoed values
# are cleaned to A-Za-z0-9._ - (anything else becomes '?') and cut to 64 bytes.
# Messages go to stderr only; values are never executed; caller shell options
# are untouched.

CC_NV_OUTPUTS="critical_sections nversion_v1 nversion_v2 nversion_v3 nversion_comparison implementation n_version_implementations"

cc_validate_input() {
  local cc_v=${RECIPE_VAR_is_critical_code-}
  case "$cc_v" in
    true|false) return 0 ;;
    '') echo "ERROR: missing required input is_critical_code (pass -c is_critical_code=true or -c is_critical_code=false)" >&2
        return 1 ;;
  esac
  local cc_s
  cc_s=$(printf '%s' "$cc_v" | LC_ALL=C tr -c 'A-Za-z0-9._ -' '?' | LC_ALL=C head -c 64)
  printf "ERROR: invalid is_critical_code '%s'; expected 'true' or 'false'\n" "$cc_s" >&2
  return 1
}

cc_require_route() {
  CC_ROUTE=""
  cc_validate_input || return 1
  local cc_v=$RECIPE_VAR_is_critical_code cc_n cc_val cc_rc cc_have="" cc_miss=""
  # Probe every route output in this shell so a read failure (rc 2) is kept.
  for cc_n in $CC_NV_OUTPUTS; do
    cc_val=$(write_context_value "$cc_n" - 2>/dev/null)
    cc_rc=$?
    if [ "$cc_rc" -eq 0 ]; then
      [ -n "$cc_val" ] && cc_have="$cc_have $cc_n "
    elif [ "$cc_rc" -ne 3 ]; then
      printf "ERROR: could not read context value '%s' (helper exit %s)\n" "$cc_n" "$cc_rc" >&2
      return 2
    fi
  done
  local cc_route=standard cc_need=implementation
  if [ "$cc_v" = true ]; then cc_route=nversion; cc_need=$CC_NV_OUTPUTS; fi
  for cc_n in $cc_need; do
    case "$cc_have" in *" $cc_n "*) ;; *) cc_miss="$cc_miss $cc_n" ;; esac
  done
  if [ "$cc_miss" = " $cc_need" ]; then
    printf "ERROR: no implementation route ran (is_critical_code='%s')\n" "$cc_v" >&2
    return 1
  fi
  if [ -n "$cc_miss" ]; then
    cc_miss=${cc_miss# }
    printf "ERROR: implementation route incomplete: %s missing %s\n" "$cc_route" "${cc_miss%% *}" >&2
    return 1
  fi
  if [ "$cc_route" = standard ]; then
    for cc_n in $CC_NV_OUTPUTS; do
      [ "$cc_n" = implementation ] && continue
      case "$cc_have" in *" $cc_n "*)
        echo "ERROR: both implementation routes produced outputs" >&2
        return 1 ;;
      esac
    done
  fi
  # shellcheck disable=SC2034 # read by the sourcing step
  CC_ROUTE=$cc_route
  return 0
}
