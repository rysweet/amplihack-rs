#!/usr/bin/env bash
# Fail if live bundled skills or Claude command files reference legacy Python
# helper scripts that are not shipped in amplihack-rs.

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
command_regex="$regex|\.claude/[A-Za-z0-9_./-]+\.py"

skill_violations="$(rg -n "$regex" amplifier-bundle/skills || true)"
command_violations="$(rg -n "$command_regex" docs/claude/commands || true)"

if [[ -n "$skill_violations" || -n "$command_violations" ]]; then
    cat >&2 <<EOF
FAIL: live bundled skills or Claude command files reference missing legacy Python helper scripts.

Python language examples are allowed. Instructions that run helpers must use
native helpers, shell/JS helpers that exist in the bundle, Rust CLI commands, or
explicit manual steps.

EOF
    if [[ -n "$skill_violations" ]]; then
        printf '%s\n' 'Violations in amplifier-bundle/skills:' >&2
        printf '%s\n' "$skill_violations" >&2
    fi
    if [[ -n "$command_violations" ]]; then
        printf '%s\n' 'Violations in docs/claude/commands:' >&2
        printf '%s\n' "$command_violations" >&2
    fi
    exit 1
fi

echo "PASS: live bundled skills and Claude command files are clear of missing legacy Python helper paths."
