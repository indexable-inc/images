"""A sample boundary for the emitter tests.

Everything the phase 1 generator renders appears here once.
"""


import collections.abc
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


def tail(store: str) -> collections.abc.AsyncIterator[str]:
    """Follow appended rows."""


async def ping() -> bool:
    """Probe the store."""


__version__: str

