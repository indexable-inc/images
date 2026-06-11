"""Google for the kernel: Gmail + Calendar, with self-service sign-in.

The ix-mcp interpreter bundles the official Google API client
(`google-api-python-client`). This module wires it to the shared Google grant
the bundled `gcal` binary holds (`IX_GCAL_BIN`, set on the mcp wrapper), so a
session can read and send mail and manage a calendar with no setup file:

    import google_auth

    await google_auth.login()            # sign in: opens your browser (once)
    gmail = google_auth.gmail()          # googleapiclient Resource for Gmail
    gmail.users().messages().send(userId="me", body=...).execute()
    cal = google_auth.calendar()         # ... and Calendar

    google_auth.status()                 # {"signed_in", "email", "scopes"}
    google_auth.logout()                 # forget this machine's grant

    # or build any Google API yourself from the credentials:
    from googleapiclient.discovery import build
    drive = build("drive", "v3", credentials=google_auth.credentials())

Signing in runs the installed-app OAuth flow inside `gcal` (PKCE, a loopback
redirect, a forced-consent refresh token) and stores an offline refresh token in
a user-only file (`~/.config/google/token.json`, mode 0600). Access tokens are
minted from it on demand and re-minted transparently when they expire; the
refresh token and client secret never cross into Python. One grant covers Gmail
(read/modify/send) and Calendar events.

`login()` opens the browser on the host running the MCP server. When that is your
own machine the consent page just appears and the loopback redirect lands by
itself. On a headless host (SSH into a VM) the browser cannot reach the host's
`127.0.0.1`; sign in on that host with `gcal auth --paste` instead.

Gmail and Calendar reach the user's personal mailbox and schedule, so they are
confined to an **incognito session** -- one whose transcript is never replicated
to a shared room. Incognito is the default: a plain ix-mcp (a single-user chat,
a Claude Code session, the room's per-thread private MCP) is incognito and may
sign in and mint tokens. The exception is a **shared (multiplayer) room**: the
room marks the one MCP it shares across participants with ``IX_MCP_SHARED=1``,
and signing in or minting from there raises `GoogleAuthError`. This keeps a
personal credential, and any mail it reads, out of state that syncs to other
people.
"""

from __future__ import annotations

import asyncio
import datetime
import json
import os
import subprocess
import webbrowser

from google.oauth2.credentials import Credentials

__all__ = [
    "GoogleAuthError",
    "calendar",
    "credentials",
    "gmail",
    "login",
    "logout",
    "service",
    "status",
]

# Refresh a little early so a call near the boundary does not race expiry.
_EXPIRY_SKEW = datetime.timedelta(seconds=30)

# Default access-token lifetime when the binary does not report `expires_in`
# (Google access tokens last ~1h); keeps `expiry` concrete so google-auth knows
# to call the refresh handler again rather than treating the token as eternal.
_DEFAULT_LIFETIME = 3600

# Minting (and logout) is a single fast binary call; cap it so a network stall
# surfaces rather than hanging the kernel.
_MINT_TIMEOUT = 30.0

# How long to wait for `gcal auth --json` to emit the consent URL (it only has
# to bind a loopback socket and build the URL -- near-instant).
_LOGIN_URL_TIMEOUT = 30.0

# How long to wait for the human to finish consenting in the browser before
# giving up. Generous: a person may need to pick an account and read the screen.
_LOGIN_TIMEOUT = 300.0


class GoogleAuthError(RuntimeError):
    """Raised when Google cannot be reached for this session.

    Usually means "not signed in": call `await login()`. Also raised in a shared
    room (where personal Google access is refused) and when the MCP host is
    missing the team OAuth client id.
    """


# The env var a shared (multiplayer) room sets on the one MCP it shares across
# participants. Incognito is the default, so an unset (or empty) value means a
# token may be minted; only a truthy value marks the session shared and refuses
# minting, keeping the personal Google grant out of synced room state.
SHARED_ENV = "IX_MCP_SHARED"


