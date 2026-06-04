from __future__ import annotations

import abc
import asyncio
import dataclasses
import io
import os
import pathlib
import signal
import posixpath
import sys
import typing

from ._ix_sdk import Branch as _RawBranch
from ._ix_sdk import BranchInfo
from ._ix_sdk import BranchStatus
from ._ix_sdk import Client as _RawClient
from ._ix_sdk import Commit as _RawCommit
from ._ix_sdk import ExecChunk as ExecChunk
from ._ix_sdk import ExecOutputStream
from ._ix_sdk import ExecResult
from ._ix_sdk import LogOutputStream
from ._ix_sdk import FsEntry
from ._ix_sdk import FsHandle as _RawFsHandle
from ._ix_sdk import FsReadResult
from ._ix_sdk import FsWriteResult
from ._ix_sdk import LogEntry
from ._ix_sdk import RegionInfo
from ._ix_sdk import RuntimeCaptureHealthInfo
from ._ix_sdk import RuntimeCaptureIssue
from ._ix_sdk import RuntimeControlHealthInfo
from ._ix_sdk import RuntimeControlIssue
from ._ix_sdk import RuntimeHealthInfo
from ._ix_sdk import RuntimeHealthState
from ._ix_sdk import RuntimeNetworkHealthInfo
from ._ix_sdk import RuntimeNetworkIssue
from ._ix_sdk import StartupInfo
from ._ix_sdk import StartupMode
from ._ix_sdk import StartupStagesInfo
from ._ix_sdk import RuntimeState
from ._ix_sdk import RuntimeStatusInfo
from ._ix_sdk import RuntimeVcpuHealthInfo
from ._ix_sdk import RuntimeVcpuIssue
from ._ix_sdk import RuntimeVcpuIssueKind
from ._ix_sdk import RuntimeVirtioMemHealthInfo
from ._ix_sdk import RuntimeVirtioMemIssue
from ._ix_sdk import Secret
from ._ix_sdk import SecretsHandle as _RawSecretsHandle
from ._ix_sdk import StreamConnection as _RawStreamConnection
from ._ix_sdk import ApiToken
from ._ix_sdk import BillingStatus
from ._ix_sdk import MetricsInfo
from ._ix_sdk import MigrationInfo
from ._ix_sdk import MigrationPhase
from ._ix_sdk import MigrationStart
from ._ix_sdk import ObservabilityLogEntry
from ._ix_sdk import PreviewDetail
from ._ix_sdk import PreviewInfo
from ._ix_sdk import TraceSummary
from ._ix_sdk import UserInfo
from ._ix_sdk import VolumeInfo
from ._ix_sdk import VolumeSnapshot
from ._ix_sdk import __version__
from ._ix_sdk import ProgressEvent as _NativeProgressEvent
from ._ix_sdk import VmStatusEvent as VmStatusEvent
from ._ix_sdk import VmStatusStream

# Structured exception classes — all subclass RuntimeError via IxError
from ._ix_sdk import IxError
from ._ix_sdk import IxAuthError
from ._ix_sdk import IxNotFoundError
from ._ix_sdk import IxValidationError
from ._ix_sdk import IxRateLimitError
from ._ix_sdk import IxConflictError
from ._ix_sdk import IxCapacityError
from ._ix_sdk import IxPaymentError
from ._ix_sdk import IxUnavailableError
from ._ix_sdk import IxConnectionError
from ._ix_sdk import IxTimeoutError



def _fmt_command(command: str | list[str]) -> str:
    if isinstance(command, list):
        text = " ".join(command)
    else:
        text = command
    text = text.replace("\n", " ").strip()
    if len(text) > 120:
        text = text[:117] + "..."
    return text


def _tail_lines(text: str, n: int) -> str:
    lines = text.rstrip("\n").splitlines()
    if len(lines) <= n:
        return "\n".join(lines)
    return "... (" + str(len(lines) - n) + " earlier lines) ...\n" + "\n".join(lines[-n:])


class CommandError(RuntimeError):
    def __init__(
        self,
        command: str | list[str],
        exit_code: int,
        stdout: str,
        stderr: str,
        *,
        streamed: bool,
    ) -> None:
        self.command = command
        self.exit_code = exit_code
        self.stdout = stdout
        self.stderr = stderr
        preview = _fmt_command(command)
        header = f"command exited with code {exit_code}: {preview}"
        if streamed:
            super().__init__(f"{header} (stdout/stderr streamed above)")
        else:
            tail = _tail_lines(stderr, 20) if stderr else ""
            if tail:
                super().__init__(f"{header}\nstderr (last 20 lines):\n{tail}")
            else:
                super().__init__(header)


