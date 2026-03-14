"""
WS2 VM Round Trip Test.

Creates an Azure Linux VM, runs install/uninstall/reinstall cycle via SSH,
and validates the settings.json is in a clean state after each uninstall.

Uses try/finally to guarantee VM cleanup regardless of test outcome.

Exported API:
  def run_round_trip(subscription: str, resource_group: str, location: str) -> dict
"""

from __future__ import annotations

import json
import subprocess
import sys
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

_SCRIPT_DIR = Path(__file__).resolve().parent
_SHARED_DIR = _SCRIPT_DIR / "shared"
for _p in (str(_SCRIPT_DIR), str(_SHARED_DIR)):
    if _p not in sys.path:
        sys.path.insert(0, _p)

from ws2_settings_inspector import InspectionResult, inspect_settings_string  # noqa: E402
from retry import wait_for_condition, retry_with_backoff  # noqa: E402

# SSH probe: number of attempts and interval before giving up waiting for a
# freshly created VM to become reachable.
_SSH_READY_MAX_ATTEMPTS = 12  # 12 × 10s = 2 min
_SSH_READY_INTERVAL_S = 10.0

# Retry parameters for individual SSH commands (transient network glitches).
_SSH_CMD_MAX_ATTEMPTS = 3
_SSH_CMD_INITIAL_DELAY_S = 5.0


@dataclass
class StepResult:
    """Result of a single step in the round-trip test."""

    name: str
    passed: bool
    output: str
    error: str = ""


@dataclass
class RoundTripResult:
    """Aggregated result of the full VM round-trip test."""

    vm_name: str
    steps: list = field(default_factory=list)
    final_inspection: Optional[InspectionResult] = None

    @property
    def passed(self) -> bool:
        return all(s.passed for s in self.steps)


