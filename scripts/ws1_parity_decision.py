"""
WS1 Parity Decision Engine.

Determines whether PR #44 (launcher-passthrough-args parity test) should be
CLOSED as superseded by PR #65 or REBASED onto main.

Decision logic:
  - Run parity test TWICE on main branch to detect flakiness
  - If BOTH runs exit 0 → PRDecision.CLOSE
  - If EITHER run exits non-zero → PRDecision.REBASE

Exported API:
  class PRDecision(enum.Enum): CLOSE, REBASE
  @dataclass class ParityTestRun: exit_code, stdout, stderr, run_number, passed
  @dataclass class DecisionResult: decision, runs, close_comment, rebase_reason
  def make_decision(run1, run2) -> DecisionResult
  def run_parity_test_once(scenario_path, case_name, run_number) -> ParityTestRun
  def build_close_comment(run1, run2) -> str
  def validate_scenario_path(path) -> Path
  def validate_case_name(name) -> str
"""

from __future__ import annotations

import enum
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

# Resolve repo root relative to this script's location
_SCRIPT_DIR = Path(__file__).resolve().parent
_REPO_ROOT = _SCRIPT_DIR.parent

# Add shared directory to path for output_sanitizer
_SHARED_DIR = _SCRIPT_DIR / "shared"
if str(_SHARED_DIR) not in sys.path:
    sys.path.insert(0, str(_SHARED_DIR))

from output_sanitizer import sanitize  # noqa: E402

# Security: case names must be alphanumeric + hyphen only
_CASE_NAME_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9\-]*$")


class PRDecision(enum.Enum):
    """Decision outcome for a parity PR."""

    CLOSE = "close"
    REBASE = "rebase"


@dataclass
class ParityTestRun:
    """Records the outcome of a single parity test execution."""

    exit_code: int
    stdout: str
    stderr: str
    run_number: int

    @property
    def passed(self) -> bool:
        """Return True if the test run exited with code 0."""
        return self.exit_code == 0


@dataclass
class DecisionResult:
    """The outcome of the parity decision engine."""

    decision: PRDecision
    runs: list
    close_comment: Optional[str]
    rebase_reason: Optional[str]


def validate_scenario_path(path: str | Path) -> Path:
    """Validate that the scenario path is within tests/parity/.

    Args:
        path: The scenario path to validate. May be relative or absolute.

    Returns:
        A resolved Path object pointing to the scenario file.

    Raises:
        ValueError: If the path is outside tests/parity/ or uses path traversal.
    """
    resolved = Path(path).resolve()
    allowed_root = (_REPO_ROOT / "tests" / "parity").resolve()

    # Check that the resolved path starts with the allowed root
    try:
        resolved.relative_to(allowed_root)
    except ValueError:
        raise ValueError(
            f"Scenario path must be within tests/parity/. "
            f"Got: {path!r} (resolved: {resolved}). "
            f"Allowed root: {allowed_root}"
        )

    return resolved


def validate_case_name(name: str) -> str:
    """Validate that the case name is alphanumeric + hyphens only.

    This prevents shell injection and path traversal attacks.

    Args:
        name: The case name to validate.

    Returns:
        The validated case name unchanged.

    Raises:
        ValueError: If the case name contains invalid characters.
    """
    if not name:
        raise ValueError("Case name must not be empty.")

    if not _CASE_NAME_PATTERN.match(name):
        raise ValueError(
            f"Case name must be alphanumeric and hyphens only (no spaces, "
            f"special characters, or path separators). Got: {name!r}"
        )

    return name


