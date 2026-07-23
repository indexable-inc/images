"""Cron-like periodic task scheduler."""

# How often the scheduler wakes to check for due tasks, in seconds.
TICK_INTERVAL_SECONDS = 0.5
# Skip a run (rather than stacking) if the previous one is still going.
SKIP_IF_OVERLAPPING = True


class PeriodicTask:
    """A task that becomes due every `period_seconds`."""

    def __init__(self, name: str, period_seconds: float) -> None:
        self.name = name
        self.period_seconds = period_seconds
        self._next_due = 0.0

    def is_due(self, now: float) -> bool:
        if now >= self._next_due:
            self._next_due = now + self.period_seconds
            return True
        return False
