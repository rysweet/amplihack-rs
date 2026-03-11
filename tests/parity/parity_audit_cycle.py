#!/usr/bin/env python3
"""Parity audit cycle: identify → validate → fix → re-validate.

Self-improvement harness that:
1. Runs all parity tests across all tiers
2. Collects divergences and categorizes them
3. Generates a gap report with fix recommendations
4. Outputs structured data for fix workstreams
5. Re-validates after fixes are applied

Designed to be run repeatedly until 100% parity is achieved.

Usage:
    # Full audit cycle
    python tests/parity/parity_audit_cycle.py

    # Validate-only (no fix recommendations)
    python tests/parity/parity_audit_cycle.py --validate-only

    # Run specific tiers
    python tests/parity/parity_audit_cycle.py --tiers tier1 tier5-gap-tests

    # Generate fix workstream specs
    python tests/parity/parity_audit_cycle.py --generate-fix-specs

Master issue: rysweet/amplihack-rs#25
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parents[2]
PARITY_DIR = REPO_ROOT / "tests" / "parity"
SCENARIOS_DIR = PARITY_DIR / "scenarios"
HARNESS_SCRIPT = PARITY_DIR / "validate_cli_parity.py"
HOOK_PARITY_SCRIPT = PARITY_DIR / "validate_parity.py"
SHADOW_HARNESS_SCRIPT = PARITY_DIR / "shadow_harness.py"
AUDIT_LOG_DIR = Path("/tmp/amplihack-parity-audit")
PYTHON_REPO = Path(os.environ.get("AMPLIHACK_PYTHON_REPO", str(Path.home() / "src" / "amploxy")))
RUST_BINARY = REPO_ROOT / "target" / "debug" / "amplihack"


@dataclass
class TierResult:
    tier_name: str
    total: int
    passed: int
    failed: int
    failures: list[dict[str, Any]]
    duration_ms: int


@dataclass
class AuditResult:
    """Complete audit cycle result."""
    cycle_id: str
    timestamp: str
    tiers: list[TierResult]
    shadow_result: dict[str, Any] | None
    total_cases: int
    total_passed: int
    total_failed: int
    parity_rate: str
    gap_categories: dict[str, list[str]]
    fix_specs: list[dict[str, Any]]


# ---------------------------------------------------------------------------
# Tier runner
# ---------------------------------------------------------------------------

def run_tier(tier_file: Path, timeout: int = 120) -> TierResult:
    """Run a single tier of parity tests."""
    tier_name = tier_file.stem
    start = time.monotonic()

    report_file = AUDIT_LOG_DIR / f"{tier_name}-report.json"
    cmd = [
        sys.executable, str(HARNESS_SCRIPT),
        "--scenario", str(tier_file),
        "--python-repo", str(PYTHON_REPO),
        "--rust-binary", str(RUST_BINARY),
        "--report", str(report_file),
        "--shadow-mode",
        "--shadow-log", str(AUDIT_LOG_DIR / f"{tier_name}-shadow.jsonl"),
    ]

    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        elapsed = int((time.monotonic() - start) * 1000)
        return TierResult(
            tier_name=tier_name,
            total=0, passed=0, failed=0,
            failures=[{"error": f"TIMEOUT after {timeout}s"}],
            duration_ms=elapsed,
        )

    elapsed = int((time.monotonic() - start) * 1000)

    # Parse report
    if report_file.exists():
        report = json.loads(report_file.read_text())
    else:
        report = []

    total = len(report)
    passed = sum(1 for r in report if r.get("match", False))
    failed = total - passed
    failures = [
        {"name": r["name"], "details": r.get("details", {})}
        for r in report if not r.get("match", False)
    ]

    # Also capture output parsing failures
    if result.returncode != 0 and total == 0:
        failures.append({
            "error": "harness_error",
            "returncode": result.returncode,
            "stderr": result.stderr[:500],
        })

    return TierResult(
        tier_name=tier_name,
        total=total,
        passed=passed,
        failed=failed,
        failures=failures,
        duration_ms=elapsed,
    )


def run_shadow_harness(timeout: int = 120) -> dict[str, Any] | None:
    """Run the shadow harness for additional coverage."""
    if not SHADOW_HARNESS_SCRIPT.exists():
        return None

    try:
        result = subprocess.run(
            [sys.executable, str(SHADOW_HARNESS_SCRIPT),
             "--python-repo", str(PYTHON_REPO),
             "--rust-binary", str(RUST_BINARY),
             "--log-dir", str(AUDIT_LOG_DIR / "shadow"),
             "--only-builtin"],
            capture_output=True, text=True, timeout=timeout,
        )
        summary_file = AUDIT_LOG_DIR / "shadow" / "summary.json"
        if summary_file.exists():
            return json.loads(summary_file.read_text())
    except subprocess.TimeoutExpired:
        return {"error": "TIMEOUT"}
    except Exception as e:
        return {"error": str(e)}
    return None


# ---------------------------------------------------------------------------
# Gap categorization
# ---------------------------------------------------------------------------

def categorize_gaps(tiers: list[TierResult]) -> dict[str, list[str]]:
    """Categorize failures into fix workstream categories."""
    categories: dict[str, list[str]] = {
        "install": [],
        "launch": [],
        "recipe": [],
        "validation": [],
        "environment": [],
        "hooks": [],
        "settings": [],
        "e2e": [],
        "other": [],
    }

    for tier in tiers:
        for failure in tier.failures:
            name = failure.get("name", failure.get("error", "unknown"))

            if "install" in name or "uninstall" in name:
                categories["install"].append(f"{tier.tier_name}/{name}")
            elif "launch" in name or "launcher" in name or "sigint" in name:
                categories["launch"].append(f"{tier.tier_name}/{name}")
            elif "recipe" in name or "live-recipe" in name:
                categories["recipe"].append(f"{tier.tier_name}/{name}")
            elif "validate" in name or "malformed" in name or "warn" in name:
                categories["validation"].append(f"{tier.tier_name}/{name}")
            elif "env" in name or "runtime" in name or "session" in name:
                categories["environment"].append(f"{tier.tier_name}/{name}")
            elif "hook" in name:
                categories["hooks"].append(f"{tier.tier_name}/{name}")
            elif "settings" in name or "config" in name:
                categories["settings"].append(f"{tier.tier_name}/{name}")
            elif "e2e" in name:
                categories["e2e"].append(f"{tier.tier_name}/{name}")
            else:
                categories["other"].append(f"{tier.tier_name}/{name}")

    # Remove empty categories
    return {k: v for k, v in categories.items() if v}


# ---------------------------------------------------------------------------
# Fix spec generation
# ---------------------------------------------------------------------------

def generate_fix_specs(categories: dict[str, list[str]], tiers: list[TierResult]) -> list[dict[str, Any]]:
    """Generate fix workstream specifications for each gap category."""
    specs = []

    for category, cases in sorted(categories.items()):
        # Collect details for all failures in this category
        details = []
        for tier in tiers:
            for failure in tier.failures:
                name = failure.get("name", "")
                full_name = f"{tier.tier_name}/{name}"
                if full_name in cases:
                    details.append(failure)

        spec = {
            "workstream_id": f"fix-{category}",
            "category": category,
            "failing_cases": cases,
            "case_count": len(cases),
            "priority": _priority(category, len(cases)),
            "description": _describe_fix(category, cases, details),
            "rust_files_likely_affected": _guess_files(category),
            "validation_command": f"python {HARNESS_SCRIPT} --scenario {SCENARIOS_DIR}/tier5-gap-tests.yaml --case " + " --case ".join(
                c.split("/", 1)[1] for c in cases[:5] if "/" in c
            ),
        }
        specs.append(spec)

    # Sort by priority
    priority_order = {"critical": 0, "high": 1, "medium": 2, "low": 3}
    specs.sort(key=lambda s: priority_order.get(s["priority"], 99))
    return specs


def _priority(category: str, count: int) -> str:
    if category in ("install", "launch") and count >= 3:
        return "critical"
    if category in ("install", "launch"):
        return "high"
    if category in ("recipe", "validation"):
        return "medium"
    return "low"


def _describe_fix(category: str, cases: list[str], details: list[dict]) -> str:
    descriptions = {
        "install": "Fix install/uninstall parity: exit codes, manifest handling, settings.json generation",
        "launch": "Add missing launch flags (--dangerously-skip-permissions, --model), fix SIGINT exit code, add env vars",
        "recipe": "Fix live recipe execution parity: variable passing, condition evaluation, error handling",
        "validation": "Add YAML field typo detection (Python warns about unrecognized fields, Rust silently absorbs)",
        "environment": "Align AMPLIHACK_* environment variables between Python and Rust",
        "hooks": "Align hook engine defaults and registration format",
        "settings": "Align settings.json creation behavior (rich template vs empty default)",
        "e2e": "Fix end-to-end workflow parity",
    }
    return descriptions.get(category, f"Fix {category} parity ({len(cases)} cases)")


def _guess_files(category: str) -> list[str]:
    file_map = {
        "install": [
            "crates/amplihack-cli/src/commands/install.rs",
            "crates/amplihack-cli/src/settings_manager.rs",
        ],
        "launch": [
            "crates/amplihack-cli/src/commands/launch.rs",
            "crates/amplihack-cli/src/env_builder.rs",
        ],
        "recipe": [
            "crates/amplihack-cli/src/commands/recipe/run.rs",
            "crates/amplihack-cli/src/commands/recipe/mod.rs",
        ],
        "validation": [
            "crates/amplihack-cli/src/commands/recipe/mod.rs",
            "crates/amplihack-cli/src/commands/recipe/show_validate.rs",
        ],
        "environment": [
            "crates/amplihack-cli/src/env_builder.rs",
        ],
        "hooks": [
            "crates/amplihack-hooks/src/lib.rs",
            "crates/amplihack-cli/src/commands/install.rs",
        ],
        "settings": [
            "crates/amplihack-cli/src/settings_manager.rs",
        ],
    }
    return file_map.get(category, [])


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

def print_audit_report(audit: AuditResult):
    """Print human-readable audit report."""
    print(f"\n{'=' * 70}")
    print(f"PARITY AUDIT CYCLE: {audit.cycle_id}")
    print(f"{'=' * 70}")
    print(f"Timestamp: {audit.timestamp}")
    print(f"Total:     {audit.total_cases}")
    print(f"Passed:    {audit.total_passed}")
    print(f"Failed:    {audit.total_failed}")
    print(f"Parity:    {audit.parity_rate}")

    print(f"\n--- Tier Results ---")
    for tier in audit.tiers:
        status = "PASS" if tier.failed == 0 else "FAIL"
        print(f"  {tier.tier_name}: {tier.passed}/{tier.total} ({status}) [{tier.duration_ms}ms]")
        for f in tier.failures[:5]:
            name = f.get("name", f.get("error", "?"))
            print(f"    ✗ {name}")
        if len(tier.failures) > 5:
            print(f"    ... and {len(tier.failures) - 5} more")

    if audit.shadow_result:
        print(f"\n--- Shadow Harness ---")
        sr = audit.shadow_result
        if "error" in sr:
            print(f"  Error: {sr['error']}")
        else:
            print(f"  Total: {sr.get('total', '?')}, Matched: {sr.get('matched', '?')}, Diverged: {sr.get('diverged', '?')}")

    if audit.gap_categories:
        print(f"\n--- Gap Categories ---")
        for cat, cases in sorted(audit.gap_categories.items()):
            print(f"  {cat}: {len(cases)} failures")

    if audit.fix_specs:
        print(f"\n--- Fix Workstream Specs ---")
        for spec in audit.fix_specs:
            print(f"\n  [{spec['priority'].upper()}] {spec['workstream_id']}")
            print(f"    Cases: {spec['case_count']}")
            print(f"    Description: {spec['description']}")
            if spec['rust_files_likely_affected']:
                print(f"    Files: {', '.join(spec['rust_files_likely_affected'])}")

    print(f"\n{'=' * 70}")
    if audit.total_failed == 0:
        print("ALL TESTS PASS — 100% PARITY ACHIEVED")
    else:
        print(f"GAPS REMAIN: {audit.total_failed} failures across {len(audit.gap_categories)} categories")
    print(f"{'=' * 70}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Parity audit cycle")
    parser.add_argument(
        "--tiers",
        nargs="*",
        help="Specific tier names to run (default: all)",
    )
    parser.add_argument(
        "--validate-only",
        action="store_true",
        help="Only validate, don't generate fix specs",
    )
    parser.add_argument(
        "--generate-fix-specs",
        action="store_true",
        help="Generate fix workstream specifications",
    )
    parser.add_argument(
        "--skip-shadow",
        action="store_true",
        help="Skip shadow harness (faster)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Write full audit result to this JSON file",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=120,
        help="Timeout per tier in seconds",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    AUDIT_LOG_DIR.mkdir(parents=True, exist_ok=True)

    cycle_id = f"audit-{int(time.time())}"

    # Discover tiers
    if args.tiers:
        tier_files = []
        for name in args.tiers:
            candidate = SCENARIOS_DIR / f"{name}.yaml"
            if candidate.exists():
                tier_files.append(candidate)
            else:
                print(f"WARNING: tier {name} not found at {candidate}", file=sys.stderr)
    else:
        tier_files = sorted(SCENARIOS_DIR.glob("*.yaml"))

    print(f"Parity Audit Cycle: {cycle_id}")
    print(f"Tiers: {len(tier_files)}")
    print(f"Python repo: {PYTHON_REPO}")
    print(f"Rust binary: {RUST_BINARY}")

    if not RUST_BINARY.exists():
        print(f"ERROR: Rust binary not found: {RUST_BINARY}", file=sys.stderr)
        print("Run: cd ~/src/amplihack-rs && cargo build", file=sys.stderr)
        return 1

    # Run tiers
    tiers: list[TierResult] = []
    for tier_file in tier_files:
        print(f"\n--- Running {tier_file.stem} ---")
        result = run_tier(tier_file, timeout=args.timeout)
        tiers.append(result)
        status = "PASS" if result.failed == 0 else f"FAIL ({result.failed} failures)"
        print(f"  {result.passed}/{result.total} passed — {status}")

    # Shadow harness
    shadow_result = None
    if not args.skip_shadow:
        print(f"\n--- Running shadow harness ---")
        shadow_result = run_shadow_harness(timeout=args.timeout)

    # Categorize and generate specs
    categories = categorize_gaps(tiers)
    fix_specs = []
    if not args.validate_only:
        fix_specs = generate_fix_specs(categories, tiers)

    # Build audit result
    total_cases = sum(t.total for t in tiers)
    total_passed = sum(t.passed for t in tiers)
    total_failed = sum(t.failed for t in tiers)

    audit = AuditResult(
        cycle_id=cycle_id,
        timestamp=datetime.now(timezone.utc).isoformat(),
        tiers=tiers,
        shadow_result=shadow_result,
        total_cases=total_cases,
        total_passed=total_passed,
        total_failed=total_failed,
        parity_rate=f"{total_passed / total_cases * 100:.1f}%" if total_cases > 0 else "N/A",
        gap_categories=categories,
        fix_specs=fix_specs,
    )

    print_audit_report(audit)

    # Write output
    output_path = args.output or (AUDIT_LOG_DIR / f"{cycle_id}.json")
    output_data = {
        "cycle_id": audit.cycle_id,
        "timestamp": audit.timestamp,
        "total_cases": audit.total_cases,
        "total_passed": audit.total_passed,
        "total_failed": audit.total_failed,
        "parity_rate": audit.parity_rate,
        "tiers": [
            {
                "name": t.tier_name,
                "total": t.total,
                "passed": t.passed,
                "failed": t.failed,
                "failures": t.failures,
                "duration_ms": t.duration_ms,
            }
            for t in audit.tiers
        ],
        "shadow_result": audit.shadow_result,
        "gap_categories": audit.gap_categories,
        "fix_specs": audit.fix_specs,
    }
    output_path.write_text(json.dumps(output_data, indent=2), encoding="utf-8")
    print(f"\nAudit result written to: {output_path}")

    return 0 if total_failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
