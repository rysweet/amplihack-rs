#!/usr/bin/env bash
# TDD tests for issue #935 — publish-snapshot cross-target E0463 fix.
#
# Root cause (verified): rust-toolchain.toml pins channel = "1.97.0", but
# .github/workflows/publish-snapshot.yml used `dtolnay/rust-toolchain@stable`
# with `targets: ${{ matrix.target }}`. dtolnay@stable adds the cross target's
# rust-std to the STABLE toolchain, but at build time rust-toolchain.toml
# overrides resolution to 1.97.0 — which never had that target's std added —
# yielding `error[E0463]: can't find crate for std` on the
# aarch64-unknown-linux-gnu leg. ci.yml's Build job is green because it pins
# `dtolnay/rust-toolchain@1.97.0` (matching the toml) with the same targets.
#
# Contract:
#   1. The fix: publish-snapshot.yml pins the toolchain to the exact channel in
#      rust-toolchain.toml and keeps `targets: ${{ matrix.target }}`.
#   2. Invariant: every `dtolnay/rust-toolchain@<ref>` step that also declares a
#      `targets:` key (i.e. provisions a cross-target's rust-std) MUST pin
#      <ref> to the rust-toolchain.toml channel. Scan/native-only steps (no
#      `targets:`) may stay on `@stable` by design.
#   3. A reusable guard, scripts/check-toolchain-refs.sh, enforces invariant (2)
#      so this drift class cannot silently regress. It exits non-zero on a
#      drifted `targets:` ref and zero on scan-only `@stable` refs.
#   4. Bugfix hygiene: this change does NOT bump the workspace version.
#
# Expected BEFORE the guard exists: FAIL (guard-script cases).
# Expected AFTER implementation:    PASS (all cases).
#
# Run: bash tests/issue_935_toolchain_ref_pin_test.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKFLOWS="$REPO_ROOT/.github/workflows"
TOOLCHAIN_TOML="$REPO_ROOT/rust-toolchain.toml"
SNAPSHOT="$WORKFLOWS/publish-snapshot.yml"
CI="$WORKFLOWS/ci.yml"
GUARD="$REPO_ROOT/scripts/check-toolchain-refs.sh"

pass=0
fail=0
TMPROOT=""

cleanup() { [ -n "$TMPROOT" ] && rm -rf "$TMPROOT"; }
trap cleanup EXIT

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    if [ $# -gt 1 ] && [ -n "$2" ]; then
        printf '      %s\n' "$2"
    fi
    fail=$((fail + 1))
}

assert() {
    local desc="$1"
    local cond="$2"
    if eval "$cond"; then
        record_pass "$desc"
    else
        record_fail "$desc" "condition: $cond"
    fi
}

# Extract the pinned channel from rust-toolchain.toml (e.g. 1.97.0).
toolchain_channel() {
    sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$TOOLCHAIN_TOML" | head -n1
}

# Return the @ref for every dtolnay/rust-toolchain step in a file that also
# declares a `targets:` key within the following few lines of the step.
targets_bearing_refs() {
    local file="$1"
    awk '
        /dtolnay\/rust-toolchain@/ {
            ref = $0
            sub(/.*dtolnay\/rust-toolchain@/, "", ref)
            sub(/[[:space:]].*/, "", ref)
            window = 6
            hit = 0
            for (i = 1; i <= window; i++) {
                if ((getline line) <= 0) break
                if (line ~ /^[[:space:]]*targets:/) { hit = 1; break }
                if (line ~ /dtolnay\/rust-toolchain@/) break
            }
            if (hit) print ref
        }
    ' "$file"
}

echo "=== issue #935 toolchain-ref pin TDD tests ==="
echo "Workflows: $WORKFLOWS"
echo "Toolchain: $TOOLCHAIN_TOML"
echo "Guard:     $GUARD"
echo

CHANNEL="$(toolchain_channel)"
assert "rust-toolchain.toml declares a channel" "[ -n '$CHANNEL' ]"
echo "Resolved channel: ${CHANNEL:-<none>}"
echo

# ---------------------------------------------------------------------------
# 1. The fix: publish-snapshot.yml
# ---------------------------------------------------------------------------
assert "publish-snapshot.yml exists" "[ -f '$SNAPSHOT' ]"

assert "publish-snapshot.yml pins dtolnay/rust-toolchain to the toml channel ($CHANNEL)" \
    "grep -q 'dtolnay/rust-toolchain@$CHANNEL' '$SNAPSHOT'"

assert "publish-snapshot.yml no longer uses dtolnay/rust-toolchain@stable" \
    "! grep -q 'dtolnay/rust-toolchain@stable' '$SNAPSHOT'"

assert "publish-snapshot.yml preserves 'targets: \${{ matrix.target }}'" \
    "grep -Eq 'targets:[[:space:]]*\\\$\\{\\{[[:space:]]*matrix.target[[:space:]]*\\}\\}' '$SNAPSHOT'"

