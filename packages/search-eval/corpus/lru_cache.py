"""Fixed-size LRU cache with least-recently-used eviction."""

from collections import OrderedDict

# Maximum number of entries before the oldest is evicted.
MAX_ENTRIES = 1024


class LruCache:
    """Evicts the least-recently-used key once MAX_ENTRIES is exceeded."""

    def __init__(self) -> None:
        self._store: OrderedDict[str, bytes] = OrderedDict()

    def get(self, key: str) -> bytes | None:
        if key not in self._store:
            return None
        self._store.move_to_end(key)
        return self._store[key]

    def put(self, key: str, value: bytes) -> None:
        self._store[key] = value
        self._store.move_to_end(key)
        if len(self._store) > MAX_ENTRIES:
            self._store.popitem(last=False)
