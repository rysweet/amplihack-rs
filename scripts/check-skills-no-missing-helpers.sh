#!/usr/bin/env bash
# Fail if live bundled skills reference legacy Python helper scripts that are
# not shipped in amplihack-rs.

set -euo pipefail

patterns=(
    '\.claude/(scenarios|skills)/[A-Za-z0-9_./-]+\.py'
    'scripts/[A-Za-z0-9_./-]+\.py'
    'common/verification/verify_skill\.py'
    'ooxml/scripts/[A-Za-z0-9_./-]+\.py'
    'run_benchmarks\.py'
    'visualizer\.py'
    'recalc\.py'
    'detect_language\.py'
    'generate_dap_config\.py'
    'test_mcp_integration\.py'
    'check_drift\.py'
    'check-freshness\.py'
    'coordinate\.py'
    'create_delegation\.py'
    'analyze_backlog\.py'
    'generate_top5\.py'
    'generate_daily_status\.py'
    'generate_roadmap_review\.py'
    'manage_state\.py'
    'verify_skill\.py'
    'unpack\.py'
    'pack\.py'
    'thumbnail\.py'
    'rearrange\.py'
    'inventory\.py'
    'replace\.py'
    'validate\.py'
)

regex=$(IFS='|'; echo "${patterns[*]}")

violations="$(rg -n "$regex" amplifier-bundle/skills || true)"

if [[ -n "$violations" ]]; then
    cat >&2 <<EOF
FAIL: live bundled skills reference missing legacy Python helper scripts.

Skills may include Python language examples, but they must not instruct users to
run helper scripts that amplihack-rs does not ship. Use native helpers, shell/JS
helpers that exist in the bundle, Rust CLI commands, or explicit manual steps.

Violations:
$violations
EOF
    exit 1
fi

echo "PASS: live bundled skills do not reference missing legacy Python helpers."