# Every targets-bearing ref in the snapshot workflow must equal the channel.
snap_bad=""
while IFS= read -r ref; do
    [ -n "$ref" ] || continue
    [ "$ref" = "$CHANNEL" ] || snap_bad="$snap_bad $ref"
done < <(targets_bearing_refs "$SNAPSHOT")
assert "publish-snapshot.yml has no drifted targets-bearing refs" "[ -z '$snap_bad' ]"

# ---------------------------------------------------------------------------
# 2. Cross-workflow invariant: ci.yml already proves the correct pattern.
# ---------------------------------------------------------------------------
ci_bad=""
while IFS= read -r ref; do
    [ -n "$ref" ] || continue
    [ "$ref" = "$CHANNEL" ] || ci_bad="$ci_bad $ref"
done < <(targets_bearing_refs "$CI")
assert "ci.yml (green reference) has all targets-bearing refs pinned to channel" "[ -z '$ci_bad' ]"

# ---------------------------------------------------------------------------
# 3. The guard script: scripts/check-toolchain-refs.sh
# ---------------------------------------------------------------------------
assert "guard script exists" "[ -f '$GUARD' ]"
assert "guard script is executable" "[ -x '$GUARD' ]"

if [ -x "$GUARD" ]; then
    TMPROOT="$(mktemp -d)"
    fx="$TMPROOT/fixtures"
    mkdir -p "$fx"
    cp "$TOOLCHAIN_TOML" "$TMPROOT/rust-toolchain.toml"

    # Fixture A: drifted — @stable WITH targets → must be flagged.
    cat > "$fx/drift-targets.yml" <<EOF
jobs:
  build:
    steps:
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-unknown-linux-gnu
EOF

    # Fixture B: scan-only — @stable with NO targets → exempt.
    cat > "$fx/scan-only.yml" <<EOF
jobs:
  scan:
    steps:
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
EOF

    # Fixture C: correct — pinned to channel WITH targets → clean.
    cat > "$fx/pinned-targets.yml" <<EOF
jobs:
  build:
    steps:
      - uses: dtolnay/rust-toolchain@$CHANNEL
        with:
          targets: aarch64-unknown-linux-gnu
EOF

    run_guard() {
        # Guard accepts explicit workflow paths and resolves the channel from
        # the nearest rust-toolchain.toml (repo root by default). Pass the toml
        # via env so fixtures are self-contained.
        TOOLCHAIN_TOML="$TMPROOT/rust-toolchain.toml" \
            bash "$GUARD" "$@" >/dev/null 2>&1
    }

    if run_guard "$fx/drift-targets.yml"; then
        record_fail "guard flags @stable + targets drift (fixture A)" "expected non-zero exit"
    else
        record_pass "guard flags @stable + targets drift (fixture A)"
    fi

    if run_guard "$fx/scan-only.yml"; then
        record_pass "guard ignores scan-only @stable (no targets) (fixture B)"
    else
        record_fail "guard ignores scan-only @stable (no targets) (fixture B)" "expected zero exit"
    fi

    if run_guard "$fx/pinned-targets.yml"; then
        record_pass "guard accepts channel-pinned + targets (fixture C)"
    else
        record_fail "guard accepts channel-pinned + targets (fixture C)" "expected zero exit"
    fi

    # The real, fixed snapshot workflow must pass the guard.
    if bash "$GUARD" "$SNAPSHOT" >/dev/null 2>&1; then
        record_pass "guard passes on the fixed publish-snapshot.yml"
    else
        record_fail "guard passes on the fixed publish-snapshot.yml" "expected zero exit"
    fi

    # Default scan (no args) must be CI-wireable: exit 0. Any remaining
    # same-class drift (e.g. release.yml) must be resolved or explicitly
    # allowlisted by the guard so the failure stays visible, never silent.
    if bash "$GUARD" >/dev/null 2>&1; then
        record_pass "guard default scan is green (CI-wireable)"
    else
        record_fail "guard default scan is green (CI-wireable)" \
            "unresolved same-class toolchain-ref drift in .github/workflows (fix or allowlist it)"
    fi
fi

# ---------------------------------------------------------------------------
# 4. Bugfix hygiene: no workspace version bump on this change.
# ---------------------------------------------------------------------------
base_ref=""
if git -C "$REPO_ROOT" rev-parse --verify -q origin/main >/dev/null 2>&1; then
    base_ref="origin/main"
elif git -C "$REPO_ROOT" rev-parse --verify -q main >/dev/null 2>&1; then
    base_ref="main"
fi

if [ -n "$base_ref" ]; then
    if git -C "$REPO_ROOT" diff "$base_ref"...HEAD -- Cargo.toml \
        | grep -Eq '^[+-]version[[:space:]]*='; then
        record_fail "bugfix does not bump the workspace version" \
            "Cargo.toml version line changed vs $base_ref"
    else
        record_pass "bugfix does not bump the workspace version"
    fi
else
    record_pass "base branch unavailable; version-bump check skipped"
fi

echo
echo "=== Results: $pass passed, $fail failed ==="
if [ "$fail" -ne 0 ]; then
    exit 1
fi
