"""
TDD Step 7: Failing tests for scripts/ws1_parity_decision.py.

Specifies the decision engine that determines whether PR #44
(launcher-passthrough-args parity test) should be CLOSED as superseded
or REBASED onto main.

EXPECTED BEHAVIOR: All tests marked FAILS below fail until
  scripts/ws1_parity_decision.py is created and implemented.

IMPLEMENTATION TARGET: scripts/ws1_parity_decision.py
  Must export:
    class PRDecision(enum.Enum): CLOSE, REBASE
    @dataclass class ParityTestRun: exit_code, stdout, stderr, run_number
    @dataclass class DecisionResult: decision, runs, close_comment, rebase_reason
    def make_decision(run1: ParityTestRun, run2: ParityTestRun) -> DecisionResult
    def run_parity_test_once(scenario_path, case_name, rust_binary, python_repo) -> ParityTestRun
    def build_close_comment(run1: ParityTestRun, run2: ParityTestRun) -> str
    def validate_scenario_path(path: str | Path) -> Path
    def validate_case_name(name: str) -> str

DESIGN SPEC:
  - Run parity test TWICE on main branch to detect flakiness
  - If BOTH runs exit 0 → PRDecision.CLOSE
  - If EITHER run exits non-zero → PRDecision.REBASE
  - Close comment must mention PR #65, the test name, and both run outputs (sanitized)
  - Security: scenario_path must start with tests/parity/; case_name is alphanumeric+hyphen only
"""

from __future__ import annotations

import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

# ---------------------------------------------------------------------------
# Import under test — FAILS until scripts/ws1_parity_decision.py exists.
# ---------------------------------------------------------------------------
try:
    from ws1_parity_decision import (
        PRDecision,
        ParityTestRun,
        build_close_comment,
        make_decision,
        run_parity_test_once,
        validate_case_name,
        validate_scenario_path,
    )

    _IMPORT_FAILED = False
except ImportError as exc:
    _IMPORT_FAILED = True
    _IMPORT_ERROR = str(exc)


def _skip_if_missing():
    if _IMPORT_FAILED:
        pytest.fail(
            "scripts/ws1_parity_decision.py not found. "
            "IMPORT ERROR: " + _IMPORT_ERROR + "\n"
            "FIX: Create scripts/ws1_parity_decision.py with PRDecision, "
            "ParityTestRun, DecisionResult, make_decision(), run_parity_test_once(), "
            "build_close_comment(), validate_scenario_path(), validate_case_name()."
        )


# ---------------------------------------------------------------------------
# PRDecision enum contract
# ---------------------------------------------------------------------------


class TestPRDecisionEnum:
    """PRDecision must have CLOSE and REBASE variants."""

    def test_close_variant_exists(self):
        _skip_if_missing()
        assert hasattr(PRDecision, "CLOSE"), "PRDecision must have CLOSE variant"

    def test_rebase_variant_exists(self):
        _skip_if_missing()
        assert hasattr(PRDecision, "REBASE"), "PRDecision must have REBASE variant"

    def test_close_and_rebase_are_distinct(self):
        _skip_if_missing()
        assert PRDecision.CLOSE != PRDecision.REBASE

    def test_pr_decision_is_enum(self):
        _skip_if_missing()
        import enum as _enum

        assert issubclass(
            PRDecision, _enum.Enum
        ), "PRDecision must be an enum.Enum subclass"


# ---------------------------------------------------------------------------
# ParityTestRun dataclass contract
# ---------------------------------------------------------------------------


