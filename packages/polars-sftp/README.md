# polars-sftp

A [Polars](https://pola.rs) IO source that reads a remote file over **SFTP** and
hands it back as a lazy `LazyFrame`. Point it at a parquet, IPC, or CSV file on
an SSH host and query it like any other Polars source:

```python
import polars as pl
from polars_sftp import scan_sftp

lf = scan_sftp("data.example.com", "/exports/events.parquet", username="andrew")
df = (
    lf.filter(pl.col("region") == "us-east")
      .select("ts", "region", "latency_ms")
      .head(1000)
      .collect()
)
```

It registers through Polars' official IO-plugin hook (`register_io_source`), so it
composes with the rest of a lazy query: column projection and an `n_rows` limit
are pushed into the reader, and any predicate is applied to the result.

## How it works

The IO-plugin interface is Python by design (Polars holds the GIL only briefly at
the hand-off and moves data zero-copy over Arrow FFI), so the surface is split:

- **Rust core** (`src/lib.rs`, a PyO3 cdylib): opens the SFTP connection with
  [`ssh2`](https://crates.io/crates/ssh2), reads the file, and decodes it with
  Polars' own parquet / IPC / CSV readers into a `DataFrame` returned as a
  `pyo3_polars::PyDataFrame`. The Rust `polars` version is pinned to match the
  Python-side Polars so the Arrow-FFI transfer lines up.
- **Python wrapper** (`python/polars_sftp/__init__.py`): `scan_sftp` probes the
  schema, then `register_io_source`s a generator that calls the Rust reader with
  the engine's projected columns and row limit and filters by the predicate.

## Authentication

Tried in order: explicit `password`, then a `private_key` file, then the SSH
agent. `username` defaults to `$USER`.

```python
scan_sftp(host, path, port=22, username=None, password=None,
          private_key="~/.ssh/id_ed25519", storage_format=None)
```

`storage_format` (`"parquet"` | `"ipc"` | `"csv"`) overrides the extension-based
format guess.

## Build

```sh
nix build .#polars-sftp     # produces the abi3 wheel under ./result
```

`pip install` the wheel into the environment that runs your Polars queries (the
crate's Rust `polars` and your Python `polars` must be the matching release).

## Known limitations

- The remote file is fetched in full and decoded in memory, so projection trims
  decoding and output, not the bytes pulled over the wire. Selective range-reads
  over SFTP (a custom `MmapBytesReader` over `ssh2` seek) would fix that and are
  the obvious next step.
- The host key is not verified (no `known_hosts` check) yet.
- One chunk per scan: the reader does not stream row groups, so `batch_size` is a
  no-op and a whole file lands in memory at once.
- The wheel links `libssh2`/`openssl` from the Nix store by rpath, so it runs in
  this Nix environment; it is not a portable manylinux wheel.

Made with AI assistance (Claude, Opus 4.8).
