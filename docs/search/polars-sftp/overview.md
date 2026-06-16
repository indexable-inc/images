# polars-sftp

`packages/polars-sftp` is a [Polars](https://pola.rs) IO source that reads a
remote file over SFTP and hands it back as a lazy `LazyFrame`. Point
`scan_sftp(host, path, ...)` at a parquet, IPC, CSV, or NDJSON (`.jsonl`) file on
an SSH host and query it like any other Polars source (README:1-18). PyO3 cdylib
plus a Python wrapper; `nix build .#polars-sftp`. Included in the search domain
as a data-access source for the corpus stack (e.g. reading remote NDJSON exports
into Polars), independent of the Mixedbread store.

## Split: Rust reader, Python plugin

The IO-plugin interface is Python by design (Polars moves data zero-copy over
Arrow FFI), so the surface is split (README:24-36):

- **Rust core** (`src/lib.rs`, a PyO3 cdylib): opens the SFTP connection with
  [`ssh2`](https://crates.io/crates/ssh2), reads the file fully into memory, and
  decodes it with Polars' own parquet/IPC/CSV/NDJSON readers into a
  `DataFrame` returned as a `pyo3_polars::PyDataFrame` (`src/lib.rs:1-14`).
  Because it decodes in Rust, the crate's `polars` version is pinned to match the
  Python-side Polars so the Arrow-FFI transfer lines up (unlike
  [`polars-mixedbread`](../polars-mixedbread/overview.md), which never touches
  Polars in Rust). `resolve_format` (`src/lib.rs:37`) infers the format from the
  extension or an explicit `storage_format` hint.
- **Python wrapper** (`python/polars_sftp/__init__.py`): `scan_sftp` probes the
  schema, then `register_io_source`s a generator that calls the Rust reader with
  the engine's projected columns and row limit and applies the predicate.

## API and auth

```python
scan_sftp(host, path, port=22, username=None, password=None,
          private_key="~/.ssh/id_ed25519", storage_format=None,
          timeout_ms=30_000, check_host_key=True)
```

Authentication is tried in order: explicit `password`, then a `private_key`
file, then the SSH agent; `username` defaults to `$USER` (README:38-47). The
server's host key is checked against `~/.ssh/known_hosts` before any credential
is sent (a recorded mismatch is rejected as a possible MITM; an unrecorded host
is trust-on-first-use); `check_host_key=False` skips it. `timeout_ms` bounds
connect and read. `storage_format` (`"parquet"`|`"ipc"`|`"csv"`|`"ndjson"`)
overrides the extension guess.

## Known limitations

- The remote file is fetched in full and decoded in memory (Polars' readers need
  `MmapBytesReader`, which an `ssh2::File` is not), so projection trims decode and
  output, not the bytes pulled over the wire (`src/lib.rs:10-14`, README:67-72).
- One chunk per scan: the reader does not stream row groups, so `batch_size` is a
  no-op.
- The wheel links `libssh2`/`openssl` from the Nix store by rpath, so it runs in
  this Nix environment; it is not a portable manylinux wheel (README:75).

## Build

`default.nix` builds the standalone cdylib (vendored `Cargo.lock`) with
`rustPlatform.buildRustPackage`, links the nix `libssh2`/`openssl` via
`OPENSSL_NO_VENDOR` + `LIBSSH2_SYS_USE_PKG_CONFIG` (no vendored C build), and
packages the abi3 wheel with `wheel/mkwheel.py`, stamping per-platform tags for
the four Linux/darwin systems (`default.nix:22-30`, `:62-86`). `package.nix` sets
`flake`/`packageSet`, so `nix build .#polars-sftp`. The Rust `polars` and the
runtime Python `polars` must be the matching release.
