"""Durable local spool: an fsync'd JSONL journal drained to weave by a flusher.

The recording invariant (index#3418): intent is durable BEFORE any dependent
action proceeds. ``append`` writes whole JSON lines under an exclusive file
lock and fsyncs before returning; a background flusher hands batches to an
injected ``sender`` and only advances the fsync'd cursor sidecar after the
sender returns. That gives at-least-once delivery in local append order, with
weave as an eventually-consistent sink: the server being down costs exactly
one loud stderr line per process per URL (and one recovery line), never a
dropped fact and never a blocked writer.

Layout: each :class:`Spool` instance owns one uniquely named ``*.jsonl``
segment (pid + random token) under ``flock(LOCK_EX)``, plus a ``.cursor``
sidecar holding the drained byte offset. On startup an instance adopts any
unlocked sibling segments (crashed writers) and drains them before its own
appends resolve nothing about cross-segment order: only per-segment (per-run)
order is promised. Item shapes are the weave WriteRequest wire form
``{"fact": {...}}`` plus the deferred-CAS form
``{"blob_b64": <b64>, "refs": [{"entity": <ApiValue>, "attr": <str>}]}``
(the sender puts the bytes and emits one hash-valued fact per ref), so any
process's spool can drain any other's segments.
"""

from __future__ import annotations

import fcntl
import json
import os
import secrets
import sys
import threading
import time
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import Callable

_BACKOFF_MIN_S = 0.25
_BACKOFF_MAX_S = 5.0
_BATCH = 500
# Truncate a fully drained segment past this size so long-lived kernels do
# not grow an unbounded on-disk journal of already-delivered facts.
_COMPACT_BYTES = 4 << 20

# One loud line per process per URL on the reachable->unreachable transition
# (and one on recovery), shared across every Spool instance pointing at that
# URL - the store, fabric, and dashboard spools must not each repeat it.
_down_lock = threading.Lock()
_down_urls: set[str] = set()

_instances_lock = threading.Lock()
_instances: list[Spool] = []


def _note_down(url: str, directory: Path, exc: BaseException) -> None:
    with _down_lock:
        if url in _down_urls:
            return
        _down_urls.add(url)
    print(
        f"weave spool: {url} unreachable ({type(exc).__name__}: {exc}); writes stay durable in "
        f"{directory} and drain when it returns (health check: `curl -s {url}/api/info`; "
        "restart: `launchctl kickstart -k gui/501/org.nix-community.home.weave-serve`)",
        file=sys.stderr,
    )


def _note_up(url: str) -> None:
    with _down_lock:
        if url not in _down_urls:
            return
        _down_urls.discard(url)
    print(f"weave spool: {url} reachable again; draining spooled writes", file=sys.stderr)


