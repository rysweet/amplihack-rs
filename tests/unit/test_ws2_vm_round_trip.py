"""
Unit tests for scripts/ws2_vm_round_trip.py.

All Azure CLI and SSH subprocess calls are mocked so tests run locally without
Azure credentials or a live VM.

Tests cover:
  - _run_ssh: subprocess invocation, return value shape
  - _ssh_is_ready: returns True on exit 0, False on non-zero, False on timeout
  - _wait_for_ssh: delegates to wait_for_condition correctly
  - _run_ssh_with_retry: retries TimeoutExpired, propagates OSError on exhaustion
  - _create_vm: parses publicIpAddress from az output
  - _delete_vm: called with check=False, does not raise on error
  - run_round_trip: happy path steps, SSH-not-ready short-circuits, VM deleted in finally
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

# ---------------------------------------------------------------------------
# Path setup
# ---------------------------------------------------------------------------
REPO_ROOT = Path(__file__).resolve().parent.parent.parent
SCRIPTS_DIR = REPO_ROOT / "scripts"
SCRIPTS_SHARED_DIR = SCRIPTS_DIR / "shared"
for _p in [str(REPO_ROOT), str(SCRIPTS_DIR), str(SCRIPTS_SHARED_DIR)]:
    if _p not in sys.path:
        sys.path.insert(0, _p)

import ws2_vm_round_trip as vrt  # noqa: E402
from ws2_vm_round_trip import (  # noqa: E402
    _create_vm,
    _delete_vm,
    _run_ssh,
    _run_ssh_with_retry,
    _ssh_is_ready,
    _wait_for_ssh,
    run_round_trip,
)


# ===========================================================================
# _run_ssh tests
# ===========================================================================


class TestRunSsh:
    def test_returns_tuple_of_returncode_stdout_stderr(self):
        mock_result = MagicMock()
        mock_result.returncode = 0
        mock_result.stdout = "hello\n"
        mock_result.stderr = ""

        with patch("ws2_vm_round_trip.subprocess.run", return_value=mock_result):
            rc, out, err = _run_ssh("1.2.3.4", "echo hello", "~/.ssh/id_rsa")

        assert rc == 0
        assert out == "hello\n"
        assert err == ""

    def test_passes_strict_host_checking_no(self):
        mock_result = MagicMock(returncode=0, stdout="", stderr="")
        with patch(
            "ws2_vm_round_trip.subprocess.run", return_value=mock_result
        ) as mock_run:
            _run_ssh("10.0.0.1", "true", "/key")

        cmd = mock_run.call_args[0][0]
        assert "-o" in cmd
        idx = cmd.index("-o")
        assert cmd[idx + 1] == "StrictHostKeyChecking=no"

    def test_uses_list_args_not_shell(self):
        mock_result = MagicMock(returncode=0, stdout="", stderr="")
        with patch(
            "ws2_vm_round_trip.subprocess.run", return_value=mock_result
        ) as mock_run:
            _run_ssh("10.0.0.1", "true", "/key")

        _, kwargs = mock_run.call_args
        assert kwargs.get("shell") is not True

    def test_non_zero_exit_code_returned(self):
        mock_result = MagicMock(returncode=1, stdout="", stderr="error")
        with patch("ws2_vm_round_trip.subprocess.run", return_value=mock_result):
            rc, _, err = _run_ssh("1.2.3.4", "false", "/key")
        assert rc == 1
        assert err == "error"


# ===========================================================================
# _ssh_is_ready tests
# ===========================================================================


class TestSshIsReady:
    def test_returns_true_on_exit_zero(self):
        with patch("ws2_vm_round_trip._run_ssh", return_value=(0, "", "")):
            assert _ssh_is_ready("1.2.3.4", "/key") is True

    def test_returns_false_on_non_zero_exit(self):
        with patch(
            "ws2_vm_round_trip._run_ssh", return_value=(1, "", "connection refused")
        ):
            assert _ssh_is_ready("1.2.3.4", "/key") is False

    def test_returns_false_on_timeout_expired(self):
        with patch(
            "ws2_vm_round_trip._run_ssh",
            side_effect=subprocess.TimeoutExpired("ssh", 30),
        ):
            assert _ssh_is_ready("1.2.3.4", "/key") is False

    def test_returns_false_on_os_error(self):
        with patch(
            "ws2_vm_round_trip._run_ssh", side_effect=OSError("network unreachable")
        ):
            assert _ssh_is_ready("1.2.3.4", "/key") is False


# ===========================================================================
# _wait_for_ssh tests
# ===========================================================================


class TestWaitForSsh:
    def test_returns_true_when_ssh_ready_immediately(self):
        with patch("ws2_vm_round_trip._ssh_is_ready", return_value=True):
            with patch("ws2_vm_round_trip.wait_for_condition", return_value=True):
                result = _wait_for_ssh("1.2.3.4", "/key")
        assert result is True

    def test_delegates_to_wait_for_condition(self):
        with patch(
            "ws2_vm_round_trip.wait_for_condition", return_value=False
        ) as mock_wait:
            result = _wait_for_ssh("1.2.3.4", "/key")

        assert result is False
        mock_wait.assert_called_once()
        args, kwargs = mock_wait.call_args
        assert args[0] is vrt._ssh_is_ready
        assert args[1] == "1.2.3.4"
        assert args[2] == "/key"
        assert kwargs["max_attempts"] == vrt._SSH_READY_MAX_ATTEMPTS
        assert kwargs["interval"] == vrt._SSH_READY_INTERVAL_S


# ===========================================================================
# _run_ssh_with_retry tests
# ===========================================================================


class TestRunSshWithRetry:
    def test_returns_result_on_first_success(self):
        with patch(
            "ws2_vm_round_trip._run_ssh", return_value=(0, "ok", "")
        ) as mock_ssh:
            rc, out, err = _run_ssh_with_retry("1.2.3.4", "cmd", "/key")
        assert rc == 0
        assert out == "ok"
        assert mock_ssh.call_count == 1

    def test_retries_on_timeout_expired(self):
        call_count = 0

        def _fake_ssh(ip, cmd, key):
            nonlocal call_count
            call_count += 1
            if call_count < 3:
                raise subprocess.TimeoutExpired("ssh", 30)
            return (0, "done", "")

        with patch("ws2_vm_round_trip._run_ssh", side_effect=_fake_ssh):
            with patch("retry.time.sleep"):
                rc, out, err = _run_ssh_with_retry("1.2.3.4", "cmd", "/key")

        assert rc == 0
        assert call_count == 3

    def test_raises_after_max_retries_on_timeout(self):
        with patch(
            "ws2_vm_round_trip._run_ssh",
            side_effect=subprocess.TimeoutExpired("ssh", 30),
        ):
            with patch("retry.time.sleep"):
                with pytest.raises(subprocess.TimeoutExpired):
                    _run_ssh_with_retry("1.2.3.4", "cmd", "/key")

    def test_retries_on_os_error(self):
        call_count = 0

        def _fake_ssh(ip, cmd, key):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                raise OSError("reset by peer")
            return (0, "recovered", "")

        with patch("ws2_vm_round_trip._run_ssh", side_effect=_fake_ssh):
            with patch("retry.time.sleep"):
                rc, out, _ = _run_ssh_with_retry("1.2.3.4", "cmd", "/key")
        assert rc == 0
        assert call_count == 2


# ===========================================================================
# _create_vm tests
# ===========================================================================


class TestCreateVm:
    def _make_az_result(self, ip: str) -> MagicMock:
        payload = json.dumps({"publicIpAddress": ip, "id": "/subscriptions/x"})
        m = MagicMock()
        m.stdout = payload
        m.returncode = 0
        return m

    def test_returns_public_ip_address(self):
        with patch(
            "ws2_vm_round_trip.subprocess.run",
            return_value=self._make_az_result("20.1.2.3"),
        ):
            ip = _create_vm("rg-test", "vm-test", "eastus")
        assert ip == "20.1.2.3"

    def test_calls_az_vm_create(self):
        with patch(
            "ws2_vm_round_trip.subprocess.run",
            return_value=self._make_az_result("1.1.1.1"),
        ) as mock_run:
            _create_vm("rg", "vm", "westus")

        cmd = mock_run.call_args[0][0]
        assert cmd[0] == "az"
        assert "vm" in cmd
        assert "create" in cmd

    def test_raises_on_az_failure(self):
        with patch(
            "ws2_vm_round_trip.subprocess.run",
            side_effect=subprocess.CalledProcessError(1, "az"),
        ):
            with pytest.raises(subprocess.CalledProcessError):
                _create_vm("rg", "vm", "eastus")


# ===========================================================================
# _delete_vm tests
# ===========================================================================


class TestDeleteVm:
    def test_calls_az_vm_delete_with_yes(self):
        with patch("ws2_vm_round_trip.subprocess.run") as mock_run:
            _delete_vm("rg", "vm-test")

        cmd = mock_run.call_args[0][0]
        assert "delete" in cmd
        assert "--yes" in cmd

    def test_does_not_raise_on_nonzero_exit(self):
        """check=False means a non-zero returncode does NOT raise CalledProcessError."""
        failed_result = MagicMock()
        failed_result.returncode = 1
        # subprocess.run with check=False returns the result, never raises on non-zero
        with patch("ws2_vm_round_trip.subprocess.run", return_value=failed_result):
            # Should not raise
            _delete_vm("rg", "vm")

    def test_uses_check_false(self):
        with patch("ws2_vm_round_trip.subprocess.run") as mock_run:
            _delete_vm("rg", "vm")
        _, kwargs = mock_run.call_args
        assert kwargs.get("check") is False


# ===========================================================================
# run_round_trip integration-style tests (all I/O mocked)
# ===========================================================================


class TestRunRoundTrip:
    """Tests for the high-level orchestration function."""

    def _make_run_round_trip_patches(
        self,
        vm_ip: str = "10.0.0.5",
        ssh_ready: bool = True,
        ssh_responses: list | None = None,
        inspection_clean: bool = True,
    ):
        """Return a dict of patch targets and their mock return values."""
        from ws2_settings_inspector import InspectionResult

        if ssh_responses is None:
            # install, uninstall, cat settings, reinstall
            ssh_responses = [
                (0, "installed", ""),
                (0, "uninstalled", ""),
                (0, "{}", ""),
                (0, "reinstalled", ""),
            ]

        mock_inspection = InspectionResult(
            is_clean=inspection_clean,
            issues=[] if inspection_clean else ["stale ref"],
            preserved_hooks={},
            stale_keys=[],
        )

        return {
            "create_vm": MagicMock(return_value=vm_ip),
            "wait_for_ssh": MagicMock(return_value=ssh_ready),
            "run_ssh_with_retry": MagicMock(side_effect=ssh_responses),
            "delete_vm": MagicMock(),
            "inspection": mock_inspection,
        }

    def test_happy_path_all_steps_pass(self, tmp_path):
        mocks = self._make_run_round_trip_patches()

        with (
            patch("ws2_vm_round_trip._create_vm", mocks["create_vm"]),
            patch("ws2_vm_round_trip._wait_for_ssh", mocks["wait_for_ssh"]),
            patch("ws2_vm_round_trip._run_ssh_with_retry", mocks["run_ssh_with_retry"]),
            patch("ws2_vm_round_trip._delete_vm", mocks["delete_vm"]),
            patch(
                "ws2_vm_round_trip.inspect_settings_string",
                return_value=mocks["inspection"],
            ),
        ):
            result = run_round_trip("sub", "rg", "eastus", "/key")

        assert result.passed
        step_names = [s.name for s in result.steps]
        assert "create_vm" in step_names
        assert "wait_for_ssh" in step_names
        assert "install" in step_names
        assert "uninstall" in step_names
        assert "check_clean_after_uninstall" in step_names
        assert "reinstall" in step_names

    def test_vm_deleted_even_when_install_fails(self, tmp_path):
        mocks = self._make_run_round_trip_patches(
            ssh_responses=[
                (1, "", "install failed"),  # install fails
                (0, "", ""),
                (0, "{}", ""),
                (0, "", ""),
            ]
        )

        with (
            patch("ws2_vm_round_trip._create_vm", mocks["create_vm"]),
            patch("ws2_vm_round_trip._wait_for_ssh", mocks["wait_for_ssh"]),
            patch("ws2_vm_round_trip._run_ssh_with_retry", mocks["run_ssh_with_retry"]),
            patch("ws2_vm_round_trip._delete_vm", mocks["delete_vm"]),
            patch(
                "ws2_vm_round_trip.inspect_settings_string",
                return_value=mocks["inspection"],
            ),
        ):
            result = run_round_trip("sub", "rg", "eastus", "/key")

        mocks["delete_vm"].assert_called_once()
        install_step = next(s for s in result.steps if s.name == "install")
        assert install_step.passed is False

    def test_ssh_not_ready_short_circuits(self, tmp_path):
        mocks = self._make_run_round_trip_patches(ssh_ready=False)

        with (
            patch("ws2_vm_round_trip._create_vm", mocks["create_vm"]),
            patch("ws2_vm_round_trip._wait_for_ssh", mocks["wait_for_ssh"]),
            patch("ws2_vm_round_trip._run_ssh_with_retry", mocks["run_ssh_with_retry"]),
            patch("ws2_vm_round_trip._delete_vm", mocks["delete_vm"]),
        ):
            result = run_round_trip("sub", "rg", "eastus", "/key")

        # SSH not ready → no install/uninstall steps attempted
        step_names = [s.name for s in result.steps]
        assert "wait_for_ssh" in step_names
        wait_step = next(s for s in result.steps if s.name == "wait_for_ssh")
        assert wait_step.passed is False
        assert "install" not in step_names
        mocks["run_ssh_with_retry"].assert_not_called()

    def test_vm_deleted_even_when_create_raises(self, tmp_path):
        """If _create_vm raises CalledProcessError, VM name is still 'deleted'
        but since there's no VM, _delete_vm should still be called (it is
        safe to call with a non-existent VM due to check=False)."""

        with (
            patch(
                "ws2_vm_round_trip._create_vm",
                side_effect=subprocess.CalledProcessError(1, "az"),
            ),
            patch("ws2_vm_round_trip._delete_vm") as mock_delete,
        ):
            with pytest.raises(subprocess.CalledProcessError):
                run_round_trip("sub", "rg", "eastus", "/key")

        mock_delete.assert_called_once()

    def test_inspection_failure_marks_step_failed(self, tmp_path):
        from ws2_settings_inspector import InspectionResult

        bad_inspection = InspectionResult(
            is_clean=False,
            issues=["stale amplihack-hooks ref"],
            preserved_hooks={},
            stale_keys=["PreToolUse"],
        )
        mocks = self._make_run_round_trip_patches()

        with (
            patch("ws2_vm_round_trip._create_vm", mocks["create_vm"]),
            patch("ws2_vm_round_trip._wait_for_ssh", mocks["wait_for_ssh"]),
            patch("ws2_vm_round_trip._run_ssh_with_retry", mocks["run_ssh_with_retry"]),
            patch("ws2_vm_round_trip._delete_vm", mocks["delete_vm"]),
            patch(
                "ws2_vm_round_trip.inspect_settings_string",
                return_value=bad_inspection,
            ),
        ):
            result = run_round_trip("sub", "rg", "eastus", "/key")

        check_step = next(
            s for s in result.steps if s.name == "check_clean_after_uninstall"
        )
        assert check_step.passed is False
        assert not result.passed

    def test_result_vm_name_is_set(self):
        mocks = self._make_run_round_trip_patches()
        with (
            patch("ws2_vm_round_trip._create_vm", mocks["create_vm"]),
            patch("ws2_vm_round_trip._wait_for_ssh", mocks["wait_for_ssh"]),
            patch("ws2_vm_round_trip._run_ssh_with_retry", mocks["run_ssh_with_retry"]),
            patch("ws2_vm_round_trip._delete_vm", mocks["delete_vm"]),
            patch(
                "ws2_vm_round_trip.inspect_settings_string",
                return_value=mocks["inspection"],
            ),
        ):
            result = run_round_trip("sub", "rg")

        assert result.vm_name.startswith("amplihack-test-")
