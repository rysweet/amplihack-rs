#!/usr/bin/env bash
# TDD tests for issue #939 — Auto Release cross-build E0463 fix.
#
# Root cause (verified): rust-toolchain.toml pins channel = "1.97.0", but
# .github/workflows/release.yml used `dtolnay/rust-toolchain@stable` with
# `targets: ${{ matrix.target }}` on its build matrix. dtolnay@stable adds the
# cross target's rust-std to the STABLE toolchain, but at build time
# rust-toolchain.toml overrides resolution back to 1.97.0 — which never had that
# target's std added — yielding `error[E0463]: can't find crate for std` on the
# cross-arch legs (`aarch64-unknown-linux-gnu` on ubuntu-latest and
# `x86_64-apple-darwin` on macos-latest). The native-arch legs pass. This is the
# SAME drift class fixed for publish-snapshot.yml in #935/#948, which introduced
# scripts/check-toolchain-refs.sh. That guard temporarily ALLOWLISTED
# release.yml as a tracked follow-up — this issue, #939.
#
# Contract:
#   1. The fix: release.yml pins the cross-build toolchain to the exact channel
#      in rust-toolchain.toml (@1.97.0) and keeps `targets: ${{ matrix.target }}`.
#   2. Full enforcement: scripts/check-toolchain-refs.sh no longer allowlists
#      release.yml. The ALLOWLIST is empty, so release.yml is fully enforced and
#      a regression back to @stable fails the guard (never a silent WARNING).
#   3. Invariant unchanged: every `dtolnay/rust-toolchain@<ref>` step that also
#      declares a `targets:` key MUST pin <ref> to the rust-toolchain.toml
#      channel; scan/native-only steps (no `targets:`) may stay on `@stable`.
#   4. Docs: docs/reference/ci-pipeline.md documents release.yml as resolved with
#      no remaining tracked/known drift.
#   5. Bugfix hygiene: this change does NOT bump the workspace version.
#
# Expected BEFORE the fix: FAIL (release.yml @stable, allowlisted, docs note).
# Expected AFTER the fix:  PASS (all cases).
#
# Run: bash tests/issue_939_release_toolchain_pin_test.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKFLOWS="$REPO_ROOT/.github/workflows"
TOOLCHAIN_TOML="$REPO_ROOT/rust-toolchain.toml"
RELEASE="$WORKFLOWS/release.yml"
CI="$WORKFLOWS/ci.yml"
GUARD="$REPO_ROOT/scripts/check-toolchain-refs.sh"
DOCS="$REPO_ROOT/docs/reference/ci-pipeline.md"

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

echo "=== issue #939 Auto Release toolchain-ref pin TDD tests ==="
echo "Workflows: $WORKFLOWS"
echo "Toolchain: $TOOLCHAIN_TOML"
echo "Guard:     $GUARD"
echo "Docs:      $DOCS"
echo

CHANNEL="$(toolchain_channel)"
assert "rust-toolchain.toml declares a channel" "[ -n '$CHANNEL' ]"
echo "Resolved channel: ${CHANNEL:-<none>}"
echo

# ---------------------------------------------------------------------------
# 1. The fix: release.yml cross-build step is pinned to the toml channel.
# ---------------------------------------------------------------------------
assert "release.yml exists" "[ -f '$RELEASE' ]"

assert "release.yml pins dtolnay/rust-toolchain to the toml channel ($CHANNEL)" \
    "grep -q 'dtolnay/rust-toolchain@$CHANNEL' '$RELEASE'"

assert "release.yml no longer uses dtolnay/rust-toolchain@stable" \
    "! grep -q 'dtolnay/rust-toolchain@stable' '$RELEASE'"

assert "release.yml preserves 'targets: \${{ matrix.target }}'" \
    "grep -Eq 'targets:[[:space:]]*\\\$\\{\\{[[:space:]]*matrix.target[[:space:]]*\\}\\}' '$RELEASE'"

# Every targets-bearing ref in the release workflow must equal the channel.
rel_bad=""
while IFS= read -r ref; do
    [ -n "$ref" ] || continue
    [ "$ref" = "$CHANNEL" ] || rel_bad="$rel_bad $ref"
done < <(targets_bearing_refs "$RELEASE")
assert "release.yml has no drifted targets-bearing refs" "[ -z '$rel_bad' ]"

