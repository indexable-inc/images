"""Token-bucket rate limiter guarding the authentication endpoint."""

# Sustained request rate, tokens added per second.
REFILL_RATE_PER_SEC = 12
# Burst ceiling: the bucket never holds more than this many tokens.
BUCKET_CAPACITY = 40


class TokenBucket:
    """Allow a request when a token is available, else reject it."""

    def __init__(self) -> None:
        self._tokens = float(BUCKET_CAPACITY)

    def refill(self, elapsed_seconds: float) -> None:
        self._tokens = min(
            BUCKET_CAPACITY, self._tokens + elapsed_seconds * REFILL_RATE_PER_SEC
        )

    def try_acquire(self) -> bool:
        if self._tokens >= 1.0:
            self._tokens -= 1.0
            return True
        return False