async def _stream_to_result(stream: ExecOutputStream) -> ExecResult:
    stdout_parts: list[bytes] = []
    stderr_parts: list[bytes] = []
    async for chunk in stream:
        if chunk.stdout:
            sys.stdout.buffer.write(chunk.stdout)
            sys.stdout.buffer.flush()
            stdout_parts.append(bytes(chunk.stdout))
        if chunk.stderr:
            sys.stderr.buffer.write(chunk.stderr)
            sys.stderr.buffer.flush()
            stderr_parts.append(bytes(chunk.stderr))
    return ExecResult(
        stream.exit_code or 0,
        b"".join(stdout_parts).decode(errors="replace"),
        b"".join(stderr_parts).decode(errors="replace"),
    )


@dataclasses.dataclass
class ProgressEvent:
    stage: str
    detail: str
    file_count: int | None = None
    block_count: int | None = None
    total_bytes: int | None = None
    from_cache: bool | None = None
    item_count: int | None = None
    cached: bool | None = None
    elapsed_ms: int | None = None
    vm_id: str | None = None

    @classmethod
    def from_native(cls, native: _NativeProgressEvent) -> "ProgressEvent":
        return cls(
            stage=native.stage,
            detail=native.detail,
            file_count=native.file_count,
            block_count=native.block_count,
            total_bytes=native.total_bytes,
            from_cache=native.from_cache,
            item_count=native.item_count,
            cached=native.cached,
            elapsed_ms=native.elapsed_ms,
            vm_id=native.vm_id,
        )


Region: typing.TypeAlias = str
DEFAULT_REGION: Region = "us-west-1"
DEFAULT_CREATE_IPV4 = False


def _normalize_path(path: str | os.PathLike[str]) -> str:
    normalized = os.fspath(path)
    if not normalized:
        raise ValueError("path must not be empty")
    return normalized


def _region_slug(region: object) -> str:
    if not isinstance(region, str):
        raise TypeError("region must be a slug string")
    if not region:
        raise ValueError("region must not be empty")
    return region


# ── File I/O internals ──────────────────────────────────────────────

@dataclasses.dataclass(frozen=True, slots=True)
class _FsOpenMode:
    raw: str
    binary: bool
    readable: bool
    writable: bool
    append: bool
    truncate: bool
    create: bool


def _parse_open_mode(mode: str) -> _FsOpenMode:
    if not mode:
        raise ValueError("mode must not be empty")

    if "x" in mode:
        raise ValueError("exclusive create mode is not supported")

    unsupported = set(mode) - set("rabt+w")
    if unsupported:
        unsupported_mode = "".join(sorted(unsupported))
        raise ValueError(f"unsupported mode characters: {unsupported_mode!r}")

    if mode.count("+") > 1:
        raise ValueError(f"invalid mode: {mode!r}")

    if "b" in mode and "t" in mode:
        raise ValueError(f"can't have text and binary mode at once: {mode!r}")

    base_modes = [part for part in mode if part in "raw"]
    if len(base_modes) != 1:
        raise ValueError(f"must have exactly one of r, a, or w: {mode!r}")

    base_mode = base_modes[0]
    updating = "+" in mode

    return _FsOpenMode(
        raw=mode,
        binary="b" in mode,
        readable=base_mode == "r" or updating,
        writable=base_mode in {"a", "w"} or updating,
        append=base_mode == "a",
        truncate=base_mode == "w",
        create=base_mode in {"a", "w"},
    )


class _VmBytesBuffer(io.BytesIO):
    """In-memory buffer backed by a remote VM file.

    Reads/writes are sync against the buffer. The async flush writes
    dirty data back to the remote VM.
    """

    def __init__(
        self,
        fs: FsHandle,
        path: str,
        initial_bytes: bytes,
        open_mode: _FsOpenMode,
        file_mode: int | None,
    ) -> None:
        super().__init__(initial_bytes)
        self._fs = fs
        self._path = path
        self._open_mode = open_mode
        self._file_mode = file_mode
        self._dirty = False
        if open_mode.append:
            self.seek(0, io.SEEK_END)

    @property
    def mode(self) -> str:
        return self._open_mode.raw

    @property
    def name(self) -> str:
        return self._path

    def readable(self) -> bool:
        return self._open_mode.readable

    def writable(self) -> bool:
        return self._open_mode.writable

    def write(self, data: bytes | bytearray | memoryview) -> int:  # pyright: ignore[reportIncompatibleMethodOverride]
        if self._open_mode.append:
            self.seek(0, io.SEEK_END)
        self._dirty = True
        return super().write(data)

    def truncate(self, size: int | None = None) -> int:
        self._dirty = True
        return super().truncate(size)

    async def aflush(self) -> None:
        if self.closed or not self._dirty or not self._open_mode.writable:
            return
        await self._fs.write_bytes(self._path, self.getvalue(), mode=self._file_mode)
        self._dirty = False