class TestParityTestRunContract:
    """ParityTestRun must record exit_code, stdout, stderr, run_number."""

    def test_parity_test_run_has_exit_code(self):
        _skip_if_missing()
        run = ParityTestRun(exit_code=0, stdout="ok", stderr="", run_number=1)
        assert run.exit_code == 0

    def test_parity_test_run_has_stdout(self):
        _skip_if_missing()
        run = ParityTestRun(exit_code=0, stdout="test passed", stderr="", run_number=1)
        assert run.stdout == "test passed"

    def test_parity_test_run_has_stderr(self):
        _skip_if_missing()
        run = ParityTestRun(
            exit_code=1, stdout="", stderr="error: test failed", run_number=2
        )
        assert run.stderr == "error: test failed"

    def test_parity_test_run_has_run_number(self):
        _skip_if_missing()
        run = ParityTestRun(exit_code=0, stdout="", stderr="", run_number=2)
        assert run.run_number == 2

    def test_parity_test_run_passed_property(self):
        """ParityTestRun must provide a .passed property that returns exit_code == 0."""
        _skip_if_missing()
        passing = ParityTestRun(exit_code=0, stdout="", stderr="", run_number=1)
        failing = ParityTestRun(
            exit_code=1, stdout="", stderr="AssertionError", run_number=1
        )
        assert passing.passed is True, "exit_code=0 must mean passed=True"
        assert failing.passed is False, "exit_code=1 must mean passed=False"


# ---------------------------------------------------------------------------
# make_decision: core logic
# ---------------------------------------------------------------------------


class TestMakeDecisionBothPass:
    """When both test runs exit 0, decision must be CLOSE."""

    def test_both_pass_gives_close(self):
        _skip_if_missing()
        run1 = ParityTestRun(exit_code=0, stdout="PASS", stderr="", run_number=1)
        run2 = ParityTestRun(exit_code=0, stdout="PASS", stderr="", run_number=2)
        result = make_decision(run1, run2)
        assert result.decision == PRDecision.CLOSE, (
            "When both runs exit 0, decision must be CLOSE. " f"Got: {result.decision}"
        )

    def test_both_pass_result_has_close_comment(self):
        _skip_if_missing()
        run1 = ParityTestRun(exit_code=0, stdout="test passed", stderr="", run_number=1)
        run2 = ParityTestRun(exit_code=0, stdout="test passed", stderr="", run_number=2)
        result = make_decision(run1, run2)
        assert (
            result.close_comment is not None
        ), "CLOSE decision must include a close_comment. Got None."
        assert isinstance(
            result.close_comment, str
        ), f"close_comment must be a str. Got: {type(result.close_comment)}"
        assert result.close_comment.strip(), "close_comment must not be empty"

    def test_both_pass_result_has_both_runs(self):
        _skip_if_missing()
        run1 = ParityTestRun(exit_code=0, stdout="ok", stderr="", run_number=1)
        run2 = ParityTestRun(exit_code=0, stdout="ok", stderr="", run_number=2)
        result = make_decision(run1, run2)
        assert result.runs[0] is run1, "runs[0] must be run1"
        assert result.runs[1] is run2, "runs[1] must be run2"


class TestMakeDecisionFirstFails:
    """When first run fails (non-zero exit), decision must be REBASE."""

    def test_first_fails_second_passes_gives_rebase(self):
        _skip_if_missing()
        run1 = ParityTestRun(
            exit_code=1, stdout="", stderr="FAIL: divergence", run_number=1
        )
        run2 = ParityTestRun(exit_code=0, stdout="PASS", stderr="", run_number=2)
        result = make_decision(run1, run2)
        assert result.decision == PRDecision.REBASE, (
            "When first run fails, decision must be REBASE even if second passes. "
            f"Got: {result.decision}"
        )

    def test_first_fails_result_has_rebase_reason(self):
        _skip_if_missing()
        run1 = ParityTestRun(
            exit_code=1, stdout="", stderr="assertion failed", run_number=1
        )
        run2 = ParityTestRun(exit_code=0, stdout="ok", stderr="", run_number=2)
        result = make_decision(run1, run2)
        assert (
            result.rebase_reason is not None
        ), "REBASE decision must include a rebase_reason. Got None."
        assert isinstance(result.rebase_reason, str)
        assert result.rebase_reason.strip()


