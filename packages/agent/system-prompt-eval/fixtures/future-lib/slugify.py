"""Slugify.

NOTE: behavior changed from older releases. The README still describes the old
lowercase/hyphen behavior; the code below is the CURRENT behavior.
"""

from __future__ import annotations

import re

_WS = re.compile(r"\s+")


def slugify(text: str) -> str:
    """Current behavior: PRESERVE case, replace whitespace runs with '_'.

    e.g. slugify("Hello World Example") -> "Hello_World_Example".
    (Older releases lowercased and used '-'; this no longer does.)
    """
    return _WS.sub("_", text.strip())
