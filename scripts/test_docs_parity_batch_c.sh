#!/usr/bin/env bash
# Test suite for Issue #420 Doc Parity Batch C.
#
# Validates the contract for ported LadybugDB / memory-tree docs:
#  - required files exist at correct Diataxis paths with kebab-case names
#  - hard-fail content checks pass (no Python-only fences, pip install,
#    `import kuzu` remnants, or leaked secrets)
#  - docs/index.md links the three new docs
#  - all internal "See also" links resolve
#  - lbug crate version reference (if any) matches workspace pin (0.15.3)
#  - KUZU_* env-var references appear only as legacy alias notes
#
# Exit non-zero on any failure. Run from repo root or the worktree root.

set -u

DOCS_DIR="${DOCS_DIR:-docs}"
INDEX="$DOCS_DIR/index.md"
PORTED=(
  "$DOCS_DIR/reference/ladybug-reference.md"
  "$DOCS_DIR/concepts/memory-tree.md"
  "$DOCS_DIR/concepts/five-type-memory.md"
)

pass=0
fail=0
report() {
  local status="$1" msg="$2"
  if [[ "$status" == "PASS" ]]; then
    printf '  \033[32mPASS\033[0m %s\n' "$msg"
    pass=$((pass + 1))
  else
    printf '  \033[31mFAIL\033[0m %s\n' "$msg"
    fail=$((fail + 1))
  fi
}

echo "==> [1/7] Required files exist"
for f in "${PORTED[@]}"; do
  if [[ -f "$f" && -s "$f" ]]; then
    report PASS "exists and non-empty: $f"
  else
    report FAIL "missing or empty: $f"
  fi
done

echo "==> [2/7] Filenames are kebab-case"
for f in "${PORTED[@]}"; do
  base="$(basename "$f" .md)"
  if [[ "$base" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
    report PASS "kebab-case: $base"
  else
    report FAIL "not kebab-case: $base"
  fi
done

echo "==> [3/7] Hard-fail content checks (no Python-only verbatim, no secrets)"
for f in "${PORTED[@]}"; do
  [[ -f "$f" ]] || continue
  if grep -Eq '^```(python|py)\s*$' "$f"; then
    report FAIL "python code fence in $f"
  else
    report PASS "no python fence: $f"
  fi
  if grep -Eq 'pip install' "$f"; then
    report FAIL "pip install reference in $f"
  else
    report PASS "no pip install: $f"
  fi
  if grep -Eq '^[[:space:]]*import[[:space:]]+kuzu\b' "$f"; then
    report FAIL "import kuzu remnant in $f"
  else
    report PASS "no import kuzu: $f"
  fi
  if grep -Eq 'ghp_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|BEGIN PRIVATE KEY' "$f"; then
    report FAIL "possible secret in $f"
  else
    report PASS "no secret pattern: $f"
  fi
done

echo "==> [4/7] docs/index.md links every ported doc"
if [[ -f "$INDEX" ]]; then
  for f in "${PORTED[@]}"; do
    rel="${f#$DOCS_DIR/}"
    if grep -Fq "$rel" "$INDEX"; then
      report PASS "index links $rel"
    else
      report FAIL "index missing link to $rel"
    fi
  done
else
  report FAIL "missing $INDEX"
fi

echo "==> [5/7] Internal links resolve"
for f in "${PORTED[@]}"; do
  [[ -f "$f" ]] || continue
  # Extract markdown link targets that look like local relative paths.
  while IFS= read -r target; do
    [[ -z "$target" ]] && continue
    # Strip optional #anchor.
    path="${target%%#*}"
    [[ -z "$path" ]] && continue   # pure-anchor link, skip
    # Resolve relative to the file's directory.
    dir="$(dirname "$f")"
    abs="$(cd "$dir" && cd "$(dirname "$path")" 2>/dev/null && pwd)/$(basename "$path")"
    if [[ -e "$abs" ]]; then
      report PASS "link ok in $(basename "$f"): $target"
    else
      report FAIL "broken link in $f: $target"
    fi
  done < <(grep -oE '\]\(\.{1,2}/[^)]+\)' "$f" | sed -E 's/^\]\(//; s/\)$//')
done

echo "==> [6/7] lbug version pin (when referenced) matches workspace (0.15.3)"
for f in "${PORTED[@]}"; do
  [[ -f "$f" ]] || continue
  if grep -Eq 'lbug[[:space:]]*=' "$f"; then
    if grep -Eq 'lbug[[:space:]]*=[[:space:]]*"0\.15\.3"' "$f"; then
      report PASS "lbug pin correct in $(basename "$f")"
    else
      report FAIL "lbug version mismatch in $f (expected 0.15.3)"
    fi
  else
    report PASS "no lbug version reference in $(basename "$f") (skipped)"
  fi
done

echo "==> [7/7] KUZU_* env-vars appear only as legacy alias notes"
for f in "${PORTED[@]}"; do
  [[ -f "$f" ]] || continue
  if grep -Eq 'KUZU_[A-Z_]+' "$f"; then
    # Each line mentioning KUZU_ must also mention "legacy" or "alias" within
    # 2 lines of context, or be inside a table row that contains those words.
    bad=$(grep -nE 'KUZU_[A-Z_]+' "$f" | while IFS=: read -r line _; do
      ctx=$(awk -v L="$line" 'NR>=L-2 && NR<=L+2' "$f")
      if echo "$ctx" | grep -Eqi 'legacy|alias|deprecated|preserved|compat'; then
        :
      else
        echo "line $line"
      fi
    done)
    if [[ -z "$bad" ]]; then
      report PASS "KUZU_* references annotated in $(basename "$f")"
    else
      report FAIL "KUZU_* not annotated as legacy in $f: $bad"
    fi
  else
    report PASS "no KUZU_* references in $(basename "$f") (skipped)"
  fi
done

echo
echo "==> Summary: $pass passed, $fail failed"
[[ "$fail" -eq 0 ]]
