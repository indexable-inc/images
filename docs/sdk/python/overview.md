# Python SDK

`sdk/python` is the `ix-sdk` Python distribution (`pyproject.toml:9`). The
importable package is `ix_sdk`: a pure-Python, async-first wrapper layer
(`ix_sdk/__init__.py`, ~1364 lines) over a native PyO3 extension `_ix_sdk` that
is NOT in this tree. The extension is compiled from the private
`crates/ix/sdk-py` and ships inside the published wheel (see
[packaging](../packaging/overview.md)); `ix_sdk/_ix_sdk.pyi` is the
hand-maintained type stub for it (`_ix_sdk.pyi:1-9`).

See [common](../common.md) for the cross-SDK model, auth, and licensing.

## Packaging metadata

- Distribution `ix-sdk`, version `0.1.0`, `requires-python = ">=3.10"`,
  `License :: Other/Proprietary License` (`pyproject.toml:9-19`).
- setuptools backend; `ix_sdk` package data ships `py.typed` and `*.pyi` so
  consumers get typing (`pyproject.toml:21-29`). The shipped tree is just
  `__init__.py`, `_ix_sdk.pyi`, `py.typed`; the compiled `_ix_sdk` is added by
  the wheel build.
- Note: `requires-python >=3.10` here, but the prebuilt wheel the Nix package
  fetches is `cp313-abi3` (`packages/ix-sdk-python/default.nix:26`), so a
  realized `pkgs.ix-sdk-python` needs CPython 3.13+.

## Object model

The wrapper exposes both a low-level client surface and a high-level `VM`/`Snapshot`
convenience. Two namespaces import from `_ix_sdk`: the raw native classes
(aliased `_Raw*`) and the public dataclasses/enums re-exported as-is
(`__init__.py:14-66`). `__all__` (`__init__.py:1294-1353`) is the public API.

### `Client` (`__init__.py:882-1067`)

Constructed with keyword-only `token` and `base_url`, both optional; the native
`_RawClient(token, base_url)` resolves defaults and `Client.base_url` reports the
effective URL (`__init__.py:883-888`). All methods are `async`. Key methods:

- Lookup: `get(branch_id)`, `get_by_name(name)`, `find_by_name(name)` (returns
  `None` on `IxNotFoundError`), `branches()`, `regions()`
  (`__init__.py:890-922`, `989-993`).
- Create: `create(image, *, region, name, env, l7_proxy_ports, ipv4,
  on_progress)` drives `create_with_progress(...)` and drains the progress
  stream (`__init__.py:945-987`); `build_snapshot_from_oci(image, *, region,
  ...)` returns a `Snapshot` (`__init__.py:893-913`).
- VM image ops: `snapshot(*, name)`, `switch_system(*, name, target/system,
  build_on=...)` (`__init__.py:924-943`).
- Account: `me()`, `current_username()`, `billing_status()`,
  `list_api_tokens()`, `revoke_api_token(token_id)` (`__init__.py:995-1011`).
- Volumes: `get_volume`, `list_volumes`, `list_volume_snapshots`
  (`__init__.py:1013-1020`).
- Previews: `create_preview`, `list_previews`, `stop_preview`, `get_preview`,
  `promote_preview` (`__init__.py:1022-1042`).
- Observability: `query_logs(...)`, `search_traces(...)`
  (`__init__.py:1044-1064`).

### `Branch` (`__init__.py:564-733`)

A running microVM. Wraps `_RawBranch` and pre-builds an `FsHandle` and
`SecretsHandle` (`__init__.py:565-569`). Surface:

- Lifecycle: `id`, `info()`, `delete()`, `pause()` -> `Snapshot`, `start()`,
  `start_with_progress()`, `restart()`, `snapshot()` -> `Snapshot`, `log()` ->
  `list[Snapshot]` (`__init__.py:571-595`, `651-652`).
- Execution: `bash(script, *, working_dir, check=True, quiet=False)`,
  `exec(command, ...)`, `exec_stream(command, ...)`, `spawn(command, ...)`. When
  not `quiet`, output is streamed to stdout/stderr via `_stream_to_result`
  (`__init__.py:127-143`, `596-649`); a non-zero exit with `check=True` raises
  `CommandError` (`__init__.py:101-124`).
- Files/secrets: `fs` (`FsHandle`), `secrets` (`SecretsHandle`), `path(...)` ->
  `RemotePath` (`__init__.py:654-663`).
- Logs/metrics: `logs_stream(*, stream="workload")`, `logs(*, limit, since,
  stream)`, `runtime_status()`, `startup_info()`, `metrics()`
  (`__init__.py:665-684`).
- Fork/migrate: `fork(snapshot_id=None, *, name)`, `fork_with_progress(...)`,
  `migrate(*, target_node_id)`, `cancel_migration(id)`, `migration()`
  (`__init__.py:686-710`).
- Streams: `subscribe_status()` -> `VmStatusStream`, `console_connect()` /
  `port_forward(port)` -> `StreamConnection` (`__init__.py:712-719`).
