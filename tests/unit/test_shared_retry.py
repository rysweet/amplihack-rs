"""
Unit tests for scripts/shared/retry.py.

Tests cover:
  - retry_with_backoff: success on first attempt, success after failures,
    exhausted attempts, non-retryable exception propagation, parameter
    validation, no sleep on final attempt, correct backoff sequence.
  - wait_for_condition: condition met immediately, condition met on last attempt,
    never met, probe exception treated as not-ready, non-retryable propagation,
    parameter validation.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path
from typing import List
from unittest.mock import patch

import pytest

# ---------------------------------------------------------------------------
# Path setup
# ---------------------------------------------------------------------------
REPO_ROOT = Path(__file__).resolve().parent.parent.parent
SCRIPTS_SHARED_DIR = REPO_ROOT / "scripts" / "shared"
for _p in [str(REPO_ROOT), str(SCRIPTS_SHARED_DIR)]:
    if _p not in sys.path:
        sys.path.insert(0, _p)

from retry import retry_with_backoff, wait_for_condition  # noqa: E402


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


class _CallCounter:
    """Callable that fails the first N calls then succeeds."""

    def __init__(self, fail_count: int = 0, return_value: object = "ok"):
        self.calls: int = 0
        self._fail_count = fail_count
        self._return_value = return_value

    def __call__(self, *args, **kwargs):
        self.calls += 1
        if self.calls <= self._fail_count:
            raise OSError(f"Simulated transient failure #{self.calls}")
        return self._return_value


class _BoolCounter:
    """Callable that returns False the first N times then True."""

    def __init__(self, false_count: int = 0):
        self.calls: int = 0
        self._false_count = false_count

    def __call__(self, *args, **kwargs) -> bool:
        self.calls += 1
        return self.calls > self._false_count


# ===========================================================================
# retry_with_backoff tests
# ===========================================================================


class TestRetryWithBackoffSuccess:
    def test_succeeds_on_first_attempt(self):
        func = _CallCounter(fail_count=0, return_value=42)
        with patch("retry.time.sleep") as mock_sleep:
            result = retry_with_backoff(func, max_attempts=3, initial_delay=1.0)
        assert result == 42
        assert func.calls == 1
        mock_sleep.assert_not_called()

    def test_succeeds_after_one_failure(self):
        func = _CallCounter(fail_count=1, return_value="done")
        with patch("retry.time.sleep"):
            result = retry_with_backoff(
                func,
                max_attempts=3,
                initial_delay=1.0,
                exceptions=(OSError,),
            )
        assert result == "done"
        assert func.calls == 2

    def test_succeeds_after_two_failures(self):
        func = _CallCounter(fail_count=2, return_value="yes")
        with patch("retry.time.sleep"):
            result = retry_with_backoff(
                func,
                max_attempts=3,
                initial_delay=0.1,
                exceptions=(OSError,),
            )
        assert result == "yes"
        assert func.calls == 3

    def test_forwards_args_and_kwargs(self):
        calls: list = []

        def _f(a, b, *, key="default"):
            calls.append((a, b, key))
            return "forwarded"

        with patch("retry.time.sleep"):
            result = retry_with_backoff(_f, 1, 2, key="custom", max_attempts=1)
        assert result == "forwarded"
        assert calls == [(1, 2, "custom")]


class TestRetryWithBackoffExhausted:
    def test_raises_last_exception_when_all_attempts_fail(self):
        func = _CallCounter(fail_count=99)
        with patch("retry.time.sleep"):
            with pytest.raises(OSError, match="Simulated transient failure"):
                retry_with_backoff(
                    func,
                    max_attempts=3,
                    initial_delay=0.1,
                    exceptions=(OSError,),
                )
        assert func.calls == 3

    def test_single_attempt_raises_immediately(self):
        func = _CallCounter(fail_count=99)
        with patch("retry.time.sleep") as mock_sleep:
            with pytest.raises(OSError):
                retry_with_backoff(
                    func,
                    max_attempts=1,
                    initial_delay=1.0,
                    exceptions=(OSError,),
                )
        mock_sleep.assert_not_called()
        assert func.calls == 1


class TestRetryWithBackoffNonRetryable:
    def test_non_retryable_exception_propagates_immediately(self):
        """ValueError is not in exceptions=(OSError,) → must propagate on attempt 1."""
        call_count = 0

        def _f():
            nonlocal call_count
            call_count += 1
            raise ValueError("not retryable")

        with patch("retry.time.sleep"):
            with pytest.raises(ValueError, match="not retryable"):
                retry_with_backoff(_f, max_attempts=5, exceptions=(OSError,))

        assert call_count == 1  # should not retry

    def test_non_retryable_does_not_sleep(self):
        def _f():
            raise KeyError("bail")

        with patch("retry.time.sleep") as mock_sleep:
            with pytest.raises(KeyError):
                retry_with_backoff(_f, max_attempts=5, exceptions=(OSError,))
        mock_sleep.assert_not_called()


class TestRetryWithBackoffSleepSequence:
    def test_sleep_durations_use_exponential_backoff(self):
        """Verify sleep(1), sleep(2), sleep(4) for backoff_factor=2, initial_delay=1."""
        func = _CallCounter(fail_count=99)
        sleep_calls: List[float] = []

        def _fake_sleep(t: float) -> None:
            sleep_calls.append(t)

        with patch("retry.time.sleep", side_effect=_fake_sleep):
            with pytest.raises(OSError):
                retry_with_backoff(
                    func,
                    max_attempts=4,
                    initial_delay=1.0,
                    max_delay=100.0,
                    backoff_factor=2.0,
                    exceptions=(OSError,),
                )

        # 4 attempts → 3 sleeps
        assert len(sleep_calls) == 3
        assert sleep_calls == [1.0, 2.0, 4.0]

    def test_sleep_capped_at_max_delay(self):
        func = _CallCounter(fail_count=99)
        sleep_calls: List[float] = []

        def _fake_sleep(t: float) -> None:
            sleep_calls.append(t)

        with patch("retry.time.sleep", side_effect=_fake_sleep):
            with pytest.raises(OSError):
                retry_with_backoff(
                    func,
                    max_attempts=5,
                    initial_delay=10.0,
                    max_delay=15.0,
                    backoff_factor=2.0,
                    exceptions=(OSError,),
                )

        for duration in sleep_calls:
            assert duration <= 15.0, f"sleep({duration}) exceeds max_delay=15.0"

    def test_no_sleep_after_last_attempt(self):
        """Sleep should NOT occur after the final failing attempt."""
        func = _CallCounter(fail_count=99)
        sleep_calls: List[float] = []

        def _fake_sleep(t: float) -> None:
            sleep_calls.append(t)

        with patch("retry.time.sleep", side_effect=_fake_sleep):
            with pytest.raises(OSError):
                retry_with_backoff(
                    func,
                    max_attempts=3,
                    initial_delay=1.0,
                    exceptions=(OSError,),
                )

        # 3 attempts → 2 sleeps (not 3)
        assert len(sleep_calls) == 2


class TestRetryWithBackoffParameterValidation:
    def test_max_attempts_zero_raises(self):
        with pytest.raises(ValueError, match="max_attempts"):
            retry_with_backoff(lambda: None, max_attempts=0)

    def test_negative_max_attempts_raises(self):
        with pytest.raises(ValueError, match="max_attempts"):
            retry_with_backoff(lambda: None, max_attempts=-1)

    def test_negative_initial_delay_raises(self):
        with pytest.raises(ValueError, match="initial_delay"):
            retry_with_backoff(lambda: None, max_attempts=1, initial_delay=-0.1)

    def test_backoff_factor_less_than_one_raises(self):
        with pytest.raises(ValueError, match="backoff_factor"):
            retry_with_backoff(lambda: None, max_attempts=1, backoff_factor=0.5)

    def test_zero_initial_delay_is_valid(self):
        """initial_delay=0 means retry immediately — should not raise."""
        func = _CallCounter(fail_count=1, return_value="immediate")
        with patch("retry.time.sleep"):
            result = retry_with_backoff(
                func,
                max_attempts=2,
                initial_delay=0.0,
                exceptions=(OSError,),
            )
        assert result == "immediate"


class TestRetryWithBackoffSubprocessTimeout:
    """Verify subprocess.TimeoutExpired is retryable (common VM SSH use case)."""

    def test_retries_on_timeout_expired(self):
        call_count = 0

        def _ssh_call():
            nonlocal call_count
            call_count += 1
            if call_count < 3:
                raise subprocess.TimeoutExpired(cmd="ssh", timeout=30)
            return (0, "ok", "")

        with patch("retry.time.sleep"):
            result = retry_with_backoff(
                _ssh_call,
                max_attempts=3,
                exceptions=(subprocess.TimeoutExpired,),
            )
        assert result == (0, "ok", "")
        assert call_count == 3


# ===========================================================================
# wait_for_condition tests
# ===========================================================================


class TestWaitForConditionSuccess:
    def test_returns_true_when_ready_immediately(self):
        probe = _BoolCounter(false_count=0)
        with patch("retry.time.sleep"):
            result = wait_for_condition(probe, max_attempts=5, interval=1.0)
        assert result is True
        assert probe.calls == 1

    def test_returns_true_after_several_false_probes(self):
        probe = _BoolCounter(false_count=3)
        with patch("retry.time.sleep"):
            result = wait_for_condition(probe, max_attempts=10, interval=0.1)
        assert result is True
        assert probe.calls == 4

    def test_returns_true_on_last_allowed_attempt(self):
        probe = _BoolCounter(false_count=4)  # True on call 5
        with patch("retry.time.sleep"):
            result = wait_for_condition(probe, max_attempts=5, interval=0.1)
        assert result is True
        assert probe.calls == 5


class TestWaitForConditionFailure:
    def test_returns_false_when_never_ready(self):
        probe = _BoolCounter(false_count=99)
        with patch("retry.time.sleep"):
            result = wait_for_condition(probe, max_attempts=3, interval=0.1)
        assert result is False
        assert probe.calls == 3

    def test_returns_false_when_probe_always_raises(self):
        def _probe():
            raise OSError("connection refused")

        with patch("retry.time.sleep"):
            result = wait_for_condition(
                _probe,
                max_attempts=3,
                interval=0.1,
                exceptions=(OSError,),
            )
        assert result is False

    def test_probe_exception_treated_as_not_ready(self):
        """OSError from probe should not propagate; counts as False."""
        call_count = 0

        def _probe():
            nonlocal call_count
            call_count += 1
            if call_count < 3:
                raise OSError("ssh: connect to host ... port 22: Connection refused")
            return True

        with patch("retry.time.sleep"):
            result = wait_for_condition(
                _probe,
                max_attempts=5,
                interval=0.1,
                exceptions=(OSError,),
            )
        assert result is True
        assert call_count == 3


class TestWaitForConditionNonRetryable:
    def test_non_retryable_probe_exception_propagates(self):
        def _probe():
            raise RuntimeError("unexpected error")

        with patch("retry.time.sleep"):
            with pytest.raises(RuntimeError, match="unexpected error"):
                wait_for_condition(
                    _probe,
                    max_attempts=5,
                    interval=0.1,
                    exceptions=(OSError,),
                )


class TestWaitForConditionSleepBehavior:
    def test_sleeps_between_polls(self):
        probe = _BoolCounter(false_count=2)  # True on call 3
        sleep_calls: List[float] = []

        def _fake_sleep(t: float) -> None:
            sleep_calls.append(t)

        with patch("retry.time.sleep", side_effect=_fake_sleep):
            result = wait_for_condition(probe, max_attempts=5, interval=7.0)
        assert result is True
        # 2 failed probes → 2 sleeps before success
        assert len(sleep_calls) == 2
        assert all(t == 7.0 for t in sleep_calls)

    def test_no_sleep_after_success(self):
        probe = _BoolCounter(false_count=0)
        with patch("retry.time.sleep") as mock_sleep:
            wait_for_condition(probe, max_attempts=5, interval=5.0)
        mock_sleep.assert_not_called()

    def test_no_sleep_after_last_failing_attempt(self):
        """When all attempts fail, sleep should NOT occur after the last one."""
        probe = _BoolCounter(false_count=99)
        sleep_calls: List[float] = []

        def _fake_sleep(t: float) -> None:
            sleep_calls.append(t)

        with patch("retry.time.sleep", side_effect=_fake_sleep):
            wait_for_condition(probe, max_attempts=3, interval=2.0)

        # 3 attempts → 2 sleeps (not 3)
        assert len(sleep_calls) == 2


class TestWaitForConditionParameterValidation:
    def test_max_attempts_zero_raises(self):
        with pytest.raises(ValueError, match="max_attempts"):
            wait_for_condition(lambda: True, max_attempts=0)

    def test_negative_max_attempts_raises(self):
        with pytest.raises(ValueError, match="max_attempts"):
            wait_for_condition(lambda: True, max_attempts=-1)

    def test_negative_interval_raises(self):
        with pytest.raises(ValueError, match="interval"):
            wait_for_condition(lambda: True, max_attempts=1, interval=-1.0)

    def test_zero_interval_is_valid(self):
        """interval=0 means poll without pause — should not raise."""
        with patch("retry.time.sleep"):
            result = wait_for_condition(lambda: True, max_attempts=1, interval=0.0)
        assert result is True