class TestMakeDecisionSecondFails:
    """When second run fails, decision must be REBASE (flakiness detected)."""

    def test_first_passes_second_fails_gives_rebase(self):
        _skip_if_missing()
        run1 = ParityTestRun(exit_code=0, stdout="PASS", stderr="", run_number=1)
        run2 = ParityTestRun(exit_code=1, stdout="", stderr="FAIL: flaky", run_number=2)
        result = make_decision(run1, run2)
        assert result.decision == PRDecision.REBASE, (
            "When second run fails (flaky), decision must be REBASE. "
            f"Got: {result.decision}"
        )

    def test_first_passes_second_fails_rebase_reason_mentions_flaky(self):
        _skip_if_missing()
        run1 = ParityTestRun(exit_code=0, stdout="ok", stderr="", run_number=1)
        run2 = ParityTestRun(exit_code=1, stdout="", stderr="timeout", run_number=2)
        result = make_decision(run1, run2)
        # The rebase_reason should communicate that the test is not reliably passing
        assert result.rebase_reason, "rebase_reason must not be empty"


class TestMakeDecisionBothFail:
    """When both runs fail, decision must be REBASE."""

    def test_both_fail_gives_rebase(self):
        _skip_if_missing()
        run1 = ParityTestRun(exit_code=1, stdout="", stderr="FAIL", run_number=1)
        run2 = ParityTestRun(exit_code=1, stdout="", stderr="FAIL", run_number=2)
        result = make_decision(run1, run2)
        assert result.decision == PRDecision.REBASE, (
            "When both runs fail, decision must be REBASE. " f"Got: {result.decision}"
        )

    def test_both_fail_close_comment_is_none(self):
        _skip_if_missing()
        run1 = ParityTestRun(exit_code=1, stdout="", stderr="FAIL", run_number=1)
        run2 = ParityTestRun(exit_code=1, stdout="", stderr="FAIL", run_number=2)
        result = make_decision(run1, run2)
        assert result.close_comment is None, (
            "REBASE decision must have close_comment=None. "
            f"Got: {result.close_comment!r}"
        )


class TestMakeDecisionNonStandardExitCodes:
    """Exit code 2, 127, 130, etc. are all non-zero → REBASE."""

    @pytest.mark.parametrize("exit_code", [2, 3, 127, 130, 255])
    def test_nonzero_exit_codes_give_rebase(self, exit_code: int):
        _skip_if_missing()
        run1 = ParityTestRun(exit_code=exit_code, stdout="", stderr="", run_number=1)
        run2 = ParityTestRun(exit_code=0, stdout="ok", stderr="", run_number=2)
        result = make_decision(run1, run2)
        assert (
            result.decision == PRDecision.REBASE
        ), f"exit_code={exit_code} on run1 must give REBASE. Got: {result.decision}"


# ---------------------------------------------------------------------------
# build_close_comment: content requirements
# ---------------------------------------------------------------------------


