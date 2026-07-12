"""A sample boundary for the emitter tests.

Everything the py generator renders appears here once.
"""


import os


class SampleError(ValueError):
    """Everything the sample boundary raises."""


class ParseError(SampleError):
    """The input did not parse."""


class Io(SampleError): ...


class Row:
    """One result row."""

    def __init__(self, id: int, label: str, tags: list[str], scores: dict[str, float]) -> None: ...

    @property
    def id(self) -> int:
        """Identifier."""

    @property
    def label(self) -> str: ...

    @property
    def tags(self) -> list[str]: ...

    @property
    def scores(self) -> dict[str, float]: ...


class Source:
    def __init__(self, path: str | os.PathLike[str]) -> None: ...

    @property
    def path(self) -> str:
        """Where the row came from."""


class Store:
    """A stateful handle over one store."""

    def __init__(self, path: str | os.PathLike[str], create: bool = True) -> None:
        """Open a store.

        Raises SampleError.
        """

    async def head(self) -> Row:
        """The first row.

        Raises SampleError.
        """

    def watch(self, prefix: str = "") -> StoreWatchStream:
        """Watch rows as they land."""

    def open_cursor(self) -> StoreCursor:
        """Open a cursor over the store."""


class StoreCursor:
    """A resource over one store; instances come from `Store.cursor`."""

    async def read(self, max_bytes: int = 4096) -> str:
        """Read the next chunk."""

    async def close(self) -> None:
        """Release the cursor.

        Raises SampleError.
        """

    async def __aenter__(self) -> StoreCursor:
        """Enter `async with`: resolves to the object itself."""

    async def __aexit__(self, exc_type: object, exc: object, tb: object) -> bool:
        """Exit `async with`: closes the resource, never suppresses the exception."""


class TailStream:
    """Async iterator produced by `tail`.

    Pull-based: each `__anext__` polls exactly one item, so the producer only runs as fast as the consumer awaits.
    """

    def __aiter__(self) -> TailStream: ...

    async def __anext__(self) -> str: ...


class StoreWatchStream:
    """Async iterator produced by `Store.watch`.

    Pull-based: each `__anext__` polls exactly one item, so the producer only runs as fast as the consumer awaits.
    """

    def __aiter__(self) -> StoreWatchStream: ...

    async def __anext__(self) -> Row: ...


def rows(store: str, limit: int = 10) -> list[Row]:
    """Fetch rows.

    Docs become docstrings.

    Raises SampleError.
    """


def write_file(path: str | os.PathLike[str], data: bytes, overwrite: bool = False) -> None:
    """Write `data` to `path`."""


def find(pattern: str, root: str | os.PathLike[str] | None = None) -> dict[str, Row]:
    """Raises SampleError."""


def greet(name: str = "hello \"world\"\n", ratio: float = 1.0, note: str | None = None) -> str: ...


def tail(store: str) -> TailStream:
    """Follow appended rows."""


async def ping() -> bool:
    """Probe the store."""


__version__: str

