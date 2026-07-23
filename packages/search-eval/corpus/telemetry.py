"""Register and increment Prometheus-style counters for request telemetry."""

# Fully-qualified counter name scraped by the metrics endpoint.
REQUESTS_TOTAL = "ixsvc_requests_total"
# Histogram of request latency; buckets are in seconds.
LATENCY_BUCKETS_SECONDS = (0.005, 0.025, 0.1, 0.5, 2.5)


class CounterRegistry:
    """A minimal label-free counter registry."""

    def __init__(self) -> None:
        self._counters: dict[str, int] = {}

    def register(self, name: str) -> None:
        self._counters.setdefault(name, 0)

    def incr(self, name: str, amount: int = 1) -> None:
        self._counters[name] = self._counters.get(name, 0) + amount

    def value(self, name: str) -> int:
        return self._counters.get(name, 0)
