"""Exponential backoff retry policy for outbound RPC calls."""

# Base delay before the first retry, in milliseconds.
BASE_DELAY_MS = 250
# Each subsequent attempt waits BASE_DELAY_MS * (BACKOFF_FACTOR ** attempt).
BACKOFF_FACTOR = 2.0
# Give up after this many total attempts.
MAX_ATTEMPTS = 5
# Cap any single sleep so a long backoff never exceeds this ceiling.
MAX_DELAY_MS = 8_000


def delay_for_attempt(attempt: int) -> float:
    """Milliseconds to sleep before `attempt` (0-based), capped at MAX_DELAY_MS."""
    raw = BASE_DELAY_MS * (BACKOFF_FACTOR**attempt)
    return min(raw, MAX_DELAY_MS)