class _AsyncFileContext:
    """Async context manager wrapping a _VmBytesBuffer."""

    def __init__(self, buf: _VmBytesBuffer) -> None:
        self._buf = buf

    async def __aenter__(self) -> _VmBytesBuffer:
        return self._buf

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: typing.Any,
    ) -> None:
        await self._buf.aflush()
        self._buf.close()


class _AsyncTextFileContext:
    """Async context manager wrapping an io.TextIOWrapper over a _VmBytesBuffer."""

    def __init__(self, wrapper: io.TextIOWrapper, buf: _VmBytesBuffer, path: str, mode: str) -> None:
        self._wrapper = wrapper
        self._buf = buf
        self.name = path
        self.mode = mode

    async def __aenter__(self) -> io.TextIOWrapper:
        return self._wrapper

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: typing.Any,
    ) -> None:
        self._wrapper.flush()
        await self._buf.aflush()
        self._wrapper.close()


# ── RemotePath ──────────────────────────────────────────────────────

class RemotePath:
    __slots__ = ("_fs", "_path")

    def __init__(self, fs: FsHandle, path: pathlib.PurePosixPath) -> None:
        self._fs = fs
        self._path = path

    def __str__(self) -> str:
        return self.as_posix()

    def __fspath__(self) -> str:
        return self.as_posix()

    def __truediv__(self, other: str | os.PathLike[str]) -> "RemotePath":
        return RemotePath(self._fs, self._path / os.fspath(other))

    @property
    def name(self) -> str:
        return self._path.name

    @property
    def parent(self) -> "RemotePath":
        return RemotePath(self._fs, self._path.parent)

    def as_posix(self) -> str:
        return self._path.as_posix()

    async def open(
        self,
        mode: str = "r",
        *,
        encoding: str = "utf-8",
        errors: str = "strict",
        newline: str | None = None,
        file_mode: int | None = None,
    ) -> _AsyncFileContext | _AsyncTextFileContext:
        return await self._fs.open(
            self,
            mode,
            encoding=encoding,
            errors=errors,
            newline=newline,
            file_mode=file_mode,
        )

    async def read_text(self, *, encoding: str = "utf-8", errors: str = "strict") -> str:
        data = await self._fs.read_bytes(self)
        return data.decode(encoding, errors)

    async def write_text(
        self,
        text: str,
        *,
        mode: int | None = None,
        encoding: str = "utf-8",
        errors: str = "strict",
    ) -> int:
        return await self._fs.write_bytes(self, text.encode(encoding, errors), mode=mode)

    async def read_bytes(self) -> bytes:
        return await self._fs.read_bytes(self)

    async def write_bytes(
        self,
        data: bytes | bytearray | memoryview,
        *,
        mode: int | None = None,
    ) -> int:
        return await self._fs.write_bytes(self, data, mode=mode)


# ── FsHandle ────────────────────────────────────────────────────────

class FsHandle:
    def __init__(self, inner: _RawFsHandle) -> None:
        self._inner = inner

    async def read(self, path: str) -> FsReadResult:
        return await self._inner.read(path)

    async def write(self, path: str, text: str, *, mode: int | None = None) -> FsWriteResult:
        return await self._inner.write(path, text, mode)

    async def list(self, path: str) -> list[FsEntry]:
        return await self._inner.list(path)

    def path(self, path: str | os.PathLike[str]) -> RemotePath:
        return RemotePath(self, pathlib.PurePosixPath(_normalize_path(path)))

    async def read_text(
        self,
        path: str | os.PathLike[str],
        *,
        encoding: str = "utf-8",
        errors: str = "strict",
    ) -> str:
        data = await self.read_bytes(path)
        return data.decode(encoding, errors)

    async def write_text(
        self,
        path: str | os.PathLike[str],
        text: str,
        *,
        mode: int | None = None,
        encoding: str = "utf-8",
        errors: str = "strict",
    ) -> int:
        return await self.write_bytes(path, text.encode(encoding, errors), mode=mode)

    async def read_bytes(self, path: str | os.PathLike[str]) -> bytes:
        return bytes(await self._inner.read_all_bytes(_normalize_path(path)))

    async def write_bytes(
        self,
        path: str | os.PathLike[str],
        data: bytes | bytearray | memoryview,
        *,
        mode: int | None = None,
    ) -> int:
        normalized_path = _normalize_path(path)
        return int(await self._inner.write_bytes(normalized_path, bytes(data), mode))

    async def open(
        self,
        path: str | os.PathLike[str],
        mode: str = "r",
        *,
        encoding: str = "utf-8",
        errors: str = "strict",
        newline: str | None = None,
        file_mode: int | None = None,
    ) -> _AsyncFileContext | _AsyncTextFileContext:
        normalized_path = _normalize_path(path)
        open_mode = _parse_open_mode(mode)
        initial_bytes = await self._initial_bytes_for_open(normalized_path, open_mode)
        raw_file = _VmBytesBuffer(self, normalized_path, initial_bytes, open_mode, file_mode)

        if open_mode.binary:
            return _AsyncFileContext(raw_file)

        wrapper = io.TextIOWrapper(raw_file, encoding=encoding, errors=errors, newline=newline)
        return _AsyncTextFileContext(wrapper, raw_file, normalized_path, open_mode.raw)

    async def _initial_bytes_for_open(self, path: str, open_mode: _FsOpenMode) -> bytes:
        entry = await self._lookup_entry(path)
        if entry is not None and entry.is_dir:
            raise IsADirectoryError(path)

        if open_mode.truncate:
            return b""

        if open_mode.create and entry is None:
            return b""

        if entry is None:
            raise FileNotFoundError(path)

        return await self.read_bytes(path)

    async def _lookup_entry(self, path: str) -> FsEntry | None:
        if path == "/":
            return FsEntry(name="/", size=0, mode=0o040755, mtime_ns=0, is_dir=True)

        directory = posixpath.dirname(path) or "."
        name = posixpath.basename(path)
        if not name:
            return FsEntry(name=path, size=0, mode=0o040755, mtime_ns=0, is_dir=True)

        for entry in await self.list(directory):
            if entry.name == name:
                return entry

        return None


