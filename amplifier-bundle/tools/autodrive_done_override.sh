#!/usr/bin/env bash
# Compatibility adapter: inconsistent DONE always requests reassessment (#2333).
# Convergence termination belongs to the evaluator, including repeated verdicts.
set -uo pipefail
[ "$#" -eq 8 ] || { echo "ERROR: expected eight observation arguments" >&2; exit 2; }
printf '%s\n' 'DONE_WITH_NONCLEAN_ROUND'
exit 0