def _run_ssh(vm_ip: str, command: str, ssh_key: str) -> tuple[int, str, str]:
    """Run a command on a remote VM via SSH.

    Args:
        vm_ip: The IP address of the VM.
        command: Shell command to run remotely.
        ssh_key: Path to the SSH private key file.

    Returns:
        Tuple of (exit_code, stdout, stderr).
    """
    result = subprocess.run(
        [
            "ssh",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=30",
            "-i",
            ssh_key,
            f"azureuser@{vm_ip}",
            command,
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    return result.returncode, result.stdout, result.stderr


def _ssh_is_ready(vm_ip: str, ssh_key: str) -> bool:
    """Probe whether the VM's SSH daemon is accepting connections.

    Runs a no-op ``true`` command; returns True on exit code 0 and False for
    any non-zero exit code or subprocess error.  Intended as a *probe* for
    :func:`wait_for_condition`.

    Args:
        vm_ip: The IP address of the VM.
        ssh_key: Path to the SSH private key file.

    Returns:
        True if SSH is ready, False otherwise.
    """
    try:
        exit_code, _, _ = _run_ssh(vm_ip, "true", ssh_key)
        return exit_code == 0
    except (subprocess.TimeoutExpired, OSError):
        return False


def _wait_for_ssh(vm_ip: str, ssh_key: str) -> bool:
    """Block until SSH is ready on the VM or timeout is reached.

    Args:
        vm_ip: The IP address of the VM.
        ssh_key: Path to the SSH private key file.

    Returns:
        True if SSH became ready within the allowed attempts, False otherwise.
    """
    return wait_for_condition(
        _ssh_is_ready,
        vm_ip,
        ssh_key,
        max_attempts=_SSH_READY_MAX_ATTEMPTS,
        interval=_SSH_READY_INTERVAL_S,
    )


def _run_ssh_with_retry(
    vm_ip: str,
    command: str,
    ssh_key: str,
) -> tuple[int, str, str]:
    """Run an SSH command with retry on :class:`subprocess.TimeoutExpired`.

    Retries up to :data:`_SSH_CMD_MAX_ATTEMPTS` times with exponential
    back-off starting at :data:`_SSH_CMD_INITIAL_DELAY_S` seconds.  Only
    :class:`subprocess.TimeoutExpired` and :class:`OSError` are treated as
    transient; non-zero exit codes are returned as-is without retry (the
    test logic decides pass/fail based on exit code).

    Args:
        vm_ip: The IP address of the VM.
        command: Shell command to run remotely.
        ssh_key: Path to the SSH private key file.

    Returns:
        Tuple of (exit_code, stdout, stderr).
    """
    return retry_with_backoff(
        _run_ssh,
        vm_ip,
        command,
        ssh_key,
        max_attempts=_SSH_CMD_MAX_ATTEMPTS,
        initial_delay=_SSH_CMD_INITIAL_DELAY_S,
        exceptions=(subprocess.TimeoutExpired, OSError),
    )


def _create_vm(resource_group: str, vm_name: str, location: str) -> str:
    """Create an Azure Linux VM using the az CLI.

    Args:
        resource_group: Azure resource group name.
        vm_name: Name for the new VM.
        location: Azure region (e.g. 'eastus').

    Returns:
        The public IP address of the created VM.

    Raises:
        subprocess.CalledProcessError: If the az CLI command fails.
    """
    result = subprocess.run(
        [
            "az",
            "vm",
            "create",
            "--resource-group",
            resource_group,
            "--name",
            vm_name,
            "--image",
            "Ubuntu2204",
            "--location",
            location,
            "--generate-ssh-keys",
            "--output",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    data = json.loads(result.stdout)
    return data["publicIpAddress"]


def _delete_vm(resource_group: str, vm_name: str) -> None:
    """Delete an Azure VM and its associated resources.

    Args:
        resource_group: Azure resource group name.
        vm_name: Name of the VM to delete.
    """
    subprocess.run(
        [
            "az",
            "vm",
            "delete",
            "--resource-group",
            resource_group,
            "--name",
            vm_name,
            "--yes",
            "--output",
            "none",
        ],
        check=False,  # Don't raise on cleanup failure
    )


def run_round_trip(
    subscription: str,
    resource_group: str,
    location: str = "eastus",
    ssh_key: str = "~/.ssh/id_rsa",
) -> RoundTripResult:
    """Run the full install/uninstall/reinstall cycle on a fresh VM.

    Creates a VM, runs the cycle, validates settings.json cleanliness,
    and always deletes the VM (try/finally).

    Args:
        subscription: Azure subscription ID.
        resource_group: Azure resource group to use.
        location: Azure region for the VM.
        ssh_key: Path to the SSH private key for VM access.

    Returns:
        RoundTripResult with per-step pass/fail and the final inspection.
    """
    vm_name = f"amplihack-test-{uuid.uuid4().hex[:8]}"
    result = RoundTripResult(vm_name=vm_name)

    try:
        # Step 1: Create VM
        vm_ip = _create_vm(resource_group, vm_name, location)
        result.steps.append(
            StepResult(
                name="create_vm",
                passed=True,
                output=f"VM {vm_name} created at {vm_ip}",
            )
        )

        # Step 1b: Wait for SSH to become ready (VM boot takes ~60 s)
        ssh_ready = _wait_for_ssh(vm_ip, ssh_key)
        result.steps.append(
            StepResult(
                name="wait_for_ssh",
                passed=ssh_ready,
                output="SSH ready" if ssh_ready else "SSH did not become ready in time",
            )
        )
        if not ssh_ready:
            # Cannot proceed without SSH — short-circuit to cleanup
            return result

        # Step 2: Install amplihack
        exit_code, stdout, stderr = _run_ssh_with_retry(
            vm_ip,
            "curl -fsSL https://raw.githubusercontent.com/rysweet/amplihack-rs/main/install.sh | bash",
            ssh_key,
        )
        result.steps.append(
            StepResult(
                name="install",
                passed=exit_code == 0,
                output=stdout,
                error=stderr,
            )
        )

        # Step 3: Uninstall amplihack
        exit_code, stdout, stderr = _run_ssh_with_retry(
            vm_ip,
            "~/.amplihack/bin/amplihack uninstall",
            ssh_key,
        )
        result.steps.append(
            StepResult(
                name="uninstall",
                passed=exit_code == 0,
                output=stdout,
                error=stderr,
            )
        )

        # Step 4: Retrieve settings.json and check it is clean
        exit_code, stdout, stderr = _run_ssh_with_retry(
            vm_ip,
            "cat ~/.claude/settings.json 2>/dev/null || echo '{}'",
            ssh_key,
        )
        # Parse and inspect the content directly — no temp file needed.
        inspection = inspect_settings_string(stdout)
        result.final_inspection = inspection

        result.steps.append(
            StepResult(
                name="check_clean_after_uninstall",
                passed=inspection.is_clean,
                output=f"is_clean={inspection.is_clean}, issues={inspection.issues}",
            )
        )

        # Step 5: Reinstall
        exit_code, stdout, stderr = _run_ssh_with_retry(
            vm_ip,
            "curl -fsSL https://raw.githubusercontent.com/rysweet/amplihack-rs/main/install.sh | bash",
            ssh_key,
        )
        result.steps.append(
            StepResult(
                name="reinstall",
                passed=exit_code == 0,
                output=stdout,
                error=stderr,
            )
        )

    finally:
        _delete_vm(resource_group, vm_name)

    return result
