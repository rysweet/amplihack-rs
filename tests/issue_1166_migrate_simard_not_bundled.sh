#!/usr/bin/env bash
# Issue #1166 — `amplihack:migrate` must not carry ~/.simard by default, and
# must not blind-overlay a populated store on the destination.
#
# migrate.sh bundled "$HOME/.simard" unconditionally and extracted the archive
# with `tar -xpf -C /`. That turned a ~79 MB session move into a 10.6 GB
# transfer, and on a destination that was itself a live Simard host it would
# have replaced a canonical 2.0 GB store — cognitive memory, state/, typed-ooda/,
# goal tombstones — with the source host's copy. There is no merge in a tar
# overlay and no prompt. It was caught by hand, one step before extraction.
#
# ~/.simard is host-local runtime state, not session state, so the default is
# now to leave it behind entirely.

set -euo pipefail

SCRIPT="amplifier-bundle/skills/migrate/scripts/migrate.sh"
fails=0

pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

[[ -f "$SCRIPT" ]] || { echo "missing $SCRIPT (run from repo root)"; exit 1; }

# --- 1. the default include list must not carry ~/.simard -------------------
# Scope to the loop that builds TAR_INCLUDES; a mention elsewhere (an --exclude,
# the opt-in branch, a comment) is fine and expected.
includes_block=$(awk '/^for p in \\$/{f=1} f{print} /^done$/{if(f) exit}' "$SCRIPT")
if [[ -z "$includes_block" ]]; then
  fail "could not locate the TAR_INCLUDES loop — this test would pass vacuously"
elif grep -q '\.simard' <<<"$includes_block"; then
  fail "~/.simard is still in the unconditional include list"
else
  pass "~/.simard is not in the default include list"
fi

# Sanity: the loop still bundles what migration actually needs, so a fix that
# empties the list altogether does not read as success.
for expected in '.config' '.amplihack' '.ssh'; do
  grep -q "$expected" <<<"$includes_block" \
    && pass "default include list still carries ~/$expected" \
    || fail "default include list lost ~/$expected"
done

# --- 2. opting in is possible, and heavy subdirs stay out --------------------
grep -q -- '--include-simard' "$SCRIPT" \
  && pass "--include-simard opt-in exists" \
  || fail "no --include-simard opt-in"

for heavy in self-deploy-target self-deploy-src bin backups; do
  grep -q -- "--exclude=\"\$HOME/.simard/$heavy\"" "$SCRIPT" \
    && pass "excludes .simard/$heavy" \
    || fail "does not exclude .simard/$heavy"
done

# --- 3. the overlay guard must run BEFORE the archive is shipped ------------
# A guard placed after `azlin cp` would still refuse, but only after pushing
# gigabytes across the network.
# first_line_matching <fixed-string> <file> — the 1-based line number of the
# first line containing <fixed-string>, or "" if there is none.
#
# One awk that reads the whole file. The `head`-terminated pipeline this
# replaces left `grep` writing into a closed pipe: under `pipefail` that is a
# non-zero pipeline whose substitution collapses to "", turning a real ordering
# check into a vacuous one — and whether it fires is a race on the pipe buffer,
# so it cannot be ruled out by running the test (issue #1434).
first_line_matching() {
  awk -v needle="$1" 'n == 0 && index($0, needle) { n = FNR } END { if (n) print n }' "$2"
}

guard_line=$(first_line_matching 'already has a populated ~/.simard' "$SCRIPT")
ship_line=$(first_line_matching 'azlin cp "$TARBALL"' "$SCRIPT")
extract_line=$(first_line_matching 'unzstd -xpf' "$SCRIPT")
if [[ -z "$guard_line" ]]; then
  fail "no destination-overlay guard found"
elif [[ -z "$ship_line" || -z "$extract_line" ]]; then
  fail "could not locate the ship/extract steps — ordering check is vacuous"
elif (( guard_line < ship_line && guard_line < extract_line )); then
  pass "overlay guard precedes both the copy (line $ship_line) and extract (line $extract_line)"
else
  fail "overlay guard at line $guard_line runs after copy/extract"
fi

# --- 4. behaviour: --force-simard-overlay alone is rejected -----------------
# Runs the real script. It must fail during argument validation, before it
# reaches the dependency check, so this works on a host without azlin.
set +e
out=$(bash "$SCRIPT" some-host --force-simard-overlay 2>&1)
rc=$?
set -e
if [[ $rc -eq 2 ]] && grep -q 'requires --include-simard' <<<"$out"; then
  pass "--force-simard-overlay without --include-simard exits 2 with a clear message"
else
  fail "--force-simard-overlay alone: expected exit 2 and a clear message, got exit $rc: ${out:0:200}"
fi

# --- 5. behaviour: unknown options still rejected ---------------------------
set +e
out=$(bash "$SCRIPT" some-host --include-simmard 2>&1); rc=$?
set -e
if [[ $rc -eq 2 ]]; then
  pass "a misspelled flag is still rejected (exit 2)"
else
  fail "misspelled flag returned $rc, expected 2"
fi

echo
if (( fails > 0 )); then
  echo "issue-1166: $fails check(s) failed"
  exit 1
fi
echo "issue-1166: all checks passed"