def run_parity_test_once(
    scenario_path: str | Path,
    case_name: str,
    run_number: int,
) -> ParityTestRun:
    """Run the parity test once and return the result.

    Security: uses subprocess with list args (no shell=True) to prevent
    shell injection. Inputs are validated before use.

    Args:
        scenario_path: Path to the YAML scenario file (must be in tests/parity/).
        case_name: Name of the test case to run (alphanumeric + hyphens only).
        run_number: Which run this is (1 or 2), for recording purposes.

    Returns:
        ParityTestRun with exit code, stdout, stderr, and run_number.
    """
    validated_path = validate_scenario_path(scenario_path)
    validated_case = validate_case_name(case_name)

    validate_cli_parity = _REPO_ROOT / "tests" / "parity" / "validate_cli_parity.py"

    cmd = [
        sys.executable,
        str(validate_cli_parity),
        "--scenario",
        str(validated_path),
        "--case",
        validated_case,
    ]

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=300,  # 5 minutes — prevents indefinite hang
        )
    except subprocess.TimeoutExpired as exc:
        return ParityTestRun(
            exit_code=1,
            stdout=exc.stdout.decode()
            if isinstance(exc.stdout, bytes)
            else (exc.stdout or ""),
            stderr="Parity test timed out after 300 s\n"
            + (
                exc.stderr.decode()
                if isinstance(exc.stderr, bytes)
                else (exc.stderr or "")
            ),
            run_number=run_number,
        )

    return ParityTestRun(
        exit_code=result.returncode,
        stdout=result.stdout,
        stderr=result.stderr,
        run_number=run_number,
    )


def build_close_comment(run1: ParityTestRun, run2: ParityTestRun) -> str:
    """Build a PR close comment for when both parity runs pass.

    The comment references PR #65, mentions the test name, includes sanitized
    output from both runs, and states that the PR is superseded.

    Args:
        run1: First parity test run result.
        run2: Second parity test run result.

    Returns:
        A Markdown-formatted PR comment string.
    """
    safe_stdout1 = sanitize(run1.stdout) or ""
    safe_stderr1 = sanitize(run1.stderr) or ""
    safe_stdout2 = sanitize(run2.stdout) or ""
    safe_stderr2 = sanitize(run2.stderr) or ""

    return f"""This PR is superseded by #65 (launcher flag injection fix).

The `launcher-passthrough-args` parity test was run twice on main to verify
that the functionality is already covered:

**Run {run1.run_number}** (exit code: {run1.exit_code})
```
{safe_stdout1}{safe_stderr1}
```

**Run {run2.run_number}** (exit code: {run2.exit_code})
```
{safe_stdout2}{safe_stderr2}
```

Both runs passed, confirming the parity test is green on main without this PR's
changes. Closing as superseded by #65.
""".strip()


def make_decision(run1: ParityTestRun, run2: ParityTestRun) -> DecisionResult:
    """Determine whether to CLOSE or REBASE PR #44.

    Logic:
      - BOTH runs exit 0 → CLOSE (test is reliably passing on main)
      - EITHER run exits non-zero → REBASE (test fails or is flaky on main)

    Args:
        run1: First parity test run.
        run2: Second parity test run.

    Returns:
        DecisionResult with the decision, both runs, and appropriate comment/reason.
    """
    runs = [run1, run2]

    if run1.passed and run2.passed:
        return DecisionResult(
            decision=PRDecision.CLOSE,
            runs=runs,
            close_comment=build_close_comment(run1, run2),
            rebase_reason=None,
        )

    # Determine reason for REBASE
    if not run1.passed and not run2.passed:
        rebase_reason = (
            f"Both parity test runs failed on main. "
            f"Run 1 exit code: {run1.exit_code}. "
            f"Run 2 exit code: {run2.exit_code}. "
            f"The test is consistently failing; rebase to investigate."
        )
    elif not run1.passed:
        rebase_reason = (
            f"Parity test run 1 failed on main (exit code: {run1.exit_code}). "
            f"Run 2 passed. Rebase PR #44 onto main and re-run to confirm."
        )
    else:
        # run2 failed — flaky test
        rebase_reason = (
            f"Parity test is flaky on main: run 1 passed but run 2 failed "
            f"(exit code: {run2.exit_code}). "
            f"Rebase PR #44 onto main and investigate flakiness before closing."
        )

    return DecisionResult(
        decision=PRDecision.REBASE,
        runs=runs,
        close_comment=None,
        rebase_reason=rebase_reason,
    )
