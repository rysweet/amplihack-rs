#!/usr/bin/env python3
"""Shadow-mode parity harness.

Runs Python and Rust implementations side-by-side for real workloads,
logging all divergences without failing. Designed for continuous validation.

Key design constraints:
  - No collision between shadow runs on the same host (isolated sandboxes)
  - No collision on GitHub (shadow branches use unique prefixes)
  - Atomic divergence logging (append-only JSONL)
  - Can run alongside normal development without interference

Master issue: rysweet/amplihack-rs#25
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

SHADOW_PREFIX = "shadow-parity"
DEFAULT_LOG_DIR = Path("/tmp/amplihack-shadow-parity")
DEFAULT_DIVERGENCE_LOG = DEFAULT_LOG_DIR / "divergences.jsonl"
DEFAULT_SUMMARY_LOG = DEFAULT_LOG_DIR / "summary.json"


@dataclass
class ShadowConfig:
    python_repo: Path = Path.home() / "src" / "amploxy"
    python_exe: Path | None = None
    rust_binary: Path = Path.home() / "src" / "amplihack-rs" / "target" / "debug" / "amplihack"
    log_dir: Path = DEFAULT_LOG_DIR
    divergence_log: Path = DEFAULT_DIVERGENCE_LOG
    keep_sandboxes: bool = False
    run_id: str = field(default_factory=lambda: f"{SHADOW_PREFIX}-{int(time.time())}")


@dataclass
class ShadowResult:
    """Result of a single shadow comparison."""
    case_name: str
    category: str
    match: bool
    python_exit: int
    rust_exit: int
    python_stdout: str
    rust_stdout: str
    python_stderr: str
    rust_stderr: str
    divergences: list[dict[str, Any]]
    duration_ms: int
    timestamp: str
    sandbox_python: str = ""
    sandbox_rust: str = ""


# ---------------------------------------------------------------------------
# Shadow sandbox isolation
# ---------------------------------------------------------------------------

def create_isolated_sandbox(run_id: str, engine: str, case_name: str) -> Path:
    """Create an isolated sandbox that won't collide with other shadow runs."""
    safe_name = "".join(c if c.isalnum() or c in "-_" else "-" for c in case_name)[:40]
    sandbox = Path(tempfile.mkdtemp(
        prefix=f"{run_id}-{engine}-{safe_name}-",
        dir="/tmp",
    ))
    (sandbox / "home").mkdir()
    (sandbox / "tmp").mkdir()
    return sandbox


def build_sandbox_env(
    sandbox: Path,
    python_repo: Path,
    extra_env: dict[str, str] | None = None,
) -> dict[str, str]:
    """Build isolated environment for shadow run."""
    env = os.environ.copy()
    home = sandbox / "home"
    env["HOME"] = str(home)
    env["TMPDIR"] = str(sandbox / "tmp")
    env["TMP"] = str(sandbox / "tmp")
    env["TEMP"] = str(sandbox / "tmp")
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    env["PYTHONPATH"] = str(python_repo / "src")
    # Prevent shadow runs from touching real git state
    env["GIT_AUTHOR_NAME"] = "shadow-parity"
    env["GIT_AUTHOR_EMAIL"] = "shadow@parity.test"
    env["GIT_COMMITTER_NAME"] = "shadow-parity"
    env["GIT_COMMITTER_EMAIL"] = "shadow@parity.test"
    # Unique shadow identifier to prevent collisions
    env["AMPLIHACK_SHADOW_RUN"] = "1"
    env["AMPLIHACK_SHADOW_SANDBOX"] = str(sandbox)
    if extra_env:
        for k, v in extra_env.items():
            rendered = v.replace("${SANDBOX_ROOT}", str(sandbox))
            rendered = rendered.replace("${HOME}", str(home))
            env[k] = rendered
    return env


# ---------------------------------------------------------------------------
# Shadow execution
# ---------------------------------------------------------------------------

