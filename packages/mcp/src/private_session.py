"""Shared privacy boundary for MCP helpers that expose personal account data."""

from __future__ import annotations

import os
import pathlib

SHARED_ENV = "IX_MCP_SHARED"


def require_private_session(
    capability: str,
    exposure: str,
    error_type: type[Exception] = RuntimeError,
) -> None:
    """Reject personal-data access from a replicated multiplayer session."""
    if os.environ.get(SHARED_ENV):
        raise error_type(
            f"{capability} is not available in a shared (multiplayer) room "
            f"({SHARED_ENV} is set), because it would expose {exposure} to "
            "everyone in the room. Use it from an incognito chat instead; "
            "its transcript and credentials stay private to you."
        )


def find_token(env_vars: tuple[str, ...], token_file: pathlib.Path) -> str | None:
    """Read the first non-empty environment or per-user file token."""
    for variable in env_vars:
        if value := os.environ.get(variable, "").strip():
            return value
    if token_file.exists() and (value := token_file.read_text().strip()):
        return value
    return None
