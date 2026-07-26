#!/usr/bin/env bash
# TDD contract tests for the content ABSORBED from PR #1001 into the
# `signal-setup` skill:
#
#   1. amplifier-bundle/skills/signal-setup/README.md   (the ported overview)
#   2. docs/skills/SKILL_CATALOG.md registration        (the catalog row)
#
# These two artifacts previously had ZERO executable coverage. This suite is
# their executable specification. Each assertion names exactly which part of the
# absorbed contract is unmet when it fails.
#
# Run: bash amplifier-bundle/skills/signal-setup/tests/test_readme_and_catalog.sh
#
# Self-contained: no network, no build. grep/awk only, matching the pattern used
# by test_skill_structure.sh in this same directory.
#
# The absorption had two hard constraints beyond "the files exist":
#   * README content must stay consistent with the SKILL.md contract (the same
#     hard-won facts: 60s/code-1001 window, ANSIUTF8i, systemd-run, daemon on
#     127.0.0.1:7583, the --host contract, idempotency, never-relay-the-QR).
#   * The catalog row must sit in the correct alphabetical slot AND keep the
#     summary counts self-consistent (declared == rows == unique names), or the
#     catalog self-check fails. Version-bump files must NOT have been dragged in.

set -uo pipefail

PASS=0
FAIL=0
pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_DIR="$(dirname "$SCRIPT_DIR")"
README="$SKILL_DIR/README.md"
SKILL_FILE="$SKILL_DIR/SKILL.md"
USAGE_FILE="$SKILL_DIR/USAGE.md"
IMPL="$SKILL_DIR/scripts/signal-setup.sh"

# Locate the repo root so we can find docs/skills/SKILL_CATALOG.md regardless of
# where the runner is invoked from. Walk up until we find the docs catalog.
find_catalog() {
  local d="$SKILL_DIR"
  while [[ "$d" != "/" ]]; do
    if [[ -f "$d/docs/skills/SKILL_CATALOG.md" ]]; then
      echo "$d/docs/skills/SKILL_CATALOG.md"
      return 0
    fi
    d="$(dirname "$d")"
  done
  return 1
}
CATALOG="$(find_catalog || true)"

# grep helpers (case-insensitive by default; _f = fixed string).
g()  { grep -qiE -- "$1" "$2" 2>/dev/null; }
gf() { grep -qiF -- "$1" "$2" 2>/dev/null; }

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: signal-setup — Absorbed README + Catalog"
echo "═══════════════════════════════════════════════════════"

# ─── Test 1: README exists and its internal links resolve ────────────────────
echo ""
echo "Test 1: README.md exists and links to real targets"

if [[ -f "$README" ]]; then
  pass "README.md exists"
else
  fail "README.md not found at $README"
  echo "Results: $PASS passed, $FAIL failed"; exit 1
fi

# The README advertises these three companion files; each link target must exist.
if gf "(./SKILL.md)" "$README" && [[ -f "$SKILL_FILE" ]]; then
  pass "README links to SKILL.md and target resolves"
else
  fail "README must link to ./SKILL.md and the file must exist"
fi

if gf "(./USAGE.md)" "$README" && [[ -f "$USAGE_FILE" ]]; then
  pass "README links to USAGE.md and target resolves"
else
  fail "README must link to ./USAGE.md and the file must exist"
fi

if gf "(./scripts/signal-setup.sh)" "$README" && [[ -f "$IMPL" ]]; then
  pass "README links to scripts/signal-setup.sh and target resolves"
else
  fail "README must link to ./scripts/signal-setup.sh and the file must exist"
fi

# ─── Test 2: README preserves the hard-won facts (consistency w/ SKILL.md) ────
echo ""
echo "Test 2: README preserves the skill's hard-won facts"

# 60-second provisioning window + close code 1001.
if g "60" "$README" && g "1001" "$README"; then
  pass "README documents the ~60s window and close code 1001"
else
  fail "README must document the 60s window AND socket close code 1001"
fi

# ANSIUTF8i inverted QR rendering (dark-terminal-safe).
if gf "ANSIUTF8i" "$README"; then
  pass "README documents ANSIUTF8i (inverted) QR rendering"
else
  fail "README must name the ANSIUTF8i inverted QR mode"
fi

# systemd-run persistence, including the remote --uid=azureuser gotcha.
if gf "systemd-run" "$README" && gf "--uid=azureuser" "$README"; then
  pass "README documents systemd-run persistence + remote --uid=azureuser"
else
  fail "README must document systemd-run and the --uid=azureuser remote gotcha"
fi

# Daemon endpoint 127.0.0.1:7583.
if gf "127.0.0.1:7583" "$README"; then
  pass "README documents the daemon endpoint 127.0.0.1:7583"
else
  fail "README must name the JSON-RPC daemon endpoint 127.0.0.1:7583"
fi

# The --host contract is the primary interface.
if gf "--host" "$README"; then
  pass "README documents the --host contract"
else
  fail "README must document the --host flag (primary interface)"
fi

# Idempotency guarantee.
if g "idempotent" "$README"; then
  pass "README documents idempotency"
else
  fail "README must state the command is idempotent"
fi

# Never relay the QR through Signal itself (the deprecated slow path).
if g "[Nn]ever .*(through|via) Signal" "$README" || g "never deliver the QR" "$README"; then
  pass "README warns: never deliver the QR through Signal itself"
else
  fail "README must warn against delivering the QR through Signal (deprecated relay)"
fi

# ─── Test 3: Catalog registration — presence + exact bundle path ─────────────
echo ""
echo "Test 3: SKILL_CATALOG.md registers signal-setup"