def run_shadow_case(
    config: ShadowConfig,
    case: dict[str, Any],
) -> ShadowResult:
    """Run a single test case through both Python and Rust, compare results."""
    case_name = case["name"]
    category = case.get("category", "unknown")
    start = time.monotonic()

    # Create isolated sandboxes
    py_sandbox = create_isolated_sandbox(config.run_id, "python", case_name)
    rs_sandbox = create_isolated_sandbox(config.run_id, "rust", case_name)

    try:
        argv = case["argv"]
        stdin_data = case.get("stdin", "")
        timeout = int(case.get("timeout", 30))
        extra_env = case.get("env", {})

        # Build envs
        py_env = build_sandbox_env(py_sandbox, config.python_repo, extra_env)
        rs_env = build_sandbox_env(rs_sandbox, config.python_repo, extra_env)

        # Run setup in both sandboxes
        setup_script = case.get("setup", "")
        if setup_script:
            for sandbox, env in [(py_sandbox, py_env), (rs_sandbox, rs_env)]:
                env_copy = dict(env)
                env_copy["SANDBOX_ROOT"] = str(sandbox)
                subprocess.run(
                    ["bash", "-lc", setup_script],
                    cwd=sandbox,
                    env=env_copy,
                    text=True,
                    capture_output=True,
                    timeout=30,
                )

        # Resolve Python exe
        python_exe = config.python_exe
        if python_exe is None:
            candidate = config.python_repo / ".venv" / "bin" / "python"
            python_exe = candidate if candidate.exists() else Path(sys.executable)

        # Run Python
        py_cmd = [str(python_exe), "-m", "amplihack", *argv]
        py_env["SANDBOX_ROOT"] = str(py_sandbox)
        cwd = py_sandbox / case.get("cwd", ".")
        cwd.mkdir(parents=True, exist_ok=True)
        try:
            py_result = subprocess.run(
                py_cmd, cwd=cwd, env=py_env, input=stdin_data,
                text=True, capture_output=True, timeout=timeout,
            )
            py_exit, py_stdout, py_stderr = py_result.returncode, py_result.stdout, py_result.stderr
        except subprocess.TimeoutExpired:
            py_exit, py_stdout, py_stderr = 124, "", "TIMEOUT"
        except Exception as e:
            py_exit, py_stdout, py_stderr = 1, "", f"ERROR: {e}"

        # Run Rust
        rs_cmd = [str(config.rust_binary), *argv]
        rs_env["SANDBOX_ROOT"] = str(rs_sandbox)
        cwd = rs_sandbox / case.get("cwd", ".")
        cwd.mkdir(parents=True, exist_ok=True)
        try:
            rs_result = subprocess.run(
                rs_cmd, cwd=cwd, env=rs_env, input=stdin_data,
                text=True, capture_output=True, timeout=timeout,
            )
            rs_exit, rs_stdout, rs_stderr = rs_result.returncode, rs_result.stdout, rs_result.stderr
        except subprocess.TimeoutExpired:
            rs_exit, rs_stdout, rs_stderr = 124, "", "TIMEOUT"
        except Exception as e:
            rs_exit, rs_stdout, rs_stderr = 1, "", f"ERROR: {e}"

        # Compare
        divergences = []
        match = True

        for target in case.get("compare", ["stdout", "stderr", "exit_code"]):
            if target == "exit_code":
                if py_exit != rs_exit:
                    divergences.append({
                        "field": "exit_code",
                        "python": py_exit,
                        "rust": rs_exit,
                    })
                    match = False
            elif target == "stdout":
                py_norm = _normalize(py_stdout, py_sandbox)
                rs_norm = _normalize(rs_stdout, rs_sandbox)
                # Try JSON-semantic comparison (ignores key ordering)
                py_json = _try_parse_json(py_norm)
                rs_json = _try_parse_json(rs_norm)
                if py_json is not None and rs_json is not None:
                    if py_json != rs_json:
                        divergences.append({
                            "field": "stdout",
                            "mode": "json",
                            "python": py_norm[:500],
                            "rust": rs_norm[:500],
                        })
                        match = False
                elif py_norm != rs_norm:
                    divergences.append({
                        "field": "stdout",
                        "python": py_norm[:500],
                        "rust": rs_norm[:500],
                    })
                    match = False
            elif target == "stderr":
                py_norm = _normalize(py_stderr, py_sandbox)
                rs_norm = _normalize(rs_stderr, rs_sandbox)
                if py_norm != rs_norm:
                    divergences.append({
                        "field": "stderr",
                        "python": py_norm[:500],
                        "rust": rs_norm[:500],
                    })
                    match = False
            elif target.startswith("fs:"):
                rel = target[3:]
                py_snap = _snapshot(py_sandbox / rel)
                rs_snap = _snapshot(rs_sandbox / rel)
                if py_snap != rs_snap:
                    divergences.append({
                        "field": target,
                        "python": _truncate_snapshot(py_snap),
                        "rust": _truncate_snapshot(rs_snap),
                    })
                    match = False

        elapsed = int((time.monotonic() - start) * 1000)

        return ShadowResult(
            case_name=case_name,
            category=category,
            match=match,
            python_exit=py_exit,
            rust_exit=rs_exit,
            python_stdout=py_stdout[:200],
            rust_stdout=rs_stdout[:200],
            python_stderr=py_stderr[:200],
            rust_stderr=rs_stderr[:200],
            divergences=divergences,
            duration_ms=elapsed,
            timestamp=datetime.now(timezone.utc).isoformat(),
            sandbox_python=str(py_sandbox),
            sandbox_rust=str(rs_sandbox),
        )
    finally:
        if not config.keep_sandboxes:
            shutil.rmtree(py_sandbox, ignore_errors=True)
            shutil.rmtree(rs_sandbox, ignore_errors=True)