def _require_incognito() -> None:
    """Allow Google access unless the session is a shared (multiplayer) room.

    Gmail/Calendar read personal data, so they are confined to incognito
    sessions -- the default for a plain ix-mcp. A shared room marks the MCP it
    replicates across participants with ``IX_MCP_SHARED``; only then is access
    refused, so a personal credential never reaches state other people can see.
    """
    if os.environ.get(SHARED_ENV):
        raise GoogleAuthError(
            "Gmail and Calendar are not available in a shared (multiplayer) room "
            "(IX_MCP_SHARED is set), because they would expose a personal mailbox "
            "to everyone in the room. Use them from an incognito chat instead; its "
            "transcript and credential stay private to you."
        )


def _binary() -> str:
    """Path to the bundled `gcal` binary, or raise if the wrapper didn't set it."""
    binary = os.environ.get("IX_GCAL_BIN")
    if not binary:
        raise GoogleAuthError("IX_GCAL_BIN is not set; the gcal binary is bundled into ix-mcp")
    return binary


def _signed_out_hint(detail: str) -> str:
    """Rewrite a `gcal` failure into the self-service next step where one exists.

    A missing/revoked/unscoped grant is fixed from here with `login()`, so say
    so. A missing OAuth client id is a host setup step `login()` cannot fix, so
    surface it unchanged.
    """
    text = detail.strip()
    low = text.lower()
    if "google_oauth_client" in low:
        return text or "the MCP host is missing the team Google OAuth client id"
    if any(
        key in low
        for key in ("no stored google token", "revoked", "missing scope", "gcal auth", "gmail auth")
    ):
        return (
            f"{text}\n\n"
            "Not signed in? Run `await google_auth.login()` to sign in (it opens your browser)."
        )
    return text or "could not reach Google"


def _mint() -> dict:
    """Mint a fresh access token + scopes from the stored grant (`gcal`)."""
    _require_incognito()
    proc = subprocess.run(
        [_binary(), "print-access-token", "--json"],
        capture_output=True,
        text=True,
        timeout=_MINT_TIMEOUT,
    )
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip()
        raise GoogleAuthError(_signed_out_hint(detail))
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


def _credentials_from(data: dict) -> Credentials:
    """Wrap an already-minted token as re-minting google-auth credentials."""
    return Credentials(
        token=data["access_token"],
        expiry=_expiry(data.get("expires_in")),
        scopes=data.get("scopes"),
        refresh_handler=_refresh_handler,
    )


def credentials() -> Credentials:
    """Mint Google credentials from the stored grant (Gmail + Calendar).

    Returns a stock `google.oauth2.credentials.Credentials` wired to re-mint via
    the bundled binary (`refresh_handler`) when the access token expires. Raises
    `GoogleAuthError` if you are not signed in -- run `await login()` first.
    """
    return _credentials_from(_mint())


def service(api: str, version: str):
    """Build a googleapiclient Resource for `api`/`version` with the grant."""
    from googleapiclient.discovery import build

    return build(api, version, credentials=credentials(), cache_discovery=False)


def gmail(version: str = "v1"):
    """A Gmail API client: read, search, and send mail.

    `gmail().users().messages().send(userId="me", body=...).execute()` to send;
    `.list()` / `.get()` to read. Run `await login()` first if not signed in.
    """
    return service("gmail", version)


def calendar(version: str = "v3"):
    """A Google Calendar API client: `calendar().events()`, ...

    Run `await login()` first if not signed in.
    """
    return service("calendar", version)


def status() -> dict:
    """Whether this session can reach Google, and as whom.

    Returns ``{"signed_in": bool, "email": str | None, "scopes": list[str]}``
    and never raises: a missing or revoked grant is reported as
    ``signed_in=False``, not an exception. Run `await login()` to sign in.
    """
    try:
        data = _mint()
    except GoogleAuthError:
        return {"signed_in": False, "email": None, "scopes": []}
    email = None
    try:
        from googleapiclient.discovery import build

        gmail_client = build("gmail", "v1", credentials=_credentials_from(data), cache_discovery=False)
        email = gmail_client.users().getProfile(userId="me").execute().get("emailAddress")
    except Exception:
        # An older grant without a Gmail scope (or a transient API error) still
        # counts as signed in; we just cannot name the account.
        pass
    return {"signed_in": True, "email": email, "scopes": data.get("scopes") or []}