- `async with branch:` deletes on exit (`__init__.py:721-730`).

### `Snapshot` (`__init__.py:807-877`)

Wraps `_RawCommit` (the native type is `Commit`). `Snapshot.from_oci(image,
*, token, base_url, region, ...)` is a one-call helper that builds a `Client`
and calls `build_snapshot_from_oci` (`__init__.py:812-832`). Read-only props
mirror the `.pyi` `Commit` (`id`, `branch_id`, `parent_id`, `status`,
`memory_mib`, `manifest_key`, `created_at_millis`; `__init__.py:834-860`).
`branch(*, name)` boots a fresh branch, `branch_with_progress(...)` returns a
`StartProgress`, `fork(*, name)` makes a divergent branch
(`__init__.py:866-877`).

### `VM` (`__init__.py:1120-1291`)

The opinionated high-level handle. `VM.start(snapshot, *, name,
stream_output=True)` returns an awaitable `_VMContext` that boots a branch and
(optionally) streams workload logs to stdout; `VM.attach(vm_id, ...)` and
`VM.get(name, ...)` adopt an existing VM (`__init__.py:1140-1180`). It forwards
`exec`/`bash`/`spawn`, `read`/`write`/`read_bytes`/`write_bytes`/`list`, and
`fork`/`snapshot`/`info` to the underlying `Branch`/`FsHandle`
(`__init__.py:1184-1245`). `close()` cancels the log task then deletes the
branch, shielded to completion (`__init__.py:1256-1288`, `1109-1118`); `wait()`
blocks until SIGINT/SIGTERM (`__init__.py:1249-1254`).

### Files: `FsHandle` and `RemotePath`

`FsHandle` (`__init__.py:416-516`) wraps `_RawFsHandle` with text/bytes helpers
(`read_text`, `write_text`, `read_bytes`, `write_bytes`, `list`) and an async
`open(path, mode, ...)` that buffers the remote file in a `_VmBytesBuffer`
(`io.BytesIO` subclass) and flushes dirty bytes back on context exit
(`__init__.py:244-338`, `466-485`). `_parse_open_mode` enforces Python-style
mode strings and rejects exclusive `x` (`__init__.py:208-241`). `RemotePath`
(`__init__.py:343-411`) is a `PurePosixPath`-backed, `os.PathLike` remote path
with `open`/`read_text`/`write_text`/`read_bytes`/`write_bytes`.

### Progress, streams, secrets

- Progress handles iterate `ProgressEvent`s then yield the final `Branch`:
  `_ProgressBase` / `_Progress` and the concrete `CreateProgress`,
  `StartProgress`, `ForkProgress` (`__init__.py:738-798`). `ProgressEvent` is a
  dataclass built from the native event via `from_native`
  (`__init__.py:146-172`).
- `StreamConnection` wraps the raw duplex stream with `read`/`write`/`close` and
  an async context manager (`__init__.py:521-543`).
- `SecretsHandle.set/delete/list` (`__init__.py:548-559`).

### Errors

`IxError(RuntimeError)` plus subclasses `IxAuthError`, `IxNotFoundError`,
`IxValidationError`, `IxRateLimitError`, `IxConflictError`, `IxCapacityError`,
`IxPaymentError`, `IxUnavailableError`, `IxConnectionError`, `IxTimeoutError`,
all imported from `_ix_sdk` (`__init__.py:69-79`, `_ix_sdk.pyi:19-29`).
`CommandError(RuntimeError)` carries `command`, `exit_code`, `stdout`, `stderr`
and a stderr-tail message for failed `exec`/`bash` (`__init__.py:101-124`).

## Regions and env

`Region` is a `str` type alias; `DEFAULT_REGION = "us-west-1"`,
`DEFAULT_CREATE_IPV4 = False` (`__init__.py:175-177`). `_default_region()` reads
`IX_REGION`, falling back to `DEFAULT_REGION` (`__init__.py:801-802`).

## Type stub drift (`_ix_sdk.pyi`)

The stub mirrors `crates/ix/sdk-py/src/` by hand and intentionally omits
native methods the wrapper does not call (`_ix_sdk.pyi:1-9`). It is the type
contract for the native `Client`, `Branch`, `Commit`, handles, streams,
enums (`BranchStatus`, `RuntimeState`, `MigrationPhase`, ...), and data classes
(`BranchInfo`, `MetricsInfo`, `RuntimeStatusInfo`, ...). Keep it in sync when
the Rust binding changes.

## How it is built and consumed

`sdk/python` itself has no `default.nix`; it is the source of the wheel built in
the private repo. The in-repo Nix consumer is
[packaging](../packaging/overview.md) (`packages/ix-sdk-python`), which fetches
the prebuilt wheel from R2 and exposes `pkgs.ix-sdk-python` /
`nix build .#ix-sdk-python`. That package's import test asserts a surface
(`Group`, `GroupMember`, `create_group`, `add_group_member`) that the wheel
provides but this source-available `ix_sdk/__init__.py` does not currently
expose; see [packaging](../packaging/overview.md) and the domain NOTES.
