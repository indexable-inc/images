"""Publish dashboard panes into the shared Loro hub.

This is the Python side of the dashboard-core producer protocol (the Rust
contract lives in ``packages/dashboard-core/src/pane.rs`` and ``publish.rs``).
A producer *binds* a unix socket in the discovery directory and streams its full
pane set as one NDJSON ``ProducerSnapshot`` line to every reader; the standalone
``dashboard`` aggregator connects in, folds each producer's stream into one Loro
document under its own scope, and serves the shared canvas over HTTP + SSE.

The producer holds no HTTP or CRDT dependency: it serializes pane dicts and
writes bytes. Each message carries the *full* current pane set (replacement
semantics), so the latest line fully describes this process and a late-joining
aggregator needs no backlog. Binding is best-effort: if the discovery directory
is unwritable the MCP keeps working without a dashboard.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import stat
import uuid
from pathlib import Path

# Cap on an exec pane's title, matching `exec_title` in pane.rs so a long
# one-liner reads as a label rather than overflowing the card head.
_TITLE_MAX = 60


def discovery_dir() -> Path:
    """Where producers expose their sockets and the aggregator looks for them.

    Resolved in the same order as ``dashboard_core::discovery_dir``: ``$IX_DASH_DIR``,
    then ``$XDG_RUNTIME_DIR/ix-dash``, then ``/tmp/ix-dash-<user>``. Kept short
    because macOS caps a unix socket path at 104 bytes.
    """
    if env := os.environ.get("IX_DASH_DIR"):
        return Path(env)
    if runtime := os.environ.get("XDG_RUNTIME_DIR"):
        return Path(runtime) / "ix-dash"
    user = os.environ.get("USER", "shared")
    return Path(f"/tmp/ix-dash-{user}")


def socket_path() -> Path:
    """A unique socket path for this process inside :func:`discovery_dir`.

    The filename is ``<pid>-<short-uuid>.sock``: the pid is legible for debugging
    and the uuid suffix keeps it unique across pid reuse.
    """
    return discovery_dir() / f"{os.getpid()}-{uuid.uuid4().hex[:8]}.sock"


def _exec_title(source: str) -> str:
    """The card title for an execution: the first non-empty source line, trimmed
    and length-capped. Mirrors ``exec_title`` in pane.rs."""
    line = next((s for s in (ln.strip() for ln in source.splitlines()) if s), "python")
    # Match exec_title in pane.rs: first MAX chars, then an ellipsis.
    return f"{line[:_TITLE_MAX]}…" if len(line) > _TITLE_MAX else line


def exec_pane(
    pane_id: str,
    *,
    source: str,
    running: bool,
    stdout: str = "",
    stderr: str = "",
    result: str = "",
    ok: bool | None = None,
    lang: str = "python",
    duration_ms: int | None = None,
    trace: list[dict] | None = None,
    title: str | None = None,
    subtitle: str = "",
) -> dict:
    """One captured run as an ``exec`` pane. Publish a ``running`` pane when the
    call starts, then replace it with the finished view (``running=False``, ``ok``
    set) when it returns, so the card animates from running to its result."""
    view: dict = {
        "kind": "exec",
        "source": source,
        "lang": lang,
        "stdout": stdout,
        "stderr": stderr,
        "result": result,
        "running": running,
    }
    if ok is not None:
        view["ok"] = ok
    if duration_ms is not None:
        view["duration_ms"] = duration_ms
    if trace:
        view["trace"] = trace
    return {
        "id": pane_id,
        "title": title if title is not None else _exec_title(source),
        "subtitle": subtitle,
        "view": view,
    }


def data_pane(pane_id: str, title: str, renderer: str, data: object, subtitle: str = "") -> dict:
    """A ``data`` pane: structured JSON rendered by the named frontend ``renderer``
    (an unknown name falls back to the generic JSON tree)."""
    return {
        "id": pane_id,
        "title": title,
        "subtitle": subtitle,
        "view": {"kind": "data", "renderer": renderer, "data": data},
    }


def html_pane(pane_id: str, title: str, html: str, subtitle: str = "") -> dict:
    """An ``html`` pane: the producer ships its own UI, mounted in a sandboxed frame."""
    return {
        "id": pane_id,
        "title": title,
        "subtitle": subtitle,
        "view": {"kind": "html", "html": html},
    }


class PaneProducer:
    """Binds a producer socket and streams the current pane set to every reader.

    Call :meth:`start` to bind, :meth:`publish` to replace the streamed set, and
    :meth:`stop` to unbind. :meth:`start` returns ``None`` (logging to stderr)
    when the socket cannot bind, so a caller can treat the dashboard as an
    optional convenience.
    """

    def __init__(self) -> None:
        self.producer_id = f"{os.getpid()}-{uuid.uuid4().hex[:8]}"
        self._line = self._encode([])
        self._version = 0
        self._cond = asyncio.Condition()
        self._server: asyncio.AbstractServer | None = None
        self._path: Path | None = None
        # Active per-connection writer tasks, so stop() can cancel the ones parked
        # waiting for the next snapshot — since CPython 3.12 `wait_closed()` blocks
        # on them, and they never wake on their own.
        self._handlers: set[asyncio.Task] = set()

    def _encode(self, panes: list[dict]) -> bytes:
        snapshot = {"producer": self.producer_id, "panes": panes}
        return (json.dumps(snapshot, separators=(",", ":")) + "\n").encode("utf-8")

    async def start(self) -> "PaneProducer | None":
        """Bind the producer socket in the discovery directory. Best-effort: on
        failure logs to stderr and returns ``None`` so the caller keeps working."""
        try:
            path = socket_path()
            _ensure_dir(path.parent)
            _reap_stale_socket(path)
            self._server = await asyncio.start_unix_server(self._handle, path=str(path))
            with contextlib.suppress(OSError):
                os.chmod(path, 0o600)
            self._path = path
            return self
        except OSError as error:
            print(f"ix-mcp: dashboard panes disabled ({error})", flush=True)
            return None

    async def publish(self, panes: list[dict]) -> None:
        """Replace the snapshot streamed to every reader. Cheap: serialize one
        line, bump the version, and wake the per-connection writers."""
        async with self._cond:
            self._line = self._encode(panes)
            self._version += 1
            self._cond.notify_all()

    async def _handle(self, _reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        """Feed one reader: write the current snapshot, then each new one as it
        lands, until the reader hangs up."""
        task = asyncio.current_task()
        if task is not None:
            self._handlers.add(task)
        last = None
        try:
            while True:
                async with self._cond:
                    await self._cond.wait_for(lambda: self._version != last)
                    line, last = self._line, self._version
                writer.write(line)
                await writer.drain()
        except (ConnectionResetError, BrokenPipeError, asyncio.CancelledError):
            pass
        finally:
            if task is not None:
                self._handlers.discard(task)
            writer.close()
            with contextlib.suppress(Exception):
                await writer.wait_closed()

    async def stop(self) -> None:
        """Stop accepting readers and unlink the socket. Idempotent."""
        if self._server is not None:
            self._server.close()
            # Cancel the parked writer tasks first: `wait_closed()` waits for
            # active handlers, and ours block on the snapshot Condition with no
            # other wakeup, so without this it would hang forever.
            for task in list(self._handlers):
                task.cancel()
            with contextlib.suppress(Exception):
                await self._server.wait_closed()
            self._server = None
        if self._path is not None:
            with contextlib.suppress(OSError):
                self._path.unlink()
            self._path = None


def _ensure_dir(directory: Path) -> None:
    """Create the discovery directory if missing, restricting one we create to the
    owner (``0700``). A pre-existing directory's permissions are left untouched."""
    if not directory.exists():
        directory.mkdir(parents=True, exist_ok=True)
        with contextlib.suppress(OSError):
            os.chmod(directory, 0o700)


def _reap_stale_socket(path: Path) -> None:
    """Reap a stale socket left by a crashed producer so the bind does not fail
    with ``EADDRINUSE``. Only an actual socket is removed; a path that exists and
    is something else is an error, never silently deleted."""
    try:
        mode = os.lstat(path).st_mode
    except FileNotFoundError:
        return
    if stat.S_ISSOCK(mode):
        path.unlink()
    else:
        raise OSError(f"{path} exists and is not a socket; refusing to overwrite")