class _Segment:
    """One JSONL file plus its fsync'd ``.cursor`` byte-offset sidecar."""

    def __init__(self, path: Path, *, create: bool) -> None:
        self.path = path
        self.fd = os.open(path, os.O_RDWR | (os.O_CREAT if create else 0), 0o600)
        try:
            # LOCK_NB: adoption must skip segments a live writer still holds.
            fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            os.close(self.fd)
            raise
        self.cursor_path = Path(f"{path}.cursor")
        self.cursor = 0
        try:
            self.cursor = int(self.cursor_path.read_text() or "0")
        except (FileNotFoundError, ValueError):
            self.cursor = 0
        self.size = os.fstat(self.fd).st_size
        self.cursor = min(self.cursor, self.size)

    @property
    def drained(self) -> bool:
        return self.cursor >= self.size

    def append(self, data: bytes) -> None:
        os.lseek(self.fd, 0, os.SEEK_END)
        view = memoryview(data)
        while view:
            view = view[os.write(self.fd, view) :]
        os.fsync(self.fd)
        self.size += len(data)

    def read_batch(self, limit: int) -> tuple[list[Any], int]:
        """Up to ``limit`` complete-line items from the cursor, plus the byte
        offset just past them. A torn trailing line (crashed writer) is never
        consumed; a corrupt complete line is skipped loudly, its bytes still
        advance the offset."""
        os.lseek(self.fd, self.cursor, os.SEEK_SET)
        buf = os.read(self.fd, self.size - self.cursor)
        items: list[Any] = []
        offset = self.cursor
        for raw in buf.split(b"\n")[:-1]:
            if len(items) >= limit:
                break
            offset += len(raw) + 1
            if not raw:
                continue
            try:
                items.append(json.loads(raw))
            except ValueError:
                print(f"weave spool: skipping corrupt line in {self.path}", file=sys.stderr)
        return items, offset

    def commit(self, offset: int) -> None:
        tmp = Path(f"{self.cursor_path}.tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        try:
            os.write(fd, str(offset).encode())
            os.fsync(fd)
        finally:
            os.close(fd)
        tmp.replace(self.cursor_path)
        self.cursor = offset

    def compact_if_drained(self) -> None:
        if self.drained and self.size >= _COMPACT_BYTES:
            os.ftruncate(self.fd, 0)
            self.size = 0
            self.commit(0)

    def close(self) -> None:
        os.close(self.fd)

    def remove(self) -> None:
        os.close(self.fd)
        self.path.unlink(missing_ok=True)
        self.cursor_path.unlink(missing_ok=True)


class Spool:
    """A durable write-behind queue draining to one weave URL.

    ``sender`` receives batches of spooled items and raises on failure;
    ``permanent(exc)`` classifies a failure retry cannot fix (an auth
    rejection), after which ``on_permanent(exc)`` fires once and the flusher
    parks with the segment retained on disk. Everything else retries with
    backoff forever - facts are never dropped.
    """

    def __init__(
        self,
        directory: str | Path,
        sender: Callable[[list[Any]], None],
        *,
        url: str,
        permanent: Callable[[BaseException], bool] | None = None,
        on_permanent: Callable[[BaseException], None] | None = None,
    ) -> None:
        self.dir = Path(directory)
        self.dir.mkdir(parents=True, exist_ok=True)
        self._sender = sender
        self._url = url
        self._permanent = permanent if permanent is not None else lambda _exc: False
        self._on_permanent = on_permanent if on_permanent is not None else self._warn_permanent
        self.parked = False
        self._closed = False
        self._cv = threading.Condition()
        # pid + random token: never collides with a live sibling's segment,
        # and a recycled pid cannot land on a crashed process's file.
        self._own = _Segment(self.dir / f"w-{os.getpid()}-{secrets.token_hex(4)}.jsonl", create=True)
        self._orphans = self._adopt()
        self._thread = threading.Thread(target=self._flusher, name="weave-spool", daemon=True)
        self._thread.start()
        with _instances_lock:
            _instances.append(self)

    @property
    def closed(self) -> bool:
        return self._closed

    def _warn_permanent(self, exc: BaseException) -> None:
        print(
            f"weave spool: {self._url} rejected writes permanently ({exc}); "
            f"flusher parked, spool retained in {self.dir}",
            file=sys.stderr,
        )

    def _adopt(self) -> list[_Segment]:
        orphans = []
        for path in sorted(self.dir.glob("*.jsonl")):
            if path == self._own.path:
                continue
            try:
                seg = _Segment(path, create=False)
            except OSError:
                continue  # a live writer holds it (or it vanished mid-scan)
            if seg.drained:
                seg.remove()
            else:
                orphans.append(seg)
        return orphans

    def append(self, item: object) -> None:
        self.append_many([item])

    def append_many(self, items: list[Any]) -> None:
        """Durably append items (one fsync for the batch), then return."""
        if not items:
            return
        data = b"".join(json.dumps(item, separators=(",", ":")).encode() + b"\n" for item in items)
        with self._cv:
            if self._closed:
                raise RuntimeError(f"weave spool is closed: {self._own.path}")
            self._own.append(data)
            self._cv.notify_all()

    def _pending_locked(self) -> bool:
        return bool(self._orphans) or not self._own.drained

    def pending(self) -> bool:
        with self._cv:
            return self._pending_locked()

    def _flusher(self) -> None:
        backoff = _BACKOFF_MIN_S
        while True:
            with self._cv:
                while not (self._closed or self.parked or self._pending_locked()):
                    self._cv.wait(0.5)
                if self.parked:
                    return
                if not self._pending_locked():
                    if self._closed:
                        return
                    continue
                seg = self._orphans[0] if self._orphans else self._own
                items, offset = seg.read_batch(_BATCH)
            if not items and offset == seg.cursor:
                # Nothing sendable: an adopted segment whose only bytes are a
                # crashed writer's torn trailing line. That append never
                # returned to its caller, so it never became durable intent.
                with self._cv:
                    if seg is self._own:
                        seg.commit(seg.size)  # unreachable: own appends are whole lines
                    else:
                        self._orphans.remove(seg)
                        seg.remove()
                    self._cv.notify_all()
                continue
            if items:
                try:
                    self._sender(items)
                except Exception as exc:  # classified: park or retry
                    if self._permanent(exc):
                        self._on_permanent(exc)
                        with self._cv:
                            self.parked = True
                            self._cv.notify_all()
                        return
                    _note_down(self._url, self.dir, exc)
                    with self._cv:
                        if self._closed:
                            return  # segment survives; a later process adopts it
                        self._cv.wait(backoff)
                    backoff = min(backoff * 2, _BACKOFF_MAX_S)
                    continue
                backoff = _BACKOFF_MIN_S
                _note_up(self._url)
            with self._cv:
                seg.commit(offset)
                if seg is self._own:
                    seg.compact_if_drained()
                elif seg.drained:
                    self._orphans.remove(seg)
                    seg.remove()
                self._cv.notify_all()

    def flush(self, timeout: float = 10.0) -> bool:
        """Block until every spooled item is delivered (or ``timeout``).

        Returns True when drained. A parked spool also returns True: the
        rejection was permanent, waiting cannot help, and callers must not
        wedge on it (the on_permanent hook already shouted)."""
        deadline = time.monotonic() + timeout
        with self._cv:
            self._cv.notify_all()
            while self._pending_locked():
                if self.parked:
                    return True
                if self._closed:
                    return False
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return False
                self._cv.wait(min(remaining, 0.1))
        return True

    def close(self, timeout: float = 5.0) -> None:
        """Stop the flusher (one best-effort final drain) and release files.

        Undelivered items stay on disk for a future instance to adopt."""
        with self._cv:
            if self._closed:
                return
            self._closed = True
            self._cv.notify_all()
        self._thread.join(timeout)
        if self._thread.is_alive():
            return  # sender still mid-call: leave fds to the daemon thread
        with self._cv:
            if self._own.drained:
                self._own.remove()
            else:
                self._own.close()
            for seg in self._orphans:
                seg.close()
            self._orphans = []
        with _instances_lock:
            if self in _instances:
                _instances.remove(self)


def close_all() -> None:
    """Close every live spool in this process (tests: join flusher threads
    before monkeypatched senders revert)."""
    with _instances_lock:
        live = list(_instances)
    for sp in live:
        sp.close()