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

check_contract() {
  local root="$1"
  local DOC="$root/docs/LAUNCH_TARGET_RESOLUTION.md"
  local SRC="$root/crates/amplihack-utils/src/launch_target.rs"
  [ -f "$DOC" ] || { echo "missing $DOC (run from repo root)"; return 1; }
  [ -f "$SRC" ] || { echo "missing $SRC"; return 1; }
  local fails=0 sig name hit actual want _ml owner_module
  local sigs=()
  pass() { printf '  ok    %s\n' "$1"; }
  fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

  # Signatures the doc declares, inside its fenced Rust blocks. Trailing `;` is
  # the doc's convention for "declaration only"; the source has `{` instead.
  # while-read, not mapfile: macOS /bin/bash is 3.2 (issue #1423).
  sigs=(); while IFS= read -r _ml; do sigs+=("$_ml"); done < <(
    grep -oE '^pub fn [a-z_]+\([^)]*\)( -> [^;]+)?;' "$DOC" \
      | sed 's/;$//' | sort -u
  )

  if [ "${#sigs[@]}" -eq 0 ]; then
    echo "  FAIL  the document declares no 'pub fn' signatures — this check would pass vacuously"
    return 1
  fi

  for sig in "${sigs[@]}"; do
    name=$(sed -E 's/^pub fn ([a-z_]+)\(.*/\1/' <<<"$sig")
    # The Scope lists legitimate sibling owners. A same-name function in any
    # other module cannot satisfy a declaration of this frozen contract.
    case "$name" in
      resolve|resolve_uncached|amplihack_prefix_bin)
        owner_module="$SRC" ;;
      is_materialized|claude_platform_packages)
        owner_module="$root/crates/amplihack-utils/src/claude_native.rs" ;;
      *) fail "doc declares '$name' without a documented ownership mapping"; continue ;;
    esac
    if [ ! -f "$owner_module" ]; then
      fail "missing owning module $owner_module for '$name'"
      continue
    fi
    # Consume the whole file (no early-terminating pipeline), and retain the
    # complete declaration through its opening body brace, including multiline
    # signatures. Multiple owning declarations cannot silently pick a first hit.
    hit=$(awk -v name="$name" '
      $0 ~ "^[[:space:]]*pub fn " name "\\(" { capture=1; signature="" }
      capture {
        signature=signature " " $0
        if (index($0, "{")) {
          sub(/[[:space:]]*\{.*/, "", signature)
          print signature
          capture=0
        }
      }
    ' "$owner_module")
    if [ -z "$hit" ]; then
      fail "doc declares '$name', which no longer exists in its owning module $owner_module"
      continue
    fi
    # Normalize whitespace only; preserve every parameter and return type.
    actual=$(printf '%s' "$hit" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//; s/[[:space:]][[:space:]]*/ /g')
    want=$(printf '%s' "$sig" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//; s/[[:space:]][[:space:]]*/ /g')
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
    return 1
  fi
  echo "launch-target contract: all ${#sigs[@]} documented signature(s) match the code"
}

# Guard regressions use a private source tree, leaving the real modules alone.
# Matching same-name declarations elsewhere must never satisfy the owner.
fixture=$(mktemp -d "${TMPDIR:-/tmp}/launch-contract.XXXXXX") || exit 1
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/docs" "$fixture/crates/amplihack-utils/src" \
    "$fixture/crates/amplihack-cli/src" "$fixture/crates/amplihack-launcher/src"
cp docs/LAUNCH_TARGET_RESOLUTION.md "$fixture/docs/"
cp crates/amplihack-utils/src/launch_target.rs "$fixture/crates/amplihack-utils/src/"
cp crates/amplihack-utils/src/claude_native.rs "$fixture/crates/amplihack-utils/src/"
owner="$fixture/crates/amplihack-utils/src/launch_target.rs"
original="$fixture/original.rs"
cp "$owner" "$original"
matching='pub fn resolve(tool: &str, override_origin: OverrideOrigin) -> Resolution {'
printf '%s\n' "$matching" > "$fixture/crates/amplihack-utils/src/agent_binary.rs"
controls=0
sed 's/^pub fn resolve(tool: &str, override_origin: OverrideOrigin) -> Resolution {/pub fn resolve(tool: WrongType, override_origin: OverrideOrigin) -> Resolution {/' "$original" > "$owner"
if check_contract "$fixture" > "$fixture/mismatch.log"; then
    echo '  FAIL  wrong owning signature was hidden by an unrelated matching declaration'
    controls=$((controls + 1))
elif grep -q "resolve has drifted from the frozen contract" "$fixture/mismatch.log"; then
    echo '  ok    wrong owning signature is rejected with unrelated matching resolve present'
else
    cat "$fixture/mismatch.log"
    controls=$((controls + 1))
fi
sed '/^pub fn resolve(tool: &str, override_origin: OverrideOrigin) -> Resolution {/d' "$original" > "$owner"
if check_contract "$fixture" > "$fixture/missing.log"; then
    echo '  FAIL  missing owning declaration was hidden by unrelated same-name functions'
    controls=$((controls + 1))
elif grep -q "doc declares 'resolve', which no longer exists in its owning module" "$fixture/missing.log"; then
    echo '  ok    missing owning declaration is rejected with unrelated resolve present'
else
    cat "$fixture/missing.log"
    controls=$((controls + 1))
fi
cp "$original" "$owner"
for decoy in agent_binary.rs aaa_unrelated.rs zzz_unrelated.rs; do
    printf '%s\n' 'pub fn resolve(cwd: &Path) -> Unrelated {' \
        'pub fn resolve_uncached() -> Unrelated {' > "$fixture/crates/amplihack-utils/src/$decoy"
    if check_contract "$fixture" > "$fixture/order.log"; then
        echo "  ok    unrelated declaration/order $decoy does not change ownership"
    else
        cat "$fixture/order.log"
        echo "  FAIL  unrelated declaration/order $decoy changed the contract result"
        controls=$((controls + 1))
    fi
done
check_contract . || exit 1
[ "$controls" -eq 0 ] || exit 1
