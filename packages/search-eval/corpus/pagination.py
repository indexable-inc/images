"""Opaque cursor pagination over an ordered result set."""

import base64

# Maximum page size a caller may request; larger requests are clamped.
MAX_PAGE_SIZE = 200
# Page size used when the caller does not specify one.
DEFAULT_PAGE_SIZE = 50


def encode_cursor(last_id: int) -> str:
    """Encode the last seen id as an opaque base64 cursor token."""
    return base64.urlsafe_b64encode(f"after:{last_id}".encode()).decode("ascii")


def decode_cursor(cursor: str) -> int:
    """Recover the last seen id from an opaque cursor token."""
    raw = base64.urlsafe_b64decode(cursor.encode("ascii")).decode("utf-8")
    return int(raw.removeprefix("after:"))