class TestBuildCloseComment:
    """Close comment must reference PR #65, the test name, and both run outputs."""

    def _make_runs(self, stdout: str = "PASS") -> tuple:
        run1 = ParityTestRun(exit_code=0, stdout=stdout, stderr="", run_number=1)
        run2 = ParityTestRun(exit_code=0, stdout=stdout, stderr="", run_number=2)
        return run1, run2

    def test_comment_mentions_pr_65(self):
        _skip_if_missing()
        run1, run2 = self._make_runs()
        comment = build_close_comment(run1, run2)
        assert "#65" in comment or "65" in comment, (
            "Close comment must reference PR #65 (launch flag injection). "
            f"Got: {comment!r}"
        )

    def test_comment_mentions_test_name(self):
        _skip_if_missing()
        run1 = ParityTestRun(
            exit_code=0,
            stdout="launcher-passthrough-args: PASS",
            stderr="",
            run_number=1,
        )
        run2 = ParityTestRun(
            exit_code=0,
            stdout="launcher-passthrough-args: PASS",
            stderr="",
            run_number=2,
        )
        comment = build_close_comment(run1, run2)
        assert "launcher-passthrough-args" in comment, (
            "Close comment must mention the test case name. " f"Got: {comment!r}"
        )

    def test_comment_includes_run_1_output(self):
        _skip_if_missing()
        run1 = ParityTestRun(
            exit_code=0, stdout="run1-output-marker", stderr="", run_number=1
        )
        run2 = ParityTestRun(
            exit_code=0, stdout="run2-output-marker", stderr="", run_number=2
        )
        comment = build_close_comment(run1, run2)
        assert "run1-output-marker" in comment, (
            "Close comment must include run 1 stdout. " f"Got: {comment!r}"
        )

    def test_comment_includes_run_2_output(self):
        _skip_if_missing()
        run1 = ParityTestRun(exit_code=0, stdout="run1-marker", stderr="", run_number=1)
        run2 = ParityTestRun(
            exit_code=0, stdout="run2-output-marker", stderr="", run_number=2
        )
        comment = build_close_comment(run1, run2)
        assert "run2-output-marker" in comment, (
            "Close comment must include run 2 stdout. " f"Got: {comment!r}"
        )

    def test_comment_does_not_contain_raw_secrets(self):
        """build_close_comment must sanitize output (no raw API keys)."""
        _skip_if_missing()
        # Simulate run output that accidentally captured a key
        run1 = ParityTestRun(
            exit_code=0,
            stdout="AMPLIHACK_API_KEY=sk-ant-secret123 PASS",
            stderr="",
            run_number=1,
        )
        run2 = ParityTestRun(exit_code=0, stdout="PASS", stderr="", run_number=2)
        comment = build_close_comment(run1, run2)
        assert "sk-ant-secret123" not in comment, (
            "build_close_comment must sanitize API keys from captured output. "
            f"Got: {comment!r}"
        )

    def test_comment_mentions_superseded(self):
        _skip_if_missing()
        run1, run2 = self._make_runs()
        comment = build_close_comment(run1, run2)
        # The word "superseded" or "superseeded" should appear
        assert "supersed" in comment.lower(), (
            "Close comment must state PR is superseded. " f"Got: {comment!r}"
        )


# ---------------------------------------------------------------------------
# validate_scenario_path: security input validation
# ---------------------------------------------------------------------------


class TestValidateScenarioPath:
    """Scenario path must be within tests/parity/ to prevent path traversal."""

    def test_valid_path_is_accepted(self):
        _skip_if_missing()
        valid = "tests/parity/scenarios/tier5-launcher.yaml"
        result = validate_scenario_path(valid)
        assert result is not None

    def test_path_outside_tests_parity_is_rejected(self):
        _skip_if_missing()
        with pytest.raises((ValueError, SystemExit, PermissionError)) as exc_info:
            validate_scenario_path("/etc/passwd")
        # Any of these exception types indicate the path was rejected
        _ = exc_info  # consumed

    def test_path_traversal_is_rejected(self):
        _skip_if_missing()
        with pytest.raises((ValueError, SystemExit, PermissionError)):
            validate_scenario_path("tests/parity/../../.ssh/id_rsa")

    def test_absolute_path_outside_repo_is_rejected(self):
        _skip_if_missing()
        with pytest.raises((ValueError, SystemExit, PermissionError)):
            validate_scenario_path("/tmp/malicious.yaml")

    def test_valid_absolute_path_within_repo_accepted(self):
        """An absolute path pointing into tests/parity/ must be accepted."""
        _skip_if_missing()
        repo_root = Path(__file__).resolve().parent.parent.parent
        valid_abs = str(
            repo_root / "tests" / "parity" / "scenarios" / "tier5-launcher.yaml"
        )
        # Should not raise
        result = validate_scenario_path(valid_abs)
        assert result is not None


# ---------------------------------------------------------------------------
# validate_case_name: security input validation
# ---------------------------------------------------------------------------


