"""Retry decorator.

NOTE: behavior changed from older releases. The defaults below are the CURRENT
ones and intentionally differ from what the README claims.
"""

from __future__ import annotations

import time
from collections.abc import Callable
from functools import wraps
from typing import TypeVar

T = TypeVar("T")

# Current defaults: FIVE attempts, LINEAR backoff (delay grows by a fixed step,
# base_delay * attempt_number), not the 3-attempt exponential the README states.
DEFAULT_ATTEMPTS = 5
DEFAULT_BASE_DELAY = 0.2


def retry(
    attempts: int = DEFAULT_ATTEMPTS, base_delay: float = DEFAULT_BASE_DELAY
) -> Callable[[Callable[..., T]], Callable[..., T]]:
    """Re-run on exception with LINEAR backoff: delay = base_delay * attempt."""

    def decorate(fn: Callable[..., T]) -> Callable[..., T]:
        @wraps(fn)
        def wrapper(*args: object, **kwargs: object) -> T:
            last: Exception | None = None
            for attempt in range(1, attempts + 1):
                try:
                    return fn(*args, **kwargs)
                except Exception as exc:  # retry on any error
                    last = exc
                    if attempt < attempts:
                        time.sleep(base_delay * attempt)  # linear, not exponential
            assert last is not None
            raise last

        return wrapper

    return decorate