# ── StreamConnection ───────────────────────────────────────────────

class StreamConnection:
    def __init__(self, inner: _RawStreamConnection) -> None:
        self._inner = inner

    async def read(self, n: int) -> bytes:
        return bytes(await self._inner.read(n))

    async def write(self, data: bytes | bytearray | memoryview) -> int:
        return await self._inner.write(bytes(data))

    async def close(self) -> None:
        await self._inner.close()

    async def __aenter__(self) -> typing.Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: typing.Any,
    ) -> None:
        await self.close()


# ── SecretsHandle ───────────────────────────────────────────────────

class SecretsHandle:
    def __init__(self, inner: _RawSecretsHandle) -> None:
        self._inner = inner

    async def set(self, key: str, value: str) -> None:
        await self._inner.set(key, value)

    async def delete(self, key: str) -> None:
        await self._inner.delete(key)

    async def list(self) -> list[Secret]:
        return await self._inner.list()


# ── Branch ──────────────────────────────────────────────────────────

class Branch:
    def __init__(self, inner: _RawBranch, client: "Client") -> None:
        self._inner = inner
        self._client = client
        self._fs = FsHandle(self._inner.fs())
        self._secrets = SecretsHandle(self._inner.secrets())

    @property
    def id(self) -> str:
        return self._inner.id

    async def info(self) -> BranchInfo:
        return await self._inner.info()

    async def delete(self) -> None:
        await self._inner.delete()

    async def pause(self) -> "Snapshot":
        return Snapshot(await self._inner.pause(), self._client)

    async def start(self) -> BranchInfo:
        return await self._inner.start()

    def start_with_progress(self) -> "StartProgress":
        return StartProgress(self._inner.start_with_progress(), self._client)

    async def restart(self) -> BranchInfo:
        return await self._inner.restart()

    async def snapshot(self) -> "Snapshot":
        return Snapshot(await self._inner.snapshot(), self._client)

    async def bash(
        self,
        script: str,
        *,
        working_dir: str | None = None,
        check: bool = True,
        quiet: bool = False,
    ) -> ExecResult:
        if quiet:
            result = await self._inner.bash(script, working_dir)
        else:
            result = await _stream_to_result(self._inner.bash_stream(script, working_dir))
        if check and result.exit_code != 0:
            raise CommandError(
                script,
                result.exit_code,
                result.stdout,
                result.stderr,
                streamed=not quiet,
            )
        return result

    async def spawn(self, command: list[str], *, working_dir: str | None = None) -> int:
        return await self._inner.spawn(command, working_dir)

    async def exec(
        self,
        command: list[str],
        *,
        working_dir: str | None = None,
        check: bool = True,
        quiet: bool = False,
    ) -> ExecResult:
        if quiet:
            result = await self._inner.exec(command, working_dir)
        else:
            result = await _stream_to_result(self._inner.exec_stream(command, working_dir))
        if check and result.exit_code != 0:
            raise CommandError(
                command,
                result.exit_code,
                result.stdout,
                result.stderr,
                streamed=not quiet,
            )
        return result

    def exec_stream(
        self,
        command: list[str],
        *,
        working_dir: str | None = None,
    ) -> "ExecOutputStream":
        return self._inner.exec_stream(command, working_dir)

    async def log(self) -> list["Snapshot"]:
        return [Snapshot(c, self._client) for c in await self._inner.log()]

    def path(self, path: str | os.PathLike[str]) -> RemotePath:
        return self._fs.path(path)

    @property
    def fs(self) -> FsHandle:
        return self._fs

    @property
    def secrets(self) -> SecretsHandle:
        return self._secrets

    def logs_stream(self, *, stream: str = "workload") -> LogOutputStream:
        return self._inner.logs_stream(stream)

    async def logs(
        self,
        *,
        limit: int,
        since: int | None = None,
        stream: str = "workload",
    ) -> list[LogEntry]:
        return await self._inner.logs(stream, limit, since)

    async def runtime_status(self) -> RuntimeStatusInfo | None:
        return await self._inner.runtime_status()

    async def startup_info(self) -> StartupInfo | None:
        return await self._inner.startup_info()

    async def metrics(self) -> MetricsInfo | None:
        return await self._inner.metrics()

    async def fork(
        self,
        snapshot_id: str | None = None,
        *,
        name: str | None = None,
    ) -> "Branch":
        raw = await self._inner.fork(snapshot_id, name)
        return Branch(raw, self._client)

    def fork_with_progress(
        self,
        snapshot_id: str,
        *,
        name: str | None = None,
    ) -> "ForkProgress":
        return ForkProgress(self._inner.fork_with_progress(snapshot_id, name), self._client)

    async def migrate(self, *, target_node_id: str | None = None) -> MigrationStart:
        return await self._inner.migrate(target_node_id)

    async def cancel_migration(self, migration_id: str) -> None:
        await self._inner.cancel_migration(migration_id)

    async def migration(self) -> MigrationInfo | None:
        return await self._inner.migration()

    def subscribe_status(self) -> VmStatusStream:
        return self._inner.subscribe_status()

    async def console_connect(self) -> StreamConnection:
        return StreamConnection(await self._inner.console_connect())

    async def port_forward(self, port: int) -> StreamConnection:
        return StreamConnection(await self._inner.port_forward(port))

    async def __aenter__(self) -> typing.Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: typing.Any,
    ) -> None:
        await self.delete()

    def __repr__(self) -> str:
        return f"Branch(id={self.id!r})"


