"""
TDD Step 7: Integration tests for WS1 — parity test infrastructure validation.

These tests verify that the parity test scaffolding (validate_cli_parity.py,
tier5-launcher.yaml) is correctly structured to support the decision engine.

Unlike test_ws1_parity_decision.py (unit tests with mocked subprocess), these
tests interact with actual files in the repository.

EXPECTED STATUS:
  - Tests in TestParityTestScenarioStructure: PASS NOW (file structure correct)
  - Tests in TestValidateCliParityScript: PASS NOW (script is well-formed)
  - Tests in TestLauncherPassthroughArgsCase: PASS NOW (case spec is valid)
  - Tests in TestParityDecisionIntegration: FAIL until ws1_parity_decision.py exists
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
TIER5_LAUNCHER_YAML = (
    REPO_ROOT / "tests" / "parity" / "scenarios" / "tier5-launcher.yaml"
)
VALIDATE_CLI_PARITY_PY = REPO_ROOT / "tests" / "parity" / "validate_cli_parity.py"
sys.path.insert(0, str(REPO_ROOT / "scripts"))


# ---------------------------------------------------------------------------
# File structure validation (PASS NOW)
# ---------------------------------------------------------------------------


class TestParityTestScenarioStructure:
    """Validates that tier5-launcher.yaml has the correct structure for WS1."""

    def test_tier5_launcher_yaml_exists(self):
        assert TIER5_LAUNCHER_YAML.exists(), (
            f"tier5-launcher.yaml not found at {TIER5_LAUNCHER_YAML}. "
            "This file is required for the WS1 parity test decision."
        )

    def test_tier5_launcher_yaml_is_valid_yaml(self):
        with open(TIER5_LAUNCHER_YAML, "r") as f:
            content = yaml.safe_load(f)
        assert content is not None, "tier5-launcher.yaml must not be empty"

    def test_tier5_launcher_yaml_has_cases_key(self):
        with open(TIER5_LAUNCHER_YAML, "r") as f:
            content = yaml.safe_load(f)
        assert "cases" in content, (
            "tier5-launcher.yaml must have a top-level 'cases' key. "
            f"Got keys: {list(content.keys())}"
        )

    def test_tier5_launcher_yaml_has_launcher_passthrough_args(self):
        with open(TIER5_LAUNCHER_YAML, "r") as f:
            content = yaml.safe_load(f)
        case_names = [c.get("name") for c in content.get("cases", [])]
        assert "launcher-passthrough-args" in case_names, (
            "tier5-launcher.yaml must contain the 'launcher-passthrough-args' case. "
            f"Found cases: {case_names}"
        )

    def test_launcher_passthrough_args_case_structure(self):
        """The launcher-passthrough-args case must have all required fields."""
        with open(TIER5_LAUNCHER_YAML, "r") as f:
            content = yaml.safe_load(f)
        cases = {c["name"]: c for c in content.get("cases", [])}
        assert "launcher-passthrough-args" in cases

        case = cases["launcher-passthrough-args"]
        assert "argv" in case, f"Case must have 'argv'. Got: {list(case.keys())}"
        assert "compare" in case, f"Case must have 'compare'. Got: {list(case.keys())}"
        assert "setup" in case, f"Case must have 'setup'. Got: {list(case.keys())}"
        assert "timeout" in case, f"Case must have 'timeout'. Got: {list(case.keys())}"

    def test_launcher_passthrough_args_argv_contains_extra_args(self):
        """The argv must include -- followed by extra args (tests passthrough)."""
        with open(TIER5_LAUNCHER_YAML, "r") as f:
            content = yaml.safe_load(f)
        cases = {c["name"]: c for c in content.get("cases", [])}
        case = cases["launcher-passthrough-args"]
        argv = case.get("argv", [])
        assert (
            "--" in argv
        ), f"launcher-passthrough-args argv must contain '--' separator. Got: {argv}"
        # Must have at least one extra arg after --
        sep_idx = argv.index("--")
        assert len(argv) > sep_idx + 1, f"Must have args after '--'. Got: {argv}"

    def test_launcher_passthrough_args_compares_exit_code(self):
        with open(TIER5_LAUNCHER_YAML, "r") as f:
            content = yaml.safe_load(f)
        cases = {c["name"]: c for c in content.get("cases", [])}
        case = cases["launcher-passthrough-args"]
        compare = case.get("compare", [])
        assert (
            "exit_code" in compare
        ), f"launcher-passthrough-args must compare exit_code. Got compare: {compare}"

    def test_launcher_passthrough_args_compares_claude_args_file(self):
        """Must compare fs:claude_args.txt to verify args were passed through."""
        with open(TIER5_LAUNCHER_YAML, "r") as f:
            content = yaml.safe_load(f)
        cases = {c["name"]: c for c in content.get("cases", [])}
        case = cases["launcher-passthrough-args"]
        compare = case.get("compare", [])
        fs_compares = [c for c in compare if isinstance(c, str) and c.startswith("fs:")]
        assert fs_compares, f"launcher-passthrough-args must compare filesystem file(s). Got compare: {compare}"
        assert any(
            "claude_args" in c for c in fs_compares
        ), f"Must compare 'fs:claude_args.txt'. Got fs compares: {fs_compares}"

    def test_launcher_passthrough_args_has_reasonable_timeout(self):
        with open(TIER5_LAUNCHER_YAML, "r") as f:
            content = yaml.safe_load(f)
        cases = {c["name"]: c for c in content.get("cases", [])}
        case = cases["launcher-passthrough-args"]
        timeout = case.get("timeout", 0)
        assert 5 <= timeout <= 60, (
            f"launcher-passthrough-args timeout must be between 5 and 60 seconds. "
            f"Got: {timeout}"
        )


# ---------------------------------------------------------------------------
# validate_cli_parity.py script validation (PASS NOW)
# ---------------------------------------------------------------------------


class TestValidateCliParityScript:
    """Validates that the parity runner script is well-formed."""

    def test_validate_cli_parity_py_exists(self):
        assert (
            VALIDATE_CLI_PARITY_PY.exists()
        ), f"validate_cli_parity.py not found at {VALIDATE_CLI_PARITY_PY}"

    def test_validate_cli_parity_py_is_executable(self):
        assert os.access(
            VALIDATE_CLI_PARITY_PY, os.R_OK
        ), "validate_cli_parity.py must be readable"

    def test_validate_cli_parity_py_has_scenario_arg(self):
        """Script must accept --scenario argument (used by decision engine)."""
        result = subprocess.run(
            [sys.executable, str(VALIDATE_CLI_PARITY_PY), "--help"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        help_text = result.stdout + result.stderr
        assert "--scenario" in help_text, (
            "validate_cli_parity.py --help must mention --scenario. "
            f"Got: {help_text[:200]}"
        )

    def test_validate_cli_parity_py_has_case_arg(self):
        """Script must accept --case argument (for running specific test cases)."""
        result = subprocess.run(
            [sys.executable, str(VALIDATE_CLI_PARITY_PY), "--help"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        help_text = result.stdout + result.stderr
        assert "--case" in help_text, (
            "validate_cli_parity.py --help must mention --case. "
            f"Got: {help_text[:200]}"
        )

    def test_validate_cli_parity_py_help_exits_zero(self):
        result = subprocess.run(
            [sys.executable, str(VALIDATE_CLI_PARITY_PY), "--help"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert result.returncode == 0, (
            f"validate_cli_parity.py --help must exit 0. Got: {result.returncode}\n"
            f"stderr: {result.stderr[:200]}"
        )


# ---------------------------------------------------------------------------
# Decision engine integration: round-trip test (FAIL until ws1_parity_decision.py)
# ---------------------------------------------------------------------------


class TestParityDecisionIntegration:
    """
    Integration tests that exercise the full WS1 decision pipeline.

    These tests FAIL until scripts/ws1_parity_decision.py exists.
    They use mocked subprocess calls to avoid requiring the full Python repo.
    """

    def test_import_ws1_parity_decision(self):
        """
        FAILS: scripts/ws1_parity_decision.py does not exist.
        PASSES: Once the script is created.
        """
        try:
            import ws1_parity_decision  # noqa: F401
        except ImportError:
            pytest.fail(
                "scripts/ws1_parity_decision.py not found. "
                "FIX: Create scripts/ws1_parity_decision.py"
            )

    def test_run_parity_test_once_uses_correct_script_path(self):
        """
        FAILS: scripts/ws1_parity_decision.py does not exist.
        PASSES: Once implemented.

        Verifies that run_parity_test_once invokes validate_cli_parity.py
        from the correct repo path (not a hardcoded absolute path).
        """
        try:
            from unittest.mock import MagicMock, patch
            from ws1_parity_decision import run_parity_test_once
        except ImportError:
            pytest.fail(
                "scripts/ws1_parity_decision.py not found. "
                "FIX: Create scripts/ws1_parity_decision.py"
            )

        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0, stdout="PASS\n", stderr="")
            run_parity_test_once(
                scenario_path="tests/parity/scenarios/tier5-launcher.yaml",
                case_name="launcher-passthrough-args",
                run_number=1,
            )

            call_args = mock_run.call_args
            cmd = call_args.args[0]

            # The command must invoke validate_cli_parity.py
            cmd_str = " ".join(str(a) for a in cmd)
            assert "validate_cli_parity" in cmd_str, (
                "run_parity_test_once must invoke validate_cli_parity.py. "
                f"Got command: {cmd_str!r}"
            )
            # Must pass the scenario path
            assert (
                "tier5-launcher.yaml" in cmd_str
            ), f"Command must include the scenario path. Got: {cmd_str!r}"
            # Must pass the case name
            assert (
                "launcher-passthrough-args" in cmd_str
            ), f"Command must include --case launcher-passthrough-args. Got: {cmd_str!r}"

    def test_decision_pipeline_close_path(self):
        """
        FAILS: scripts/ws1_parity_decision.py does not exist.
        PASSES: Once implemented.

        Full pipeline: 2 passing runs → CLOSE decision with valid PR comment.
        """
        try:
            from unittest.mock import MagicMock, patch
            from ws1_parity_decision import (
                PRDecision,
                make_decision,
                run_parity_test_once,
            )
        except ImportError:
            pytest.fail("scripts/ws1_parity_decision.py not found")

        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(
                returncode=0,
                stdout="launcher-passthrough-args: PASS\nexit_code: 0\n",
                stderr="",
            )
            run1 = run_parity_test_once(
                scenario_path="tests/parity/scenarios/tier5-launcher.yaml",
                case_name="launcher-passthrough-args",
                run_number=1,
            )
            run2 = run_parity_test_once(
                scenario_path="tests/parity/scenarios/tier5-launcher.yaml",
                case_name="launcher-passthrough-args",
                run_number=2,
            )

        result = make_decision(run1, run2)
        assert result.decision == PRDecision.CLOSE
        assert result.close_comment is not None
        assert (
            len(result.close_comment) > 50
        ), f"Close comment must be substantive (>50 chars). Got: {result.close_comment!r}"

    def test_decision_pipeline_rebase_path(self):
        """
        FAILS: scripts/ws1_parity_decision.py does not exist.
        PASSES: Once implemented.

        Full pipeline: 1 failing run → REBASE decision.
        """
        try:
            from unittest.mock import MagicMock, patch
            from ws1_parity_decision import (
                PRDecision,
                make_decision,
                run_parity_test_once,
            )
        except ImportError:
            pytest.fail("scripts/ws1_parity_decision.py not found")

        with patch("subprocess.run") as mock_run:
            # First run fails
            mock_run.return_value = MagicMock(
                returncode=1,
                stdout="",
                stderr="launcher-passthrough-args: FAIL\ndivergence: fs:claude_args.txt\n",
            )
            run1 = run_parity_test_once(
                scenario_path="tests/parity/scenarios/tier5-launcher.yaml",
                case_name="launcher-passthrough-args",
                run_number=1,
            )
            # Second run passes (flaky)
            mock_run.return_value = MagicMock(returncode=0, stdout="PASS\n", stderr="")
            run2 = run_parity_test_once(
                scenario_path="tests/parity/scenarios/tier5-launcher.yaml",
                case_name="launcher-passthrough-args",
                run_number=2,
            )

        result = make_decision(run1, run2)
        assert (
            result.decision == PRDecision.REBASE
        ), f"Failing first run must give REBASE. Got: {result.decision}"
        assert result.rebase_reason, "REBASE result must have a rebase_reason"