def _try_parse_json(text: str) -> Any | None:
    """Try to parse text as JSON. Returns parsed value or None."""
    stripped = text.strip()
    if not stripped:
        return None
    try:
        return json.loads(stripped)
    except (json.JSONDecodeError, ValueError):
        return None


def _normalize(text: str, sandbox: Path) -> str:
    """Normalize output for comparison."""
    result = text.replace(str(sandbox), "<SANDBOX>")
    result = result.replace(str(sandbox / "home"), "<HOME>")
    return result.strip()


def _snapshot(path: Path) -> dict[str, Any]:
    """Snapshot a file or directory for comparison."""
    if not path.exists():
        return {"exists": False}
    if path.is_file():
        data = path.read_bytes()
        text = None
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            pass
        return {
            "exists": True,
            "type": "file",
            "sha256": hashlib.sha256(data).hexdigest(),
            "text": text,
        }
    entries = {}
    for child in sorted(path.rglob("*")):
        if child.is_file():
            rel = child.relative_to(path).as_posix()
            data = child.read_bytes()
            entries[rel] = hashlib.sha256(data).hexdigest()
    return {"exists": True, "type": "dir", "entries": entries}


def _truncate_snapshot(snap: dict[str, Any]) -> dict[str, Any]:
    """Truncate snapshot for logging."""
    result = dict(snap)
    if "text" in result and result["text"] and len(result["text"]) > 500:
        result["text"] = result["text"][:500] + "..."
    return result


# ---------------------------------------------------------------------------
# Shadow test cases (built-in)
# ---------------------------------------------------------------------------