# ── Progress handles ───────────────────────────────────────────────

class _ProgressBase(abc.ABC):
    """Base class for progress handles with streaming events."""

    _stream: typing.Any
    _result: "Branch | None"
    _exhausted: bool

    def __aiter__(self) -> typing.Self:
        return self

    async def __anext__(self) -> ProgressEvent:
        if self._exhausted:
            raise StopAsyncIteration
        item = await self._stream.__anext__()
        if isinstance(item, _NativeProgressEvent):
            return ProgressEvent.from_native(item)
        self._result = self._wrap_result(item)
        self._exhausted = True
        raise StopAsyncIteration

    @abc.abstractmethod
    def _wrap_result(self, item: typing.Any) -> "Branch": ...

    async def _drain(self) -> "Branch":
        if self._result is not None:
            return self._result
        async for _ in self:
            pass
        assert self._result is not None
        return self._result


class _Progress(_ProgressBase):
    """Concrete progress handle. Iterate for events, then call `.branch()`."""

    def __init__(self, stream: typing.Any, client: "Client") -> None:
        self._stream = stream
        self._client = client
        self._result: Branch | None = None
        self._exhausted = False

    def _wrap_result(self, item: typing.Any) -> Branch:
        return Branch(item, self._client)

    async def branch(self) -> "Branch":
        result = await self._drain()
        assert isinstance(result, Branch)
        return result


class CreateProgress(_Progress):
    pass


class StartProgress(_Progress):
    async def info(self) -> "BranchInfo":
        return await (await self.branch()).info()


class ForkProgress(_Progress):
    pass


def _default_region() -> Region:
    return os.environ.get("IX_REGION", DEFAULT_REGION)


# ── Snapshot ───────────────────────────────────────────────────────

