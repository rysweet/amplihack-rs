"""
Retry utility: exponential back-off for transient failures in external calls.

Exported API:
  def retry_with_backoff(
      func,
      *args,
      max_attempts: int = 3,
      initial_delay: float = 1.0,
      max_delay: float = 60.0,
      backoff_factor: float = 2.0,
      exceptions: tuple = (Exception,),
      **kwargs,
  ) -> Any

  def wait_for_condition(
      probe,
      *args,
      max_attempts: int = 10,
      interval: float = 5.0,
      exceptions: tuple = (Exception,),
      **kwargs,
  ) -> bool
"""

from __future__ import annotations

import time
from typing import Any, Callable, Tuple, Type


def retry_with_backoff(
    func: Callable,
    *args: Any,
    max_attempts: int = 3,
    initial_delay: float = 1.0,
    max_delay: float = 60.0,
    backoff_factor: float = 2.0,
    exceptions: Tuple[Type[Exception], ...] = (Exception,),
    **kwargs: Any,
) -> Any:
    """Call *func* with retry and exponential back-off on transient failures.

    Each attempt waits ``initial_delay * backoff_factor ** attempt`` seconds
    (capped at ``max_delay``) before retrying.

    Args:
        func: Callable to invoke.
        *args: Positional arguments forwarded to *func*.
        max_attempts: Maximum number of attempts (default 3).
        initial_delay: Seconds to wait before the first retry (default 1.0).
        max_delay: Maximum wait between retries in seconds (default 60.0).
        backoff_factor: Multiplier applied to delay on each failure (default 2.0).
        exceptions: Exception types that trigger a retry. Any other exception
            propagates immediately. Default is ``(Exception,)``.
        **kwargs: Keyword arguments forwarded to *func*.

    Returns:
        The return value of *func* on the first successful call.

    Raises:
        The last exception raised by *func* after all attempts are exhausted.
        Any exception NOT in *exceptions* is re-raised immediately.
    """
    if max_attempts < 1:
        raise ValueError(f"max_attempts must be >= 1, got {max_attempts}")
    if initial_delay < 0:
        raise ValueError(f"initial_delay must be >= 0, got {initial_delay}")
    if backoff_factor < 1:
        raise ValueError(f"backoff_factor must be >= 1, got {backoff_factor}")

    last_exc: Exception | None = None
    delay = initial_delay

    for attempt in range(max_attempts):
        try:
            return func(*args, **kwargs)
        except exceptions as exc:  # type: ignore[misc]
            last_exc = exc
            if attempt < max_attempts - 1:
                time.sleep(min(delay, max_delay))
                delay *= backoff_factor
        except Exception:
            # Non-retryable exception — propagate immediately
            raise

    assert last_exc is not None  # for type narrowing
    raise last_exc


def wait_for_condition(
    probe: Callable[..., bool],
    *args: Any,
    max_attempts: int = 10,
    interval: float = 5.0,
    exceptions: Tuple[Type[Exception], ...] = (Exception,),
    **kwargs: Any,
) -> bool:
    """Poll *probe* until it returns True or attempts are exhausted.

    Useful for waiting on external resources (e.g., SSH becoming available
    on a freshly created VM) without a busy loop.

    Args:
        probe: Callable that returns ``True`` when the condition is met,
               ``False`` (or raises an *exceptions* type) when not yet ready.
        *args: Positional arguments forwarded to *probe*.
        max_attempts: Maximum number of poll attempts (default 10).
        interval: Seconds between polls (default 5.0).
        exceptions: Exception types from *probe* that are treated as
            "not ready yet" (retry). Other exceptions propagate immediately.
        **kwargs: Keyword arguments forwarded to *probe*.

    Returns:
        ``True`` if the condition was met within *max_attempts*.
        ``False`` if all attempts were exhausted without success.
    """
    if max_attempts < 1:
        raise ValueError(f"max_attempts must be >= 1, got {max_attempts}")
    if interval < 0:
        raise ValueError(f"interval must be >= 0, got {interval}")

    for attempt in range(max_attempts):
        try:
            if probe(*args, **kwargs):
                return True
        except exceptions:  # type: ignore[misc]
            pass  # treat as "not ready yet"
        except Exception:
            raise  # non-retryable

        if attempt < max_attempts - 1:
            time.sleep(interval)

    return False
