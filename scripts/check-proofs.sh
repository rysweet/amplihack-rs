#!/usr/bin/env bash
# Proof gate for the spawn-accounting decisions (issue #1329).
#
# `docs/spec/OrchLedger.tla` covers ORDERING -- what happens when processes race,
# crash, and interleave. This covers LOGIC -- whether a single decision function can
# return a wrong answer for some input a test never tried. Both are needed: the two
# real defects in this area were one of each kind.
#
# Kani turns each `#[kani::proof]` harness into a constraint problem and asks a solver
# whether ANY input violates it. A pass means the claim holds for every input, not for
# the inputs someone thought of.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! command -v cargo-kani >/dev/null 2>&1; then
  echo "check-proofs: kani not installed — skipping" >&2
  echo "            install with: cargo install --locked kani-verifier && cargo kani setup" >&2
  exit 2
fi

# Every harness must pass, and there must BE harnesses: a proof suite that silently
# verifies nothing is worse than none, because it reports success.
out="$(cargo kani -p amplihack-cli 2>&1)"
echo "$out" | grep -E "Checking harness|VERIFICATION:" || true

if ! grep -qE "Complete - [0-9]+ successfully verified harnesses, 0 failures" <<<"$out"; then
  echo "check-proofs: FAILED" >&2
  grep -E "Failed Checks|VERIFICATION:- FAILED" <<<"$out" | head -20 >&2
  exit 1
fi

count=$(grep -oE "Complete - [0-9]+ successfully verified" <<<"$out" | grep -oE "[0-9]+" | head -1)
if [ "${count:-0}" -lt 1 ]; then
  echo "check-proofs: no harnesses ran — the proof gate is verifying nothing" >&2
  exit 1
fi
echo "check-proofs: $count harness(es) proved for all inputs"
