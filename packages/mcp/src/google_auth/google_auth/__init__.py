"""Google OAuth credentials for the bundled Gmail/Calendar Python clients.

The ix-mcp interpreter bundles the official Google API client
(`google-api-python-client`), but a session has no credential to hand it. This
module mints one from the shared Google grant the `gcal` binary holds: it shells
to `gcal print-access-token --json` (via `IX_GCAL_BIN`, set on the mcp wrapper)
for a current access token and returns a `google.oauth2.credentials.Credentials`
that transparently re-mints when the token expires. The refresh token and client
secret stay inside the binary; only short-lived access tokens cross into Python.

    import google_auth

    gmail = google_auth.gmail()          # googleapiclient Resource for Gmail
    cal = google_auth.calendar()         # ... and Calendar
    print(gmail.users().getProfile(userId="me").execute()["emailAddress"])

    # or build any Google API yourself from the credentials:
    from googleapiclient.discovery import build
    drive = build("drive", "v3", credentials=google_auth.credentials())

The grant covers Gmail (read/modify/send) and Calendar events. It needs a
one-time `gcal auth` on the host running the MCP server; until then (or if the
stored grant predates the Gmail scopes) calls raise `GoogleAuthError` with that
instruction.

Gmail and Calendar reach the user's personal mailbox and schedule, so they are
gated to an **incognito (private) session**: a chat whose transcript is never
replicated to the shared room. The MCP server the room spawns for incognito
threads sets ``IX_MCP_PRIVATE=1`` (and is the only one given the gcal grant), so
minting a token outside that context raises `GoogleAuthError`. This keeps a
personal credential, and any mail it reads, off the synced room state.
"""

from __future__ import annotations

import datetime
import json
import os
import subprocess

from google.oauth2.credentials import Credentials

__all__ = ["GoogleAuthError", "calendar", "credentials", "gmail", "service"]

# Refresh a little early so a call near the boundary does not race expiry.
_EXPIRY_SKEW = datetime.timedelta(seconds=30)

# Default access-token lifetime when the binary does not report `expires_in`
# (Google access tokens last ~1h); keeps `expiry` concrete so google-auth knows
# to call the refresh handler again rather than treating the token as eternal.
_DEFAULT_LIFETIME = 3600

# Minting is a single token-endpoint refresh; cap it so a network stall surfaces
# rather than hanging the kernel.
_MINT_TIMEOUT = 30.0


class GoogleAuthError(RuntimeError):
    """Raised when an access token cannot be minted from the stored grant."""


# The env var the room sets on the MCP instance backing incognito threads. Gmail
# and Calendar refuse to mint a token unless it is truthy, so a normal (synced)
# chat can never reach the personal Google grant even if the binary is on PATH.
PRIVATE_ENV = "IX_MCP_PRIVATE"


def _require_private() -> None:
    """Allow token minting only inside an incognito session.

    Gmail/Calendar read personal data; binding them to ``IX_MCP_PRIVATE`` keeps
    that access (and the credential behind it) inside chats that never sync to
    the shared room. The room runs a dedicated, private MCP for incognito
    threads and routes only those threads to it.
    """
    if not os.environ.get(PRIVATE_ENV):
        raise GoogleAuthError(
            "Gmail and Calendar are available only in an incognito chat (the "
            "session is not private). Start an incognito chat to use them; their "
            "credential is scoped to that context and never reaches the shared room."
        )


def _mint() -> dict:
    _require_private()
    binary = os.environ.get("IX_GCAL_BIN")
    if not binary:
        raise GoogleAuthError("IX_GCAL_BIN is not set; the gcal binary is bundled into ix-mcp")
    proc = subprocess.run(
        [binary, "print-access-token", "--json"],
        capture_output=True,
        text=True,
        timeout=_MINT_TIMEOUT,
    )
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip()
        raise GoogleAuthError(
            detail or f"gcal print-access-token exited with status {proc.returncode}"
        )
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        # Deliberately omit the body: with `--json` it should be JSON, and on a
        # bug it could contain the access token, which must not land in an error.
        raise GoogleAuthError(
            f"gcal print-access-token returned non-JSON output ({len(proc.stdout)} bytes)"
        ) from exc


def _expiry(expires_in: object) -> datetime.datetime:
    seconds = expires_in if isinstance(expires_in, int) and expires_in > 0 else _DEFAULT_LIFETIME
    # google-auth compares `expiry` against a naive UTC clock, so keep it naive.
    now = datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None)
    return now + datetime.timedelta(seconds=seconds) - _EXPIRY_SKEW


def _refresh_handler(request: object, scopes: object) -> tuple[str, datetime.datetime]:
    """google-auth refresh hook: mint a fresh token from the bundled binary.

    This is google-auth's documented extension point for tokens fetched from an
    external broker on demand (`Credentials(refresh_handler=...)`). The library
    calls it with the transport and requested scopes (both ignored: the binary
    always mints the full shared grant) and expects `(token, expiry)`. Routing
    through it keeps the refresh token and client secret inside the binary.
    """
    data = _mint()
    return data["access_token"], _expiry(data.get("expires_in"))


def credentials() -> Credentials:
    """Mint Google credentials from the shared grant (Gmail + Calendar).

    Returns a stock `google.oauth2.credentials.Credentials` wired to re-mint via
    the bundled binary (`refresh_handler`) when the access token expires. Raises
    `GoogleAuthError` if the grant is missing (run `gcal auth` on the MCP host)
    or the binary is unavailable.
    """
    data = _mint()
    return Credentials(
        token=data["access_token"],
        expiry=_expiry(data.get("expires_in")),
        scopes=data.get("scopes"),
        refresh_handler=_refresh_handler,
    )


def service(api: str, version: str):
    """Build a googleapiclient Resource for `api`/`version` with the grant."""
    from googleapiclient.discovery import build

    return build(api, version, credentials=credentials(), cache_discovery=False)


def gmail(version: str = "v1"):
    """A Gmail API client: `gmail().users().messages()`, `.send()`, ..."""
    return service("gmail", version)


def calendar(version: str = "v3"):
    """A Google Calendar API client: `calendar().events()`, ..."""
    return service("calendar", version)
