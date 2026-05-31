"""Serialize domain events to the wire format the bus expects."""

import json

# Bumped whenever the on-the-wire event shape changes incompatibly.
SCHEMA_VERSION = 3


def encode_event(kind: str, payload: dict[str, object], occurred_at_ms: int) -> bytes:
    """Encode one event as a compact UTF-8 JSON line for the message bus."""
    envelope = {
        "v": SCHEMA_VERSION,
        "kind": kind,
        "ts_ms": occurred_at_ms,
        "data": payload,
    }
    return json.dumps(envelope, separators=(",", ":"), sort_keys=True).encode("utf-8")
