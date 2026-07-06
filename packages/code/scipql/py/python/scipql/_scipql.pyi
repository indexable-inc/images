"""Type stub for the unibind-generated extension module backing `scipql`.

Hand-maintained to mirror packages/code/scipql/py/src/lib.rs (the
`#[unibind::export]` module). Keep in sync when changing the binding; stub
generation from the embedded IR lands with unibind phase 1 (#1991). The
public `scipql` API (`__init__.py`) lowers these record classes into polars
DataFrames.
"""

from typing import final

__version__: str


class ScipqlError(ValueError):
    """Everything the scipql boundary raises, split by pipeline stage."""


class IndexingError(ScipqlError):
    """Producing, loading, or lowering the SCIP index failed."""


class SouffleError(ScipqlError):
    """Materializing facts or running Soufflé failed."""


class EditError(ScipqlError):
    """Computing or applying edits failed."""


@final
class Occurrence:
    """One `occurrence` fact: a symbol use site with its byte range and role."""

    symbol: str
    path: str
    start: int
    end: int
    role: str

    def __init__(self, symbol: str, path: str, start: int, end: int, role: str) -> None: ...


@final
class SymbolInfo:
    """One `symbol_info` fact: a symbol's kind and display name."""

    symbol: str
    kind: str
    display_name: str

    def __init__(self, symbol: str, kind: str, display_name: str) -> None: ...


@final
class Document:
    """One `document` fact: an indexed source path."""

    path: str

    def __init__(self, path: str) -> None: ...


@final
class Relationship:
    """One `relationship` fact: a typed edge between two symbols."""

    symbol: str
    related: str
    kind: str

    def __init__(self, symbol: str, related: str, kind: str) -> None: ...


@final
class Facts:
    """The four fact relations a SCIP index lowers into."""

    occurrence: list[Occurrence]
    symbol_info: list[SymbolInfo]
    document: list[Document]
    relationship: list[Relationship]

    def __init__(
        self,
        occurrence: list[Occurrence],
        symbol_info: list[SymbolInfo],
        document: list[Document],
        relationship: list[Relationship],
    ) -> None: ...


@final
class Relation:
    """One Soufflé `.output` relation: its column names and untyped string rows."""

    columns: list[str]
    rows: list[dict[str, str]]

    def __init__(self, columns: list[str], rows: list[dict[str, str]]) -> None: ...


def index(project: str, output: str = ...) -> str:
    """Run rust-analyzer's SCIP indexer over ``project``; return the output path."""


def facts(index_path: str, root: str | None = ...) -> Facts:
    """Lower a SCIP index into its four fact relations (see ``scipql.facts``)."""


def query(index_path: str, program: str, root: str | None = ...) -> dict[str, Relation]:
    """Run a Soufflé ``program``; return one relation per ``.output`` declaration."""


def fix(index_path: str, program: str, root: str | None = ..., write: bool = ...) -> str:
    """Apply a ``fix`` program's ``edit`` relation; return the unified diff."""


def rename(
    index_path: str,
    selector: str,
    new_name: str,
    root: str | None = ...,
    write: bool = ...,
) -> str:
    """Rename every occurrence whose moniker ends with ``selector``; return the diff."""
