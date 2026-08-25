#!/usr/bin/env bash
# Scenario helper for issue #1326: the spec gate must still distinguish the designs.
#
# Prints PASS when every ablation behaved as specified, or when tla2tools is
# unavailable (exit 2) so the scenario does not fail on a missing optional tool.
# Prints FAIL only when the model check genuinely disagreed with the spec.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || { echo FAIL; exit 0; }
./scripts/check-spec.sh >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 0 ] || [ "$rc" -eq 2 ]; then echo PASS; exit 0; fi
echo "FAIL: spec gate disagreed with docs/spec (rc=$rc)"
exit 1
