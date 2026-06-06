"""Async Python bindings for Gmail and Google Calendar.

Two ``Client`` classes, one per product, share the same on-disk OAuth grant
as the ``gmail``/``gcal`` CLIs and the ``ix-google-mcp`` server. Methods
are native asyncio coroutines bridged from Rust via pyo3-async-runtimes:
``await`` them on your own event loop. Result shapes mirror the upstream
Google API JSON exactly; consult the ``google-gmail`` and
``google-calendar`` crate docs (or ``gmail --json``/``gcal --json``) for
the field layout.

Auth bootstrap is out of band, like everywhere else in the repo. Run
``gmail auth`` (or ``gcal auth``) on the host once to mint the refresh
token. ``Client()`` then reads ``GOOGLE_OAUTH_CLIENT_ID`` and
``GOOGLE_OAUTH_CLIENT_SECRET`` from the environment and
``~/.config/google/token.json`` from disk.

Example::

    import ix_google
    gmail = ix_google.gmail.Client()
    messages = await gmail.search("from:alice newer_than:7d")
    for stub in messages:
        full = await gmail.get_message(stub["id"])
        print(full["snippet"])
"""

from __future__ import annotations

from . import calendar, gmail
from ._ix_google import __version__

__all__ = ["__version__", "calendar", "gmail"]
