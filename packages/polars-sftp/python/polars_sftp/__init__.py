"""Polars IO source for remote files over SFTP.

``scan_sftp`` returns a lazy ``pl.LazyFrame`` backed by a file on a remote host,
read over SFTP and decoded by Polars (parquet / IPC / CSV). It is registered
through Polars' official IO-plugin hook (``register_io_source``), so it composes
with the rest of a lazy query and participates in projection, ``n_rows`` and
predicate handling. The actual read + decode happens in Rust (``_polars_sftp``).
"""

from __future__ import annotations

from typing import TypedDict
from collections.abc import Iterator

import polars as pl
from polars.io.plugins import register_io_source

from ._polars_sftp import __version__, read_sftp

__all__ = ["__version__", "read_sftp", "scan_sftp"]


class _ConnKwargs(TypedDict):
    """The connection arguments forwarded to ``read_sftp`` on every read.

    Typed so the ``**conn`` splat matches ``read_sftp``'s per-keyword types
    (a bare dict literal would widen them to one union and fail strict checking).
    """

    port: int
    username: str | None
    password: str | None
    private_key: str | None
    storage_format: str | None
    timeout_ms: int
    check_host_key: bool


def scan_sftp(
    host: str,
    path: str,
    *,
    port: int = 22,
    username: str | None = None,
    password: str | None = None,
    private_key: str | None = None,
    storage_format: str | None = None,
    timeout_ms: int = 30_000,
    check_host_key: bool = True,
) -> pl.LazyFrame:
    """Lazily scan a remote file over SFTP.

    Reads ``parquet`` / ``ipc`` / ``csv`` from ``host``:``path`` (format inferred
    from the extension unless ``storage_format`` is given). Column projection and
    an ``n_rows`` limit are pushed into the reader; any predicate is applied to the
    returned data. Authentication is tried in order: ``password``, then
    ``private_key`` file, then the SSH agent. ``username`` defaults to ``$USER``.

    The remote file is fetched in full and decoded in memory, so projection trims
    decoding and output rather than bytes transferred over the wire.
    """
    conn: _ConnKwargs = {
        "port": port,
        "username": username,
        "password": password,
        "private_key": private_key,
        "storage_format": storage_format,
        "timeout_ms": timeout_ms,
        "check_host_key": check_host_key,
    }

    def _schema() -> pl.Schema:
        # Read zero rows: this fetches the file and returns an empty frame whose
        # schema is the source schema (before projection).
        return read_sftp(host, path, with_columns=None, n_rows=0, **conn).schema

    def _source(
        with_columns: list[str] | None,
        predicate: pl.Expr | None,
        n_rows: int | None,
        batch_size: int | None,
    ) -> Iterator[pl.DataFrame]:
        df = read_sftp(host, path, with_columns=with_columns, n_rows=n_rows, **conn)
        if predicate is not None:
            df = df.filter(predicate)
        yield df

    return register_io_source(_source, schema=_schema)
