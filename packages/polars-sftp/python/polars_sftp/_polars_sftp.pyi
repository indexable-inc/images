"""Type stub for the PyO3 extension module backing `polars_sftp`."""

import polars as pl

__version__: str

def read_sftp(
    host: str,
    path: str,
    *,
    port: int = ...,
    username: str | None = ...,
    password: str | None = ...,
    private_key: str | None = ...,
    storage_format: str | None = ...,
    with_columns: list[str] | None = ...,
    n_rows: int | None = ...,
    timeout_ms: int = ...,
    check_host_key: bool = ...,
) -> pl.DataFrame:
    """Read a remote file over SFTP into a DataFrame (see `scan_sftp`)."""
    ...
