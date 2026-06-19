"""Bridge the federated TUI *resources* into this MCP server.

The ``ix`` CLI exposes the federated terminal-resource catalog over its
``resources`` subcommand (``ix resources ls/get/act --json``). This module shells
out to that CLI at runtime and maps its JSON into the MCP ``resources/list`` /
``resources/read`` surface plus a ``tui_act`` tool, so an agent (or Claude Code)
can ``@``-mention a peer's live terminal resource and drive it.

Why shell out instead of linking ``ix``: index cannot take a Nix build-dependency
on the ``ix`` binary (it would close the ix<->index dependency cycle), and the
R2/pyo3 distribution path is heavier than this needs to be. Calling ``ix`` from
``PATH`` at runtime sidesteps both -- and lets the bridge **degrade gracefully**:
when ``ix`` is not installed (or has no ``resources`` subcommand, or errors), the
list comes back empty and a read raises a clear error rather than crashing the
server.

Peers are configured with the ``IX_RESOURCE_PEERS`` environment variable: a
comma-separated list of peer URLs (each passed to ``ix`` as ``--peer <url>``).
Unset/empty means local-only for *listing* -- ``ix resources ls`` is run with no
``--peer`` flag and reports whatever the local node federates.

A read/act for a uri targets a single peer **by URL**. The ``ix`` CLI's
``--peer`` is a full endpoint ``url::Url`` (e.g. ``https://<addr>/rpc``), not a
bare host or label, so a peer cannot be inferred from the uri's host
(``ix://<host>/<name>`` -- ``<host>`` is a display label, not an endpoint, and
fails the CLI's URL parse). Instead: an explicit ``peer=`` URL is used directly;
otherwise the bridge iterates the configured peer URLs (the same list
:func:`list_resources` uses) and probes each with ``ix resources get`` until one
advertises the uri, then reads/acts against that peer. With no explicit peer and
no configured peers, a read/act raises a clear "no peer configured" error rather
than silently shelling out a bare host.

Everything here runs on the kernel's asyncio loop via
:func:`asyncio.create_subprocess_exec` -- never a blocking ``subprocess.run``,
which would freeze the one shared event loop. The CLI's JSON is parsed into typed
pydantic models at the boundary (the repo convention) so a malformed or
forward-incompatible payload fails loudly at one place instead of threading
untyped dicts through the handlers.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import shutil
from pathlib import Path
from typing import TYPE_CHECKING

from pydantic import BaseModel, ConfigDict, Field, ValidationError

if TYPE_CHECKING:
    from collections.abc import Sequence

logger = logging.getLogger(__name__)

__all__ = [
    "Ack",
    "ResourceBridgeError",
    "ResourceEntry",
    "ResourceNotFoundError",
    "ResourceSnapshot",
    "act",
    "configured_peers",
    "list_resources",
    "read_resource",
]

# JSON-RPC error code for "resource not found". The MCP resources spec reserves
# -32002 for a read of an unknown/unavailable resource; the server layer maps
# :class:`ResourceNotFoundError` onto an ``McpError`` carrying this code.
RESOURCE_NOT_FOUND = -32002

# Environment variable naming the federated peers to query, comma-separated URLs.
# Unset/empty => local-only (no ``--peer`` flag passed to ``ix``).
PEERS_ENV = "IX_RESOURCE_PEERS"

# The CLI the bridge shells out to. Overridable via ``IX_RESOURCES_BIN`` so a test
# (or a non-PATH install) can point at a specific binary; defaults to ``ix`` on
# PATH.
_IX_BIN_ENV = "IX_RESOURCES_BIN"

# Per-call timeout (seconds) so a hung/unreachable peer cannot wedge a request
# forever. The CLI has its own network timeouts; this is a backstop.
_CALL_TIMEOUT = 30.0


class ResourceBridgeError(RuntimeError):
    """A federated-resource operation failed (CLI missing, nonzero, bad JSON)."""


class ResourceNotFoundError(ResourceBridgeError):
    """The requested resource uri is unknown to the targeted peer (maps to -32002)."""


# ---------------------------------------------------------------------------
# Boundary models -- the shape of `ix resources ... --json` output.
# ---------------------------------------------------------------------------
#
# `extra="ignore"` keeps these forward-compatible: the CLI may add fields without
# breaking the bridge. Only the uri is truly required; every other field defaults
# so a sparser-than-expected payload still parses (a peer that omits `caps`, say).


class _BridgeModel(BaseModel):
    model_config = ConfigDict(extra="ignore")


class ResourceEntry(_BridgeModel):
    """One federated resource as reported by ``ix resources ls --json``.

    The ``uri`` (``ix://<host>/<name>`` by convention) is both the federated
    identity and the MCP resource uri, so a client ``@``-mentions exactly what
    ``ix`` advertises.
    """

    uri: str
    name: str = ""
    host: str = ""
    caps: list[str] = Field(default_factory=list)
    alive: bool = True
    mime: str = "text/plain"


class ResourceSnapshot(_BridgeModel):
    """A point-in-time read of a resource from ``ix resources get <uri> --json``."""

    uri: str = ""
    text: str = ""
    mime: str = "text/plain"


class Ack(_BridgeModel):
    """The acknowledgement ``ix resources act ... --json`` returns for a drive.

    Kept permissive (only the common fields are named, ``extra="ignore"`` carries
    the rest) because the Ack shape is the CLI's to define; the tool returns the
    parsed dict to the agent verbatim.
    """

    uri: str = ""
    ok: bool = True
    delivered: bool | None = None
    detail: str | None = None


def _ix_bin() -> str | None:
    """The ``ix`` executable to run, or ``None`` when it is not installed.

    Honors ``IX_RESOURCES_BIN`` (an explicit path or a name resolved on PATH) and
    otherwise looks up ``ix`` on PATH. Returning ``None`` -- rather than raising --
    is what lets :func:`list_resources` degrade to an empty list when the CLI is
    absent.
    """
    override = os.environ.get(_IX_BIN_ENV)
    candidate = override or "ix"
    # An override that is a path to an existing file is used as-is; otherwise
    # resolve it (or the default) on PATH.
    if override and os.path.sep in override:
        return override if Path(override).exists() else None
    return shutil.which(candidate)


def configured_peers() -> list[str]:
    """The peer URLs from ``IX_RESOURCE_PEERS`` (comma-separated), empty if unset.

    Whitespace around each entry is stripped and blanks dropped, so
    ``"a, , b "`` yields ``["a", "b"]`` and an unset/blank var yields ``[]``
    (local-only).
    """
    raw = os.environ.get(PEERS_ENV, "")
    return [peer.strip() for peer in raw.split(",") if peer.strip()]


async def _run_ix(args: Sequence[str]) -> tuple[int, str, str]:
    """Run ``ix <args>`` on the loop, returning ``(returncode, stdout, stderr)``.

    Raises :class:`ResourceBridgeError` only for an *execution* failure the caller
    cannot inspect (the binary vanished between the PATH check and exec, or the
    call timed out). A nonzero exit with output is returned normally so the caller
    can decide (empty-list vs. raise) per operation.
    """
    binary = _ix_bin()
    if binary is None:
        raise FileNotFoundError("ix CLI not found on PATH")
    try:
        proc = await asyncio.create_subprocess_exec(
            binary,
            *args,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
    except FileNotFoundError:
        # Raced away after the which() check, or PATH entry is not executable.
        raise
    try:
        stdout_b, stderr_b = await asyncio.wait_for(proc.communicate(), _CALL_TIMEOUT)
    except TimeoutError as exc:
        # The child may have exited in the race between the timeout firing and the
        # kill, so a missing process is not an error here -- only the timeout is.
        with contextlib.suppress(ProcessLookupError):
            proc.kill()
        await proc.wait()
        raise ResourceBridgeError(f"ix {' '.join(args)} timed out after {_CALL_TIMEOUT:g}s") from exc
    return proc.returncode or 0, stdout_b.decode("utf-8", "replace"), stderr_b.decode("utf-8", "replace")


def _peer_flags(peers: Sequence[str]) -> list[str]:
    """Expand peer URLs into repeated ``--peer <url>`` flags."""
    flags: list[str] = []
    for peer in peers:
        flags.extend(("--peer", peer))
    return flags


def _parse_entries(stdout: str) -> list[ResourceEntry]:
    """Parse ``ix resources ls --json`` stdout into typed entries.

    Accepts either a bare JSON array of entries or an object with a ``resources``
    (or ``entries``) array, since a CLI may wrap its list. A non-list/!dict
    payload, or one whose items fail validation, raises
    :class:`ResourceBridgeError`.
    """
    text = stdout.strip()
    if not text:
        return []
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ResourceBridgeError(f"ix resources ls returned invalid JSON: {exc}") from exc
    if isinstance(payload, dict):
        items = payload.get("resources", payload.get("entries", []))
    else:
        items = payload
    if not isinstance(items, list):
        raise ResourceBridgeError("ix resources ls JSON is not a list of entries")
    try:
        return [ResourceEntry.model_validate(item) for item in items]
    except ValidationError as exc:
        raise ResourceBridgeError(f"ix resources ls entry failed validation: {exc}") from exc


async def list_resources(peers: Sequence[str] | None = None) -> list[ResourceEntry]:
    """List federated resources via ``ix resources ls --json``.

    ``peers`` defaults to :func:`configured_peers` (``IX_RESOURCE_PEERS``); pass an
    explicit list to override. Each peer becomes a ``--peer <url>`` flag; an empty
    list runs ``ix resources ls`` with no peer flag (local-only).

    Degrades gracefully: returns ``[]`` (and logs) when ``ix`` is not installed or
    the command exits nonzero -- it never raises for an absent/unhealthy CLI, so a
    ``resources/list`` always answers. A *successful* command with a malformed
    JSON body still raises (that is a real contract violation, not a missing tool).
    """
    if peers is None:
        peers = configured_peers()
    args = ["resources", "ls", "--json", *_peer_flags(peers)]
    try:
        rc, stdout, stderr = await _run_ix(args)
    except FileNotFoundError:
        logger.info("ix CLI not on PATH; federated resources/list is empty")
        return []
    except ResourceBridgeError as exc:
        logger.warning("ix resources ls failed: %s", exc)
        return []
    if rc != 0:
        logger.warning("ix resources ls exited %d: %s", rc, stderr.strip() or stdout.strip())
        return []
    return _parse_entries(stdout)


def _peers_to_try(peer: str | None) -> list[str]:
    """The peer URLs a single-uri op should try, in order.

    An explicit ``peer`` (a full endpoint URL) is the only candidate when given.
    Otherwise the configured peer URLs (``IX_RESOURCE_PEERS``) are returned, to be
    probed in turn. Returns an empty list only when no peer is configured and none
    was passed -- the caller turns that into a clear "no peer configured" error,
    because the CLI's ``--peer`` is a URL and the uri's host is a label, not an
    endpoint we could fall back to.
    """
    if peer:
        return [peer]
    return configured_peers()


def _no_peer_error(op: str, uri: str) -> ResourceBridgeError:
    """The error raised when a read/act has no peer URL to target."""
    return ResourceBridgeError(
        f"cannot {op} {uri}: no peer configured "
        f"(set {PEERS_ENV} or pass an explicit peer URL)",
    )


class _PeerProbeFailures:
    """Accumulate per-peer probe failures to pick the right terminal error.

    Multi-peer iteration tries each configured peer in turn; a peer can fail two
    ways: it does not advertise the uri (:class:`ResourceNotFoundError`), or it is
    unreachable/erroring (:class:`ResourceBridgeError`). After all peers are
    exhausted we want the *most specific true* outcome:

    * every peer answered "not found" -> a genuine :class:`ResourceNotFoundError`
      (the resource exists nowhere we can reach), mapping to MCP ``-32002``;
    * at least one peer errored for a transport/parse reason -> the last such
      :class:`ResourceBridgeError`, so an all-peers-down read is reported as a
      real failure, never masqueraded as a clean not-found.
    """

    def __init__(self) -> None:
        self._last_not_found: ResourceNotFoundError | None = None
        self._last_other: ResourceBridgeError | None = None

    def record(self, exc: ResourceBridgeError) -> None:
        if isinstance(exc, ResourceNotFoundError):
            self._last_not_found = exc
        else:
            self._last_other = exc

    def terminal(self, uri: str) -> ResourceBridgeError:
        """The error to raise once every peer has been tried without a hit."""
        if self._last_other is not None:
            return self._last_other
        return self._last_not_found or ResourceNotFoundError(f"unknown resource: {uri}")


async def _get_snapshot(uri: str, peer_url: str) -> ResourceSnapshot:
    """Probe one peer for ``uri`` via ``ix resources get <uri> --peer <url> --json``.

    Returns the parsed snapshot, or raises :class:`ResourceNotFoundError` when that
    peer does not advertise the uri (so the caller can try the next peer), and
    :class:`ResourceBridgeError` for any other failure against this peer.
    """
    args = ["resources", "get", uri, "--json", "--peer", peer_url]
    try:
        rc, stdout, stderr = await _run_ix(args)
    except FileNotFoundError as exc:
        raise ResourceBridgeError("ix CLI not found on PATH; cannot read federated resource") from exc
    if rc != 0:
        message = stderr.strip() or stdout.strip()
        if _looks_not_found(message):
            raise ResourceNotFoundError(f"unknown resource: {uri}")
        raise ResourceBridgeError(f"ix resources get {uri} exited {rc}: {message}")
    text = stdout.strip()
    if not text:
        raise ResourceNotFoundError(f"unknown resource: {uri}")
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ResourceBridgeError(f"ix resources get returned invalid JSON: {exc}") from exc
    try:
        return ResourceSnapshot.model_validate(payload)
    except ValidationError as exc:
        raise ResourceBridgeError(f"ix resources get snapshot failed validation: {exc}") from exc


async def read_resource(uri: str, peer: str | None = None) -> tuple[str, str]:
    """Read a snapshot of ``uri``, resolving a real peer endpoint URL.

    Returns ``(text, mime)``. With an explicit ``peer`` (a full endpoint URL) the
    bridge runs one ``ix resources get <uri> --peer <url> --json``. Otherwise it
    iterates the configured peer URLs (``IX_RESOURCE_PEERS``), probing each until
    one advertises the uri.

    Raises :class:`ResourceNotFoundError` (mapped by the server to MCP error
    ``-32002``) when no targeted peer has the uri, and :class:`ResourceBridgeError`
    for any other failure (no peer configured, CLI missing, a peer's non-404
    nonzero exit, bad JSON).
    """
    peers = _peers_to_try(peer)
    if not peers:
        raise _no_peer_error("read", uri)
    failures = _PeerProbeFailures()
    for peer_url in peers:
        try:
            snapshot = await _get_snapshot(uri, peer_url)
        except ResourceBridgeError as exc:
            # This peer 404'd OR is unreachable/erroring; either way it does not
            # serve the uri *right now*, so try the next configured peer rather
            # than letting one dead peer hide a resource a later peer owns.
            failures.record(exc)
            continue
        return snapshot.text, snapshot.mime
    # Exhausted every peer. Surface a not-found only if *every* failure was a true
    # not-found; otherwise the most recent transport/parse error (so an
    # all-peers-down read does not masquerade as -32002).
    raise failures.terminal(uri)


def _looks_not_found(message: str) -> bool:
    """Heuristic: does this CLI stderr describe a missing *resource*?

    The federated CLI does not (yet) expose a machine error code, so a not-found
    is inferred from the message. Kept narrow so an unrelated failure is not
    misreported as ``-32002``:

    * Phrases that are unambiguously about a missing resource map to not-found
      regardless of wording around them (``"resource not found"`` etc.).
    * The bare ``"not found"`` / ``"does not exist"`` family only counts when the
      message also mentions a resource, so a shell ``"command not found"`` or an
      ``"unknown peer"`` / ``"unknown flag"`` (a config/transport error) is NOT
      swallowed as a resource-not-found.
    """
    lowered = message.lower()
    resource_phrases = ("resource not found", "no such resource", "unknown resource")
    if any(phrase in lowered for phrase in resource_phrases):
        return True
    generic = ("not found", "no such", "does not exist", "not available")
    return "resource" in lowered and any(token in lowered for token in generic)


async def _resolve_peer_for(uri: str, peers: Sequence[str]) -> str:
    """Return the single peer URL (from ``peers``) that advertises ``uri``.

    Probes each candidate with ``ix resources get`` (a single explicit ``peers``
    list short-circuits to that one URL without a probe -- an explicit ``peer=``
    is the caller's assertion). Raises :class:`ResourceNotFoundError` when no peer
    advertises the uri, so ``act`` never sends keystrokes to a peer that does not
    own the resource.
    """
    if len(peers) == 1:
        return peers[0]
    failures = _PeerProbeFailures()
    for peer_url in peers:
        try:
            await _get_snapshot(uri, peer_url)
        except ResourceBridgeError as exc:
            # 404 or unreachable/erroring: this peer can't serve the uri now, so
            # keep probing rather than letting a dead peer hide the real owner.
            failures.record(exc)
            continue
        return peer_url
    raise failures.terminal(uri)


async def act(uri: str, send_keys: str, peer: str | None = None) -> dict[str, object]:
    """Drive a resource: ``ix resources act <uri> --send-keys <s> --peer <url>``.

    Resolves a real peer endpoint URL the same way :func:`read_resource` does. With
    an explicit ``peer`` (a full endpoint URL) the bridge acts against it directly.
    Otherwise it first probes the configured peer URLs (``IX_RESOURCE_PEERS``) with
    ``ix resources get`` to find the *one* peer that advertises the uri, then sends
    the keys only to that peer -- so keystrokes never land on the wrong host.

    Returns the parsed Ack as a plain dict (the agent-facing tool hands it back
    verbatim). Raises :class:`ResourceNotFoundError` for an unknown uri (-> -32002,
    when no targeted peer has it) and :class:`ResourceBridgeError` otherwise (no
    peer configured, CLI missing, nonzero exit, bad JSON).
    """
    peers = _peers_to_try(peer)
    if not peers:
        raise _no_peer_error("act on", uri)
    peer_url = await _resolve_peer_for(uri, peers)
    args = ["resources", "act", uri, "--send-keys", send_keys, "--json", "--peer", peer_url]
    try:
        rc, stdout, stderr = await _run_ix(args)
    except FileNotFoundError as exc:
        raise ResourceBridgeError("ix CLI not found on PATH; cannot act on federated resource") from exc
    if rc != 0:
        message = stderr.strip() or stdout.strip()
        if _looks_not_found(message):
            raise ResourceNotFoundError(f"unknown resource: {uri}")
        raise ResourceBridgeError(f"ix resources act {uri} exited {rc}: {message}")
    text = stdout.strip()
    if not text:
        # An empty body on success is a bare ack.
        return Ack(uri=uri, ok=True).model_dump(exclude_none=True)
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ResourceBridgeError(f"ix resources act returned invalid JSON: {exc}") from exc
    try:
        ack = Ack.model_validate(payload)
    except ValidationError as exc:
        raise ResourceBridgeError(f"ix resources act ack failed validation: {exc}") from exc
    # Return the parsed-and-revalidated payload (named fields plus anything the CLI
    # added, since the agent may want the extras) rather than only the typed subset.
    merged: dict[str, object] = dict(payload) if isinstance(payload, dict) else {}
    merged.update(ack.model_dump(exclude_none=True))
    return merged
