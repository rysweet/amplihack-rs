#!/usr/bin/env bash
# Spec gate for issue #1326.
#
# Model-checks docs/spec/OrchLedger.tla in all four ablation configurations and
# asserts the EXPECTED outcome of each. Both directions matter:
#   - B_proposed must pass          (the design we ship is sound at these bounds)
#   - A/C/D must fail, specifically (the ablations still describe real hazards)
# A spec that no longer distinguishes the designs is not a gate, so an unexpected
# PASS is as much a failure here as an unexpected violation.
set -euo pipefail

SPEC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../docs/spec" && pwd)"
JAR="${TLA2TOOLS_JAR:-$HOME/tla2tools.jar}"

if ! command -v java >/dev/null 2>&1; then
  echo "check-spec: java not found — cannot run the spec gate" >&2
  exit 2
fi
if [ ! -f "$JAR" ]; then
  echo "check-spec: tla2tools.jar not found at $JAR" >&2
  echo "            set TLA2TOOLS_JAR, or fetch it from" >&2
  echo "            https://github.com/tlaplus/tlaplus/releases" >&2
  exit 2
fi

# config:expected   ("PASS", or the invariant that must be violated)
CASES=(
  "E_no_reaping:CapacityRecovers"
  "A_today:NodeBudget"
  "B_proposed:PASS"
  "C_no_lock:NodeBudget"
  "D_unsealed:CeilingMonotone"
)

cd "$SPEC_DIR"
rc=0
for case in "${CASES[@]}"; do
  cfg="${case%%:*}"; want="${case##*:}"
  out="$(java -XX:+UseParallelGC -cp "$JAR" tlc2.TLC \
           -config "$cfg.cfg" -workers auto -cleanup OrchLedger.tla 2>&1 || true)"

  if [ "$want" = "PASS" ]; then
    if grep -q "Model checking completed. No error has been found" <<<"$out"; then
      states=$(grep -oE "[0-9,]+ distinct states found" <<<"$out" | tail -1)
      printf '  ok   %-14s no error (%s)\n' "$cfg" "$states"
    else
      printf '  FAIL %-14s expected no error; got:\n' "$cfg"
      grep -E "Invariant .* is violated|Error:" <<<"$out" | head -3 | sed 's/^/       /'
      rc=1
    fi
  else
    # A safety violation reports "Invariant X is violated"; a liveness one reports
    # "Temporal properties were violated" without naming which. Accept either, and
    # require the named property to at least be under check in that config.
    if grep -q "Invariant $want is violated" <<<"$out" \
       || { grep -q "Temporal properties were violated" <<<"$out" \
            && grep -q "$want" "$cfg.cfg"; }; then
      printf '  ok   %-14s %s violated as expected\n' "$cfg" "$want"
    else
      printf '  FAIL %-14s expected %s to be violated; ablation no longer models the hazard\n' \
             "$cfg" "$want"
      rc=1
    fi
  fi
done

[ $rc -eq 0 ] && echo "check-spec: all configurations behaved as specified"
exit $rc