class Snapshot:
    def __init__(self, inner: _RawCommit, client: "Client") -> None:
        self._inner = inner
        self._client = client

    @staticmethod
    async def from_oci(
        image: str,
        *,
        token: str | None = None,
        base_url: str | None = None,
        region: Region | None = None,
        name: str | None = None,
        env: dict[str, str] | None = None,
        ipv4: bool = DEFAULT_CREATE_IPV4,
        l7_proxy_ports: list[int] | None = None,
    ) -> "Snapshot":
        client = Client(token=token, base_url=base_url)
        return await client.build_snapshot_from_oci(
            image,
            region=region if region is not None else _default_region(),
            name=name,
            env=env,
            ipv4=ipv4,
            l7_proxy_ports=l7_proxy_ports,
        )

    @property
    def id(self) -> str:
        return self._inner.id

    @property
    def branch_id(self) -> str:
        return self._inner.branch_id

    @property
    def parent_id(self) -> str | None:
        return self._inner.parent_id

    @property
    def status(self) -> str:
        return self._inner.status

    @property
    def memory_mib(self) -> int:
        return self._inner.memory_mib

    @property
    def manifest_key(self) -> str | None:
        return self._inner.manifest_key

    @property
    def created_at_millis(self) -> int:
        return self._inner.created_at_millis

    @property
    def client(self) -> "Client":
        return self._client

    async def branch(self, *, name: str | None = None) -> Branch:
        return Branch(await self._inner.branch(name), self._client)

    def branch_with_progress(
        self,
        *,
        name: str | None = None,
    ) -> StartProgress:
        return StartProgress(self._inner.branch_with_progress(name), self._client)

    async def fork(self, *, name: str | None = None) -> Branch:
        return Branch(await self._inner.fork(name), self._client)


# ── Client ──────────────────────────────────────────────────────────

class Client:
    def __init__(self, *, token: str | None = None, base_url: str | None = None) -> None:
        self._inner = _RawClient(token, base_url)

    @property
    def base_url(self) -> str:
        return self._inner.base_url()

    async def get(self, branch_id: str) -> Branch:
        return Branch(await self._inner.get(branch_id), self)

    async def build_snapshot_from_oci(
        self,
        image: str,
        *,
        region: Region,
        name: str | None = None,
        env: dict[str, str] | None = None,
        l7_proxy_ports: list[int] | None = None,
        ipv4: bool = DEFAULT_CREATE_IPV4,
    ) -> Snapshot:
        return Snapshot(
            await self._inner.build_snapshot_from_oci(
                image,
                _region_slug(region),
                name,
                env,
                l7_proxy_ports,
                ipv4,
            ),
            self,
        )

    async def get_by_name(self, name: str) -> Branch:
        return Branch(await self._inner.get_by_name(name), self)

    async def find_by_name(self, name: str) -> Branch | None:
        try:
            return Branch(await self._inner.get_by_name(name), self)
        except IxNotFoundError:
            return None

    async def snapshot(self, *, name: str) -> BranchInfo:
        return await self._inner.snapshot(name)

    async def switch_system(
        self,
        *,
        name: str,
        target: str | None = None,
        system: str | None = None,
        build_on: typing.Literal["auto", "local", "remote"] = "auto",
        region: Region | None = None,
        env: dict[str, str] | None = None,
        l7_proxy_ports: list[int] | None = None,
        ipv4: bool | None = None,
    ) -> BranchInfo:
        del region, env, l7_proxy_ports, ipv4
        resolved_target = target if target is not None else system
        if resolved_target is None:
            raise ValueError("switch_system requires target or system")
        return await self._inner.switch_system(name, resolved_target, build_on)

    async def create(
        self,
        image: str,
        *,
        region: Region,
        name: str | None = None,
        env: dict[str, str] | None = None,
        l7_proxy_ports: list[int] | None = None,
        ipv4: bool = DEFAULT_CREATE_IPV4,
        on_progress: "typing.Callable[[ProgressEvent], None] | None" = None,
    ) -> Branch:
        progress = self.create_with_progress(
            image,
            region=region,
            name=name,
            env=env,
            l7_proxy_ports=l7_proxy_ports,
            ipv4=ipv4,
        )
        if on_progress:
            async for event in progress:
                on_progress(event)
        return await progress.branch()

    def create_with_progress(
        self,
        image: str,
        *,
        region: Region,
        name: str | None = None,
        env: dict[str, str] | None = None,
        l7_proxy_ports: list[int] | None = None,
        ipv4: bool = DEFAULT_CREATE_IPV4,
    ) -> "CreateProgress":
        stream = self._inner.create_with_progress(
            image,
            _region_slug(region),
            name,
            env,
            l7_proxy_ports,
            ipv4,
        )
        return CreateProgress(stream, self)

    async def branches(self) -> list[BranchInfo]:
        return await self._inner.branches()

    async def regions(self) -> list[RegionInfo]:
        return await self._inner.regions()

    async def me(self) -> UserInfo | None:
        return await self._inner.me()

    async def current_username(self) -> str:
        user = await self.me()
        if user is None:
            raise RuntimeError("ix API returned no authenticated user")
        return user.username

    async def billing_status(self) -> BillingStatus:
        return await self._inner.billing_status()

    async def list_api_tokens(self) -> list[ApiToken]:
        return await self._inner.list_api_tokens()

    async def revoke_api_token(self, token_id: str) -> None:
        await self._inner.revoke_api_token(token_id)

    async def get_volume(self, volume_id: str) -> VolumeInfo:
        return await self._inner.get_volume(volume_id)

    async def list_volumes(self) -> list[VolumeInfo]:
        return await self._inner.list_volumes()

    async def list_volume_snapshots(self, volume_id: str) -> list[VolumeSnapshot]:
        return await self._inner.list_volume_snapshots(volume_id)

    async def create_preview(
        self,
        *,
        image_tag: str | None = None,
        fork_volumes: bool | None = None,
    ) -> PreviewDetail:
        return await self._inner.create_preview(image_tag, fork_volumes)

    async def list_previews(self) -> list[PreviewInfo]:
        return await self._inner.list_previews()

    async def stop_preview(self, preview_id: str) -> None:
        await self._inner.stop_preview(preview_id)

    async def get_preview(self, preview_id: str) -> PreviewDetail:
        """Fetch details of a single preview by id."""
        return await self._inner.get_preview(preview_id)

    async def promote_preview(self, preview_id: str) -> PreviewDetail:
        """Promote a preview to a full VM."""
        return await self._inner.promote_preview(preview_id)

    async def query_logs(
        self,
        *,
        limit: int,
        trace_id: str | None = None,
        request_id: str | None = None,
        since: int | None = None,
        until: int | None = None,
    ) -> list[ObservabilityLogEntry]:
        return await self._inner.query_logs(limit, trace_id, request_id, since, until)

    async def search_traces(
        self,
        *,
        limit: int,
        trace_id: str | None = None,
        request_id: str | None = None,
        since: int | None = None,
        until: int | None = None,
    ) -> list[TraceSummary]:
        return await self._inner.search_traces(limit, trace_id, request_id, since, until)

    def __repr__(self) -> str:
        return f"Client(base_url={self.base_url!r})"