SHADOW_CASES: list[dict[str, Any]] = [
    # CLI basics
    {
        "name": "shadow-version",
        "category": "cli",
        "argv": ["version"],
        "compare": ["exit_code"],
    },
    {
        "name": "shadow-help",
        "category": "cli",
        "argv": ["--help"],
        "compare": ["exit_code"],
    },
    # Mode detection
    {
        "name": "shadow-mode-detect",
        "category": "mode",
        "argv": ["mode", "detect"],
        "compare": ["stdout", "exit_code"],
    },
    # Recipe validation
    {
        "name": "shadow-recipe-validate-valid",
        "category": "recipe",
        "argv": ["recipe", "validate", "test.yaml", "--format", "json"],
        "setup": """
cat > test.yaml <<'EOF'
name: shadow-test
description: Shadow mode test recipe
steps:
  - id: step-1
    command: echo hello
EOF
""",
        "compare": ["stdout", "exit_code"],
    },
    {
        "name": "shadow-recipe-validate-invalid",
        "category": "recipe",
        "argv": ["recipe", "validate", "bad.yaml", "--format", "json"],
        "setup": """
cat > bad.yaml <<'EOF'
description: Missing name
steps: []
EOF
""",
        "compare": ["stdout", "exit_code"],
    },
    # Recipe list
    {
        "name": "shadow-recipe-list",
        "category": "recipe",
        "argv": ["recipe", "list", "recipes", "--format", "json"],
        "setup": """
mkdir -p recipes
cat > recipes/alpha.yaml <<'EOF'
name: alpha
description: Alpha recipe
tags: [core]
steps:
  - id: s1
    command: echo alpha
EOF
""",
        "compare": ["stdout", "exit_code"],
    },
    # Recipe dry-run
    {
        "name": "shadow-recipe-dry-run",
        "category": "recipe",
        "argv": ["recipe", "run", "demo.yaml", "--dry-run", "--format", "json"],
        "setup": """
cat > demo.yaml <<'EOF'
name: demo
description: Demo
steps:
  - id: s1
    command: echo hello
EOF
""",
        "compare": ["stdout", "exit_code"],
    },
    # Memory tree (empty)
    {
        "name": "shadow-memory-tree-empty",
        "category": "memory",
        "argv": ["memory", "tree", "--backend", "sqlite"],
        "compare": ["stdout", "exit_code"],
    },
    # Plugin verify missing
    {
        "name": "shadow-plugin-verify-missing",
        "category": "plugin",
        "argv": ["plugin", "verify", "nonexistent"],
        "compare": ["stdout", "stderr", "exit_code"],
    },
    # ── WS5: New parity cases (issue #39) ──────────────────────────────────
    # Install — local mode, idempotent.  exit_code only; stdout/stderr differ
    # across environments (paths, Python versions, package state).
    {
        "name": "shadow-install-local",
        "category": "install",
        "argv": ["install", "--local"],
        "compare": ["exit_code"],
    },
    # Uninstall — local mode.  exit_code only; same rationale as install.
    {
        "name": "shadow-uninstall-local",
        "category": "uninstall",
        "argv": ["uninstall", "--local"],
        "compare": ["exit_code"],
    },
    # Plugin list (distinct from verify-missing which tests error paths).
    {
        "name": "shadow-plugin-list",
        "category": "plugin",
        "argv": ["plugin", "list"],
        "compare": ["exit_code"],
    },
    # Memory status — lightweight query that does not require a running session.
    {
        "name": "shadow-memory-status",
        "category": "memory",
        "argv": ["memory", "status", "--backend", "sqlite"],
        "compare": ["exit_code"],
    },
]


# ---------------------------------------------------------------------------
# Harness runner
# ---------------------------------------------------------------------------

