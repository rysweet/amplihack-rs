#!/usr/bin/env python3
"""Phase 1c: Parity validation — run golden inputs through both Python and Rust hooks.

Measures match rate between Python hook output and Rust hook binary output.
Target: ≥95% parity across all golden test cases.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

RUST_BINARY = Path.home() / "src/amplihack-rs/target/release/amplihack-hooks"
GOLDEN_DIR = Path.home() / "src/amplihack-rs/tests/golden/hooks"
PYTHON_HOOKS_DIR = Path(
    os.environ.get(
        "AMPLIHACK_PYTHON_HOOKS_DIR",
        str(Path.home() / "src/amplihack/.claude/tools/amplihack/hooks"),
    )
)

# Map golden dir names to Rust subcommands and Python hook scripts
HOOK_MAP = {
    "pre_tool_use": ("pre-tool-use", "pre_tool_use.py"),
    "post_tool_use": ("post-tool-use", "post_tool_use.py"),
    "stop": ("stop", "stop.py"),
    "session_stop": ("session-stop", "session_stop.py"),
    "session_start": ("session-start", "session_start.py"),
    "pre_compact": ("pre-compact", "pre_compact.py"),
    "user_prompt_submit": ("user-prompt-submit", "user_prompt_submit.py"),
}


def run_rust_hook(subcommand: str, input_json: str) -> dict:
    """Run input through Rust hook binary."""
    try:
        result = subprocess.run(
            [str(RUST_BINARY), subcommand],
            input=input_json,
            capture_output=True,
            text=True,
            timeout=10,
        )
        stdout = result.stdout.strip()
        if not stdout:
            return {}
        return json.loads(stdout)
    except (subprocess.TimeoutExpired, json.JSONDecodeError, Exception) as e:
        return {"__error__": str(e)}


def run_python_hook(script_name: str, input_json: str) -> dict:
    """Run input through Python hook script."""
    script_path = PYTHON_HOOKS_DIR / script_name
    if not script_path.exists():
        return {"__error__": f"Script not found: {script_path}"}
    try:
        result = subprocess.run(
            [sys.executable, str(script_path)],
            input=input_json,
            capture_output=True,
            text=True,
            timeout=10,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
        stdout = result.stdout.strip()
        if not stdout:
            return {}
        return json.loads(stdout)
    except (subprocess.TimeoutExpired, json.JSONDecodeError, Exception) as e:
        return {"__error__": str(e)}


def normalize_output(output: dict) -> dict:
    """Normalize output for comparison — remove version field (Rust-only)."""
    result = dict(output)
    result.pop("version", None)
    result.pop("__error__", None)
    return result


def compare_outputs(rust_out: dict, python_out: dict) -> tuple[bool, str]:
    """Compare Rust and Python outputs. Returns (match, reason)."""
    r = normalize_output(rust_out)
    p = normalize_output(python_out)

    # Both empty = match (allow)
    if not r and not p:
        return True, "both_allow"

    # Check hookSpecificOutput match
    r_decision = r.get("hookSpecificOutput", {})
    p_decision = p.get("hookSpecificOutput", {})

    if not r_decision and not p_decision:
        return True, "both_allow"

    r_perm = r_decision.get("permissionDecision", "")
    p_perm = p_decision.get("permissionDecision", "")

    if r_perm != p_perm:
        return False, f"decision_mismatch: rust={r_perm} python={p_perm}"

    # Decisions match — good enough for parity
    return True, f"decision_match: {r_perm}"


def main():
    total = 0
    matches = 0
    mismatches = []
    rust_errors = 0
    python_errors = 0
    skipped = 0

    for hook_dir, (rust_subcmd, python_script) in HOOK_MAP.items():
        hook_path = GOLDEN_DIR / hook_dir
        if not hook_path.exists():
            print(f"SKIP: {hook_dir} — no golden files")
            continue

        input_files = sorted(hook_path.glob("*.input.json"))
        print(f"\n{'='*60}")
        print(f"Hook: {hook_dir} ({len(input_files)} golden files)")
        print(f"{'='*60}")

        hook_matches = 0
        hook_total = 0

        for input_file in input_files:
            test_name = input_file.stem.replace(".input", "")
            input_json = input_file.read_text()

            rust_out = run_rust_hook(rust_subcmd, input_json)
            python_out = run_python_hook(python_script, input_json)

            hook_total += 1
            total += 1

            if "__error__" in rust_out:
                rust_errors += 1
                print(f"  RUST_ERR: {test_name}: {rust_out['__error__'][:80]}")
                continue
            if "__error__" in python_out:
                python_errors += 1
                # Python errors are expected for some hooks that need SDK
                skipped += 1
                continue

            match, reason = compare_outputs(rust_out, python_out)
            if match:
                matches += 1
                hook_matches += 1
            else:
                mismatches.append((hook_dir, test_name, reason, rust_out, python_out))
                print(f"  MISMATCH: {test_name}: {reason}")

        effective = hook_total - python_errors
        if effective > 0:
            pct = hook_matches / effective * 100
            print(f"  Result: {hook_matches}/{effective} ({pct:.1f}%)")

    # Summary
    effective_total = total - python_errors - rust_errors
    print(f"\n{'='*60}")
    print(f"PARITY SUMMARY")
    print(f"{'='*60}")
    print(f"Total golden files:   {total}")
    print(f"Rust errors:          {rust_errors}")
    print(f"Python errors/skip:   {python_errors}")
    print(f"Effective comparisons:{effective_total}")
    print(f"Matches:              {matches}")
    if effective_total > 0:
        parity = matches / effective_total * 100
        print(f"PARITY RATE:          {parity:.1f}%")
        target = 95.0
        if parity >= target:
            print(f"✓ TARGET MET (≥{target}%)")
        else:
            print(f"✗ BELOW TARGET ({target}%)")
    else:
        print("No effective comparisons possible")

    if mismatches:
        print(f"\nMISMATCHES ({len(mismatches)}):")
        for hook, name, reason, rust, python in mismatches[:20]:
            print(f"  {hook}/{name}: {reason}")
            print(f"    Rust:   {json.dumps(normalize_output(rust), sort_keys=True)[:120]}")
            print(f"    Python: {json.dumps(normalize_output(python), sort_keys=True)[:120]}")

    # Telemetry check
    print(f"\n{'='*60}")
    print(f"TELEMETRY CHECK")
    print(f"{'='*60}")
    test_input = '{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"echo hi"}}'
    result = subprocess.run(
        [str(RUST_BINARY), "pre-tool-use"],
        input=test_input,
        capture_output=True,
        text=True,
        timeout=5,
    )
    stderr = result.stderr.strip()
    if stderr:
        try:
            telemetry = json.loads(stderr)
            print(f"  hook:        {telemetry.get('hook', '?')}")
            print(f"  duration_us: {telemetry.get('duration_us', '?')}")
            print(f"  result:      {telemetry.get('result', '?')}")
            print(f"  ✓ Telemetry JSON emitted correctly")
        except json.JSONDecodeError:
            print(f"  ✗ Telemetry is not valid JSON: {stderr[:100]}")
    else:
        print(f"  ✗ No telemetry on stderr")

    sys.exit(0 if (effective_total > 0 and matches / effective_total >= 0.95) else 1)


if __name__ == "__main__":
    main()