# ── VM ─────────────────────────────────────────────────────────────

async def _print_log_stream(stream: LogOutputStream) -> None:
    try:
        async for chunk in stream:
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
    except asyncio.CancelledError:
        pass


class _VMContext:
    def __init__(self, coro: typing.Coroutine[typing.Any, typing.Any, "VM"]) -> None:
        self._task: asyncio.Task[VM] | None = None
        self._coro = coro
        self._vm: VM | None = None

    def _ensure_task(self) -> asyncio.Task["VM"]:
        if self._task is None:
            self._task = asyncio.ensure_future(self._coro)
        return self._task

    def __await__(self) -> typing.Generator[typing.Any, None, "VM"]:
        return self._ensure_task().__await__()

    async def __aenter__(self) -> "VM":
        self._vm = await self._ensure_task()
        return self._vm

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: typing.Any,
    ) -> None:
        assert self._vm is not None
        await _await_shielded_to_completion(self._vm.close())


async def _await_shielded_to_completion(
    awaitable: typing.Awaitable[typing.Any],
) -> typing.Any:
    task = asyncio.ensure_future(awaitable)
    try:
        return await asyncio.shield(task)
    except asyncio.CancelledError:
        await asyncio.shield(task)
        raise


class VM:
    def __init__(
        self,
        client: Client,
        branch: Branch,
        *,
        _log_task: asyncio.Task[None] | None = None,
    ) -> None:
        self._client = client
        self._branch = branch
        self._log_task = _log_task

    @property
    def id(self) -> str:
        return self._branch.id

    @property
    def branch(self) -> Branch:
        return self._branch

    @staticmethod
    def start(
        snapshot: Snapshot,
        *,
        name: str | None = None,
        stream_output: bool = True,
    ) -> _VMContext:
        async def _create() -> "VM":
            branch = await snapshot.branch(name=name)
            log_task: asyncio.Task[None] | None = None
            if stream_output:
                log_stream = branch.logs_stream(stream="workload")
                log_task = asyncio.create_task(_print_log_stream(log_stream))
            return VM(snapshot.client, branch, _log_task=log_task)
        return _VMContext(_create())

    @staticmethod
    def attach(
        vm_id: str,
        *,
        token: str | None = None,
        base_url: str | None = None,
    ) -> _VMContext:
        async def _create() -> "VM":
            client = Client(token=token, base_url=base_url)
            branch = await client.get(vm_id)
            return VM(client, branch)
        return _VMContext(_create())

    @staticmethod
    async def get(
        name: str,
        *,
        token: str | None = None,
        base_url: str | None = None,
    ) -> "VM | None":
        client = Client(token=token, base_url=base_url)
        branch = await client.find_by_name(name)
        if branch is None:
            return None
        return VM(client, branch)

    # ── Info ─────────────────────────────────────────────────────

    async def info(self) -> BranchInfo:
        return await self._branch.info()

    async def snapshot(self) -> Snapshot:
        return await self._branch.snapshot()

    # ── Execution ───────────────────────────────────────────────

    async def exec(
        self,
        command: list[str],
        *,
        working_dir: str | None = None,
        check: bool = True,
        quiet: bool = False,
    ) -> ExecResult:
        return await self._branch.exec(
            command, working_dir=working_dir, check=check, quiet=quiet,
        )

    async def bash(
        self,
        script: str,
        *,
        working_dir: str | None = None,
        check: bool = True,
        quiet: bool = False,
    ) -> ExecResult:
        return await self._branch.bash(
            script, working_dir=working_dir, check=check, quiet=quiet,
        )

    async def spawn(
        self,
        command: list[str],
        *,
        working_dir: str | None = None,
    ) -> int:
        return await self._branch.spawn(command, working_dir=working_dir)

    # ── Files ───────────────────────────────────────────────────

    async def read(self, path: str) -> str:
        return await self._branch.fs.read_text(path)

    async def write(self, path: str, text: str) -> int:
        return await self._branch.fs.write_text(path, text)

    async def read_bytes(self, path: str) -> bytes:
        return await self._branch.fs.read_bytes(path)

    async def write_bytes(self, path: str, data: bytes | bytearray | memoryview) -> int:
        return await self._branch.fs.write_bytes(path, data)

    async def list(self, path: str) -> list[FsEntry]:
        return await self._branch.fs.list(path)

    # ── Forking ─────────────────────────────────────────────────

    async def fork(self, name: str | None = None) -> "VM":
        forked = await self._branch.fork(name=name)
        return VM(self._client, forked)

    # ── Lifecycle ───────────────────────────────────────────────

    async def wait(self) -> None:
        shutdown = asyncio.Event()
        loop = asyncio.get_running_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            loop.add_signal_handler(sig, shutdown.set)
        await shutdown.wait()

    async def close(self) -> None:
        log_task_error: BaseException | None = None
        if self._log_task is not None:
            log_task = self._log_task
            self._log_task = None
            log_task.cancel()
            try:
                await log_task
            except asyncio.CancelledError:
                pass
            except BaseException as error:
                log_task_error = error

        try:
            await self._branch.delete()
        except BaseException as delete_error:
            if log_task_error is not None:
                delete_error.add_note(f"log stream task also failed: {log_task_error!r}")
            raise

        if log_task_error is not None:
            raise log_task_error

    async def __aenter__(self) -> typing.Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: typing.Any,
    ) -> None:
        await _await_shielded_to_completion(self.close())

    def __repr__(self) -> str:
        return f"VM(id={self.id!r})"