def run_shadow_harness(config: ShadowConfig, cases: list[dict[str, Any]]) -> dict[str, Any]:
    """Run shadow harness over all cases, return summary."""
    config.log_dir.mkdir(parents=True, exist_ok=True)

    results: list[ShadowResult] = []
    total = len(cases)

    for i, case in enumerate(cases, 1):
        name = case["name"]
        print(f"[{i}/{total}] {name} ... ", end="", flush=True)
        result = run_shadow_case(config, case)
        results.append(result)

        status = "MATCH" if result.match else "DIVERGED"
        print(f"{status} ({result.duration_ms}ms)")

        # Log divergences atomically (append-only JSONL)
        if not result.match:
            with open(config.divergence_log, "a", encoding="utf-8") as f:
                record = {
                    "run_id": config.run_id,
                    "case": result.case_name,
                    "category": result.category,
                    "divergences": result.divergences,
                    "timestamp": result.timestamp,
                }
                f.write(json.dumps(record, sort_keys=True) + "\n")

    # Summary
    matched = sum(1 for r in results if r.match)
    diverged = total - matched
    by_category: dict[str, dict[str, int]] = {}
    for r in results:
        cat = r.category
        if cat not in by_category:
            by_category[cat] = {"matched": 0, "diverged": 0}
        if r.match:
            by_category[cat]["matched"] += 1
        else:
            by_category[cat]["diverged"] += 1

    summary = {
        "run_id": config.run_id,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "total": total,
        "matched": matched,
        "diverged": diverged,
        "parity_rate": f"{matched / total * 100:.1f}%" if total > 0 else "N/A",
        "by_category": by_category,
        "divergence_details": [
            {"case": r.case_name, "divergences": r.divergences}
            for r in results if not r.match
        ],
    }

    config.summary_log.write_text(
        json.dumps(summary, indent=2), encoding="utf-8"
    )

    print(f"\n{'=' * 60}")
    print(f"SHADOW PARITY SUMMARY")
    print(f"{'=' * 60}")
    print(f"Run ID:    {config.run_id}")
    print(f"Total:     {total}")
    print(f"Matched:   {matched}")
    print(f"Diverged:  {diverged}")
    print(f"Parity:    {summary['parity_rate']}")
    for cat, counts in sorted(by_category.items()):
        print(f"  {cat}: {counts['matched']} match, {counts['diverged']} diverged")
    if diverged > 0:
        print(f"\nDivergence log: {config.divergence_log}")
    print(f"Summary:        {config.summary_log}")

    return summary


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Shadow-mode parity harness")
    parser.add_argument(
        "--python-repo",
        type=Path,
        default=Path.home() / "src" / "amploxy",
    )
    parser.add_argument(
        "--python-exe",
        type=Path,
        default=None,
    )
    parser.add_argument(
        "--rust-binary",
        type=Path,
        default=Path.home() / "src" / "amplihack-rs" / "target" / "debug" / "amplihack",
    )
    parser.add_argument(
        "--log-dir",
        type=Path,
        default=DEFAULT_LOG_DIR,
    )
    parser.add_argument("--keep-sandboxes", action="store_true")
    parser.add_argument(
        "--cases-file",
        type=Path,
        help="Optional YAML file with additional shadow cases",
    )
    parser.add_argument(
        "--only-builtin",
        action="store_true",
        help="Run only built-in shadow cases (skip scenario files)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    config = ShadowConfig(
        python_repo=args.python_repo.resolve(),
        python_exe=args.python_exe,
        rust_binary=args.rust_binary.resolve(),
        log_dir=args.log_dir,
        divergence_log=args.log_dir / "divergences.jsonl",
        keep_sandboxes=args.keep_sandboxes,
    )
    config.summary_log = config.log_dir / "summary.json"

    cases = list(SHADOW_CASES)

    if args.cases_file and args.cases_file.exists():
        import yaml
        extra = yaml.safe_load(args.cases_file.read_text())
        if isinstance(extra, dict) and "cases" in extra:
            for c in extra["cases"]:
                c.setdefault("category", "custom")
            cases.extend(extra["cases"])

    if not config.rust_binary.exists():
        print(f"ERROR: Rust binary not found: {config.rust_binary}", file=sys.stderr)
        return 1

    summary = run_shadow_harness(config, cases)
    return 0 if summary["diverged"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