# The cross-arch legs that triggered #939 must be present in the matrix so the
# single pinned toolchain step provisions their rust-std.
assert "release.yml matrix includes the aarch64-unknown-linux-gnu cross leg" \
    "grep -q 'aarch64-unknown-linux-gnu' '$RELEASE'"
assert "release.yml matrix includes the x86_64-apple-darwin cross leg" \
    "grep -q 'x86_64-apple-darwin' '$RELEASE'"

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
# 3. Full enforcement: the guard no longer allowlists release.yml.
# ---------------------------------------------------------------------------
assert "guard script exists" "[ -f '$GUARD' ]"
assert "guard script is executable" "[ -x '$GUARD' ]"

# The guard must no longer carry release.yml as an allowlisted (exempt) entry;
# an empty allowlist is what makes the invariant self-enforcing.
assert "guard no longer allowlists release.yml" \
    "! grep -Eq 'ALLOWLIST=\\([^)]*release\\.yml' '$GUARD'"

if [ -x "$GUARD" ]; then
    TMPROOT="$(mktemp -d)"
    fx="$TMPROOT/fixtures"
    mkdir -p "$fx"
    cp "$TOOLCHAIN_TOML" "$TMPROOT/rust-toolchain.toml"

    # Fixture A: a release-like file drifted back to @stable WITH targets.
    # With the allowlist empty this MUST fail the guard (hard error, not warn).
    cat > "$fx/release.yml" <<EOF
jobs:
  build:
    steps:
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: \${{ matrix.target }}
EOF

    # Fixture B: correct — release-like file pinned to channel WITH targets.
    cat > "$fx/release-pinned.yml" <<EOF
jobs:
  build:
    steps:
      - uses: dtolnay/rust-toolchain@$CHANNEL
        with:
          targets: \${{ matrix.target }}
EOF

    run_guard() {
        TOOLCHAIN_TOML="$TMPROOT/rust-toolchain.toml" \
            bash "$GUARD" "$@" >/dev/null 2>&1
    }

    # Regression guard: a drifted release.yml is now a FAILURE, not an
    # allowlisted warning.
    if run_guard "$fx/release.yml"; then
        record_fail "guard fails on a drifted release.yml (no longer allowlisted)" \
            "expected non-zero exit now that ALLOWLIST is empty"
    else
        record_pass "guard fails on a drifted release.yml (no longer allowlisted)"
    fi

    if run_guard "$fx/release-pinned.yml"; then
        record_pass "guard accepts channel-pinned release.yml + targets"
    else
        record_fail "guard accepts channel-pinned release.yml + targets" "expected zero exit"
    fi

    # The real, fixed release workflow must pass the guard.
    if bash "$GUARD" "$RELEASE" >/dev/null 2>&1; then
        record_pass "guard passes on the fixed release.yml"
    else
        record_fail "guard passes on the fixed release.yml" "expected zero exit"
    fi

    # Default scan (no args) must be green with zero allowlisted drift, so a
    # WARNING line for a tracked follow-up must NOT appear.
    scan_out="$(TOOLCHAIN_TOML="$TMPROOT/rust-toolchain.toml"; bash "$GUARD" 2>&1)"
    scan_rc=$?
    if [ "$scan_rc" -eq 0 ]; then
        record_pass "guard default scan is green (exit 0, CI-wireable)"
    else
        record_fail "guard default scan is green (exit 0, CI-wireable)" \
            "unresolved toolchain-ref drift in .github/workflows"
    fi
    if printf '%s\n' "$scan_out" | grep -qi 'allowlisted'; then
        record_fail "guard default scan reports zero allowlisted drift" \
            "found an allowlisted (tracked follow-up) warning; allowlist should be empty"
    else
        record_pass "guard default scan reports zero allowlisted drift"
    fi
fi

# ---------------------------------------------------------------------------
# 4. Docs: release.yml drift is documented as resolved (no remaining note).
# ---------------------------------------------------------------------------
assert "ci-pipeline.md exists" "[ -f '$DOCS' ]"
assert "ci-pipeline.md has no 'Known remaining drift' note" \
    "! grep -qi 'remaining drift' '$DOCS'"
assert "ci-pipeline.md documents release.yml pinned to the channel" \
    "grep -q 'dtolnay/rust-toolchain@$CHANNEL' '$DOCS'"

# ---------------------------------------------------------------------------
# 5. Bugfix hygiene: no workspace version bump on this change.
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
