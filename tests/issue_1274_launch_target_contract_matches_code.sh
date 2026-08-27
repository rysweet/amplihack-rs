#!/usr/bin/env bash
# `docs/LAUNCH_TARGET_RESOLUTION.md` calls itself the frozen contract for
# `launch_target`'s public signatures, and `launch_target.rs` opens with:
#
#     Do not change the signatures below without updating
#     `docs/LAUNCH_TARGET_RESOLUTION.md`, which is the frozen contract.
#
# That rule was honour-system, and it failed. #1276 added an `OverrideOrigin`
# parameter to `resolve` and `resolve_uncached`; the code changed, the doc did
# not, and nothing noticed. A contract nobody checks is a comment.
#
# So: every `pub fn` signature the document declares must exist verbatim in the
# source. The document is the authority on WHICH functions are contractual —
# listing one is what opts it in — and the code is the authority on their shape.

set -uo pipefail

DOC="docs/LAUNCH_TARGET_RESOLUTION.md"
SRC="crates/amplihack-utils/src/launch_target.rs"
# The document's Scope line covers several modules, not just launch_target, so
# a declared signature may legitimately live in a sibling (e.g. claude_native).
SRC_DIRS="crates/amplihack-utils/src crates/amplihack-cli/src crates/amplihack-launcher/src"
[ -f "$DOC" ] || { echo "missing $DOC (run from repo root)"; exit 1; }
[ -f "$SRC" ] || { echo "missing $SRC"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

# Signatures the doc declares, inside its fenced Rust blocks. Trailing `;` is
# the doc's convention for "declaration only"; the source has `{` instead.
mapfile -t sigs < <(
  grep -oE '^pub fn [a-z_]+\([^)]*\)( -> [A-Za-z_:<>&, ]+)?;' "$DOC" \
    | sed 's/;$//' | sort -u
)

if [ "${#sigs[@]}" -eq 0 ]; then
  echo "  FAIL  the document declares no 'pub fn' signatures — this check would pass vacuously"
  exit 1
fi

for sig in "${sigs[@]}"; do
  name=$(sed -E 's/^pub fn ([a-z_]+)\(.*/\1/' <<<"$sig")
  # shellcheck disable=SC2086
  hit=$(grep -rhE "^pub fn ${name}\(" $SRC_DIRS 2>/dev/null | head -1)
  if [ -z "$hit" ]; then
    fail "doc declares '$name', which no longer exists in the documented scope"
    continue
  fi
  # Compare the source's signature with the doc's, normalised for whitespace.
  # `{` is literal in a BRE; escaping it starts an interval expression.
  actual=$(printf '%s' "$hit" | sed 's/[[:space:]]*{[[:space:]]*$//' | tr -s ' ')
  want=$(tr -s ' ' <<<"$sig")
  if [ "$actual" = "$want" ]; then
    pass "$name matches the documented signature"
  else
    fail "$name has drifted from the frozen contract
          doc:  $want
          code: $actual"
  fi
done

# The module comment is what tells the next person the rule exists. If it goes,
# this check is the only thing left holding the contract together.
if grep -q 'frozen contract' "$SRC"; then
  pass "launch_target.rs still points at the frozen contract"
else
  fail "launch_target.rs no longer references the frozen contract doc"
fi

echo
if [ "$fails" -gt 0 ]; then
  echo "launch-target contract: $fails mismatch(es)"
  exit 1
fi
echo "launch-target contract: all ${#sigs[@]} documented signature(s) match the code"