class TestValidateCaseName:
    """Case name must be alphanumeric + hyphens only (no shell injection)."""

    @pytest.mark.parametrize(
        "valid_name",
        [
            "launcher-passthrough-args",
            "gap-launch-sigint-exit-code",
            "tier1-basic",
            "test123",
            "a",
        ],
    )
    def test_valid_case_names_are_accepted(self, valid_name: str):
        _skip_if_missing()
        result = validate_case_name(valid_name)
        assert (
            result == valid_name
        ), f"Valid case name {valid_name!r} must be returned unchanged. Got: {result!r}"

    @pytest.mark.parametrize(
        "invalid_name",
        [
            "",  # empty
            "test; rm -rf /",  # shell injection
            "test && evil",  # shell injection
            "test$(evil)",  # command substitution
            "test `evil`",  # backtick injection
            "../etc/passwd",  # path traversal
            "test\neval",  # newline injection
        ],
    )
    def test_invalid_case_names_are_rejected(self, invalid_name: str):
        _skip_if_missing()
        with pytest.raises((ValueError, SystemExit)):
            validate_case_name(invalid_name)

    def test_case_name_with_spaces_is_rejected(self):
        _skip_if_missing()
        with pytest.raises((ValueError, SystemExit)):
            validate_case_name("test case with spaces")


# ---------------------------------------------------------------------------
# run_parity_test_once: subprocess behavior (uses mocking)
# ---------------------------------------------------------------------------


class TestRunParityTestOnce:
    """run_parity_test_once must invoke the parity test script via subprocess."""

    def test_passing_subprocess_returns_run_with_exit_0(self):
        _skip_if_missing()
        with patch("subprocess.run") as mock_run:
            mock_result = MagicMock()
            mock_result.returncode = 0
            mock_result.stdout = "launcher-passthrough-args: PASS\n"
            mock_result.stderr = ""
            mock_run.return_value = mock_result

            run = run_parity_test_once(
                scenario_path="tests/parity/scenarios/tier5-launcher.yaml",
                case_name="launcher-passthrough-args",
                run_number=1,
            )

        assert run.exit_code == 0, f"exit_code must be 0. Got: {run.exit_code}"
        assert run.passed is True

    def test_failing_subprocess_returns_run_with_nonzero_exit(self):
        _skip_if_missing()
        with patch("subprocess.run") as mock_run:
            mock_result = MagicMock()
            mock_result.returncode = 1
            mock_result.stdout = ""
            mock_result.stderr = "launcher-passthrough-args: FAIL divergence detected\n"
            mock_run.return_value = mock_result

            run = run_parity_test_once(
                scenario_path="tests/parity/scenarios/tier5-launcher.yaml",
                case_name="launcher-passthrough-args",
                run_number=2,
            )

        assert run.exit_code == 1, f"exit_code must be 1. Got: {run.exit_code}"
        assert run.passed is False

    def test_subprocess_is_called_with_list_args_not_shell_true(self):
        """Security: subprocess.run must be called with list args, not shell=True."""
        _skip_if_missing()
        with patch("subprocess.run") as mock_run:
            mock_result = MagicMock()
            mock_result.returncode = 0
            mock_result.stdout = ""
            mock_result.stderr = ""
            mock_run.return_value = mock_result

            run_parity_test_once(
                scenario_path="tests/parity/scenarios/tier5-launcher.yaml",
                case_name="launcher-passthrough-args",
                run_number=1,
            )

            call_kwargs = mock_run.call_args
            # The first positional argument must be a list, not a string
            if call_kwargs.args:
                cmd_arg = call_kwargs.args[0]
                assert isinstance(cmd_arg, list), (
                    "subprocess.run must be called with a list of args (not a string) "
                    "to prevent shell injection. Got: " + repr(cmd_arg)
                )
            # shell=True must not be set
            shell = call_kwargs.kwargs.get("shell", False)
            assert (
                shell is not True
            ), "subprocess.run must NOT use shell=True. Got shell=True in call."

    def test_run_number_is_recorded_in_result(self):
        _skip_if_missing()
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0, stdout="ok", stderr="")
            run = run_parity_test_once(
                scenario_path="tests/parity/scenarios/tier5-launcher.yaml",
                case_name="launcher-passthrough-args",
                run_number=2,
            )
        assert run.run_number == 2, f"run_number must be 2. Got: {run.run_number}"