def logout() -> dict:
    """Sign out: forget this machine's stored Google grant.

    Deletes the local token file so the next call needs a fresh `login()`.
    Idempotent. Returns ``{"signed_out": bool, "removed": list[str]}``. This does
    not revoke the grant at Google -- do that at
    https://myaccount.google.com/permissions
    """
    proc = subprocess.run(
        [_binary(), "logout", "--json"],
        capture_output=True,
        text=True,
        timeout=_MINT_TIMEOUT,
    )
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip()
        raise GoogleAuthError(detail or f"gcal logout exited with status {proc.returncode}")
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        return {"signed_out": True, "removed": []}


def _consent_blocking(open_browser: bool, timeout: float) -> tuple[str, dict]:
    """Drive `gcal auth --json` to completion synchronously; return (url, result).

    Runs on a worker thread (see `login`), so blocking here never stalls the
    kernel's event loop. The NDJSON contract is two lines: the consent URL, then
    (after the browser redirect lands) the result. A watchdog bounds the wait for
    the URL, and `wait(timeout=...)` bounds the wait for the human; a pipe is only
    drained to EOF once the process is dead, so an abandoned sign-in can never
    wedge this thread.
    """
    import threading

    proc = subprocess.Popen(  # noqa: S603 -- bundled gcal binary, fixed args
        [_binary(), "auth", "--json"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    # The URL is near-instant (bind a socket, build a string); a watchdog kills
    # gcal if it never arrives, so this readline cannot block forever.
    watchdog = threading.Timer(_LOGIN_URL_TIMEOUT, proc.kill)
    watchdog.start()
    try:
        url_line = proc.stdout.readline()
    finally:
        watchdog.cancel()

    if not url_line.strip():
        # gcal exited (or was killed) before emitting the URL: surface its error.
        proc.wait()
        detail = proc.stderr.read().strip()
        raise GoogleAuthError(_signed_out_hint(detail) if detail else "gcal auth did not start")

    auth_url = json.loads(url_line)["auth_url"]
    if open_browser:
        # Best effort: a headless host has no browser, and the caller still gets
        # auth_url back to open by hand.
        try:
            webbrowser.open(auth_url)
        except Exception:
            pass

    try:
        proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        raise GoogleAuthError(
            f"timed out after {timeout:.0f}s waiting for you to finish signing in. "
            f"Open {auth_url} and consent, then try again."
        )

    # The process has exited, so draining its pipes to EOF is safe and bounded.
    out_rest = proc.stdout.read()
    detail = proc.stderr.read().strip()
    if proc.returncode != 0:
        raise GoogleAuthError(_signed_out_hint(detail) if detail else "sign-in did not complete")

    for line in reversed(out_rest.splitlines()):
        if line.strip():
            return auth_url, json.loads(line)
    return auth_url, {}


async def login(*, open_browser: bool = True, timeout: float = _LOGIN_TIMEOUT) -> dict:
    """Sign in to Google: open your browser, consent, store the grant.

    Drives the installed-app OAuth flow in the bundled `gcal` binary: it prints a
    consent URL, this helper opens it in your browser, and the loopback redirect
    completes the grant. One consent covers Gmail (read/modify/send) and Calendar
    for this machine; the offline refresh token is stored user-only on disk and
    reused by every later `gmail()` / `calendar()` call, so you sign in once.

    Returns ``{"signed_in": True, "email", "scopes", "auth_url"}``. Surface
    ``auth_url`` to the user in case the browser did not open on its own.

    Raises `GoogleAuthError` in a shared room, when the host lacks the team OAuth
    client id, or if sign-in is not completed within ``timeout`` seconds. On a
    headless host (where the browser cannot reach this host's loopback), sign in
    on that host with `gcal auth --paste` instead.
    """
    _require_incognito()
    # The subprocess interaction blocks (it waits for a human in a browser), so
    # run it on a worker thread to keep the kernel's event loop responsive.
    auth_url, result = await asyncio.to_thread(_consent_blocking, open_browser, timeout)
    email = None
    try:
        email = await asyncio.to_thread(
            lambda: gmail().users().getProfile(userId="me").execute().get("emailAddress")
        )
    except Exception:
        # The grant is stored and valid; we just could not name the account.
        pass
    return {
        "signed_in": True,
        "email": email,
        "scopes": result.get("scopes") or [],
        "auth_url": auth_url,
    }