__all__ = [
    "ApiToken",
    "BillingStatus",
    "Branch",
    "BranchInfo",
    "BranchStatus",
    "Client",
    "CommandError",
    "Snapshot",
    "CreateProgress",
    "ExecResult",
    "ForkProgress",
    "FsEntry",
    "FsHandle",
    "FsReadResult",
    "FsWriteResult",
    "LogEntry",
    "LogOutputStream",
    "MetricsInfo",
    "MigrationInfo",
    "MigrationPhase",
    "MigrationStart",
    "ObservabilityLogEntry",
    "PreviewDetail",
    "PreviewInfo",
    "ProgressEvent",
    "Region",
    "DEFAULT_REGION",
    "DEFAULT_CREATE_IPV4",
    "RegionInfo",
    "RemotePath",
    "RuntimeCaptureHealthInfo",
    "RuntimeCaptureIssue",
    "RuntimeControlHealthInfo",
    "RuntimeControlIssue",
    "RuntimeHealthInfo",
    "RuntimeHealthState",
    "RuntimeNetworkHealthInfo",
    "RuntimeNetworkIssue",
    "RuntimeState",
    "RuntimeStatusInfo",
    "RuntimeVcpuHealthInfo",
    "RuntimeVcpuIssue",
    "RuntimeVcpuIssueKind",
    "RuntimeVirtioMemHealthInfo",
    "RuntimeVirtioMemIssue",
    "VM",
    "IxError",
    "IxAuthError",
    "IxNotFoundError",
    "IxValidationError",
    "IxRateLimitError",
    "IxConflictError",
    "IxCapacityError",
    "IxPaymentError",
    "IxUnavailableError",
    "IxConnectionError",
    "IxTimeoutError",
    "Secret",
    "SecretsHandle",
    "StartProgress",
    "StartupInfo",
    "StartupMode",
    "StartupStagesInfo",
    "StreamConnection",
    "TraceSummary",
    "UserInfo",
    "VolumeInfo",
    "VolumeSnapshot",
    "__version__",
]
