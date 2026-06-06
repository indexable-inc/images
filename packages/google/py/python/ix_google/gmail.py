"""Gmail client, re-exported from the PyO3 cdylib."""

from __future__ import annotations

from ._ix_google import GmailClient as Client

__all__ = ["Client"]