if [[ -n "$CATALOG" && -f "$CATALOG" ]]; then
  pass "SKILL_CATALOG.md located"
else
  fail "docs/skills/SKILL_CATALOG.md not found by walking up from skill dir"
  echo "Results: $PASS passed, $FAIL failed"; exit 1
fi

# The row must use the exact bundle path the generator emits.
# shellcheck disable=SC2016  # literal backticks are catalog markdown, not expansion
if grep -qF '| `signal-setup` | `signal-setup/SKILL.md` |' "$CATALOG"; then
  pass "catalog contains the signal-setup row with correct bundle path"
else
  fail "catalog must contain: | \`signal-setup\` | \`signal-setup/SKILL.md\` |"
fi

# ─── Test 4: Catalog row sits in the correct alphabetical slot ───────────────
echo ""
echo "Test 4: signal-setup occupies the correct alphabetical slot"

# Ordered list of skill names exactly as they appear as table rows.
# shellcheck disable=SC2016  # literal backticks are catalog markdown, not expansion
mapfile -t CAT_NAMES < <(grep -oE '^\| `[a-z0-9-]+`' "$CATALOG" | sed -E 's/^\| `([a-z0-9-]+)`/\1/')

# Find signal-setup's neighbours in emission order.
prev=""; next=""; found=""
for i in "${!CAT_NAMES[@]}"; do
  if [[ "${CAT_NAMES[$i]}" == "signal-setup" ]]; then
    found=1
    [[ $i -gt 0 ]] && prev="${CAT_NAMES[$((i-1))]}"
    next_idx=$((i+1))
    [[ $next_idx -lt ${#CAT_NAMES[@]} ]] && next="${CAT_NAMES[$next_idx]}"
    break
  fi
done

if [[ -n "$found" ]]; then
  pass "signal-setup present in catalog row list"
else
  fail "signal-setup missing from catalog row list"
fi

# 'sig' sorts after 'sha' (shadow-*) and before 'sil' (silent-*).
if [[ "$prev" == "shadow-testing" ]]; then
  pass "predecessor row is shadow-testing (correct)"
else
  fail "predecessor should be shadow-testing, got '${prev:-<none>}'"
fi

if [[ "$next" == "silent-degradation-audit" ]]; then
  pass "successor row is silent-degradation-audit (correct)"
else
  fail "successor should be silent-degradation-audit, got '${next:-<none>}'"
fi

# Whole table must be strictly ascending (defends the slot globally).
sorted_ok=1
for ((i=1; i<${#CAT_NAMES[@]}; i++)); do
  if [[ "${CAT_NAMES[$((i-1))]}" > "${CAT_NAMES[$i]}" ]]; then
    sorted_ok=0
    echo "    ordering break: '${CAT_NAMES[$((i-1))]}' > '${CAT_NAMES[$i]}'"
  fi
done
if [[ "$sorted_ok" -eq 1 ]]; then
  pass "catalog rows are in strict alphabetical order"
else
  fail "catalog rows are NOT strictly alphabetical (see breaks above)"
fi

# ─── Test 5: Catalog summary counts stay self-consistent ─────────────────────
echo ""
echo "Test 5: catalog summary counts are self-consistent"

row_count="${#CAT_NAMES[@]}"
uniq_count="$(printf '%s\n' "${CAT_NAMES[@]}" | sort -u | wc -l | tr -d ' ')"
declared_unique="$(grep -oE 'Unique bundled skill names:\*\* [0-9]+' "$CATALOG" | grep -oE '[0-9]+' | head -1)"
declared_defs="$(grep -oE 'Skill definition files:\*\* [0-9]+' "$CATALOG" | grep -oE '[0-9]+' | head -1)"

if [[ "$row_count" -eq "$uniq_count" ]]; then
  pass "no duplicate skill rows ($row_count rows == $uniq_count unique)"
else
  fail "duplicate rows: $row_count rows but only $uniq_count unique names"
fi

if [[ -n "$declared_unique" && "$declared_unique" -eq "$uniq_count" ]]; then
  pass "declared 'Unique bundled skill names' ($declared_unique) == actual unique ($uniq_count)"
else
  fail "declared unique count '${declared_unique:-<none>}' != actual unique $uniq_count"
fi

if [[ -n "$declared_defs" && "$declared_defs" -eq "$row_count" ]]; then
  pass "declared 'Skill definition files' ($declared_defs) == actual rows ($row_count)"
else
  fail "declared definition count '${declared_defs:-<none>}' != actual rows $row_count"
fi

# ─── Test 6: Absorption dragged in NO version-bump files ─────────────────────
echo ""
echo "Test 6: version-bump exclusion invariant"

# The absorption was documentation-only. Neither the skill dir nor the catalog
# change should have introduced dependency/version manifests. A stray Cargo.toml
# or package.json inside the skill would signal an over-broad cherry-pick.
if find "$SKILL_DIR" -maxdepth 3 \( -name Cargo.toml -o -name Cargo.lock -o -name package.json \) 2>/dev/null | grep -q .; then
  fail "signal-setup skill dir must not contain Cargo/package manifests"
  find "$SKILL_DIR" -maxdepth 3 \( -name Cargo.toml -o -name Cargo.lock -o -name package.json \) 2>/dev/null | sed 's/^/    stray: /'
else
  pass "no Cargo/package manifests under the skill dir (version-bump excluded)"
fi

echo ""
echo "Results: $PASS passed, $FAIL failed"
[[ "$FAIL" -eq 0 ]]
