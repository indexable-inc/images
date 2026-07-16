"""Bounded subprocess backends for the supported production harnesses."""

from __future__ import annotations

import asyncio
import contextlib
import os
import signal
import tempfile
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

_ERROR_TAIL_BYTES = 8 * 1024
_STOP_GRACE_SECONDS = 10.0
_KILL_GRACE_SECONDS = 5.0


class Backend(Protocol):
    """One live agent process."""

    async def result(self) -> str: ...

    async def interrupt(self) -> None: ...

    async def close(self) -> None: ...


class AgentExitError(RuntimeError):
    """The agent process exited without a usable final answer."""


@dataclass(frozen=True)
class BackendSpec:
    harness: str
    model: str
    effort: str | None
    cwd: Path
    max_result_bytes: int


def child_environment(source: Mapping[str, str]) -> dict[str, str]:
    """Keep workflow/provider credentials but withhold record-plane authority."""

    return {
        key: value
        for key, value in source.items()
        if key not in {"WEAVE_IDENTITY", "WEAVE_TOKEN", "WEAVE_URL"}
    }


def validate_provider_credential(harness: str, environ: Mapping[str, str]) -> None:
    """Fail before recording a call that cannot authenticate its model."""

    names = {
        "claude": ("ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"),
        "codex": ("OPENAI_API_KEY",),
    }[harness]
    if not any(environ.get(name) for name in names):
        joined = " or ".join(names)
        raise RuntimeError(f"{harness} requires {joined} in the runner environment")


def argv_for(spec: BackendSpec, result_file: Path) -> list[str]:
    """Return the non-secret argv; the prompt is always delivered on stdin."""

    if spec.harness == "claude":
        return [
            "claude",
            "-p",
            "--no-session-persistence",
            "--model",
            spec.model,
        ]
    effort = spec.effort or "medium"
    return [
        "codex",
        "exec",
        "--skip-git-repo-check",
        "--ephemeral",
        "-c",
        f"model_reasoning_effort={effort!r}",
        "--model",
        spec.model,
        "--sandbox",
        "danger-full-access",
        "--output-last-message",
        os.fspath(result_file),
        "-",
    ]


def _read_bounded(path: Path, limit: int, *, label: str) -> bytes:
    size = path.stat().st_size
    if size > limit:
        raise AgentExitError(f"{label} is {size} bytes; limit is {limit} bytes")
    return path.read_bytes()


def _read_tail(path: Path, limit: int = _ERROR_TAIL_BYTES) -> str:
    with path.open("rb") as stream:
        stream.seek(0, os.SEEK_END)
        size = stream.tell()
        stream.seek(max(0, size - limit))
        return stream.read().decode(errors="replace").strip()


def _signal_group(process: asyncio.subprocess.Process, signum: signal.Signals) -> None:
    """Signal a still-live process group, tolerating an exit between checks."""

    with contextlib.suppress(ProcessLookupError):
        os.killpg(process.pid, signum)


class ProcessBackend:
    """A process group with disk-spooled output and bounded final reads."""

    def __init__(
        self,
        *,
        spec: BackendSpec,
        tempdir: tempfile.TemporaryDirectory[str],
        process: asyncio.subprocess.Process,
        stdout_path: Path,
        stderr_path: Path,
        result_path: Path,
    ) -> None:
        self._spec = spec
        self._tempdir = tempdir
        self._process = process
        self._stdout_path = stdout_path
        self._stderr_path = stderr_path
        self._result_path = result_path
        self._interrupted = False

    @classmethod
    async def spawn(
        cls,
        spec: BackendSpec,
        prompt: bytes,
        *,
        environ: Mapping[str, str],
    ) -> ProcessBackend:
        tempdir = tempfile.TemporaryDirectory(prefix="fabric-agent-run-")
        root = Path(tempdir.name)
        stdout_path = root / "stdout.log"
        stderr_path = root / "stderr.log"
        result_path = root / "result.txt"
        argv = argv_for(spec, result_path)
        process: asyncio.subprocess.Process | None = None
        try:
            with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
                process = await asyncio.create_subprocess_exec(
                    *argv,
                    stdin=asyncio.subprocess.PIPE,
                    stdout=stdout,
                    stderr=stderr,
                    cwd=spec.cwd,
                    env=child_environment(environ),
                    start_new_session=True,
                )
            if process.stdin is None:
                raise RuntimeError("agent process has no stdin pipe")
            process.stdin.write(prompt)
            await process.stdin.drain()
            process.stdin.close()
            await process.stdin.wait_closed()
        except BaseException:
            if process is not None and process.returncode is None:
                _signal_group(process, signal.SIGKILL)
                await process.wait()
            tempdir.cleanup()
            raise
        return cls(
            spec=spec,
            tempdir=tempdir,
            process=process,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
            result_path=result_path,
        )

    async def result(self) -> str:
        code = await self._process.wait()
        if code != 0:
            detail = _read_tail(self._stderr_path) or _read_tail(self._stdout_path)
            suffix = f": {detail}" if detail else ""
            raise AgentExitError(f"{self._spec.harness} exited {code}{suffix}")
        path = self._stdout_path if self._spec.harness == "claude" else self._result_path
        if not path.exists():
            raise AgentExitError(f"{self._spec.harness} wrote no final result")
        body = _read_bounded(path, self._spec.max_result_bytes, label="agent result")
        return body.decode(errors="replace").strip()

    async def interrupt(self) -> None:
        if self._interrupted or self._process.returncode is not None:
            return
        self._interrupted = True
        _signal_group(self._process, signal.SIGINT)
        try:
            await asyncio.wait_for(
                asyncio.shield(self._process.wait()),
                timeout=_STOP_GRACE_SECONDS,
            )
            return
        except TimeoutError:
            _signal_group(self._process, signal.SIGTERM)
        try:
            await asyncio.wait_for(
                asyncio.shield(self._process.wait()),
                timeout=_KILL_GRACE_SECONDS,
            )
        except TimeoutError:
            _signal_group(self._process, signal.SIGKILL)
            await self._process.wait()

    async def close(self) -> None:
        if self._process.returncode is None:
            await self.interrupt()
        self._tempdir.cleanup()


async def spawn_backend(
    spec: BackendSpec,
    prompt: bytes,
    *,
    environ: Mapping[str, str],
) -> Backend:
    return await ProcessBackend.spawn(spec, prompt, environ=environ)


def supported_harnesses() -> Sequence[str]:
    return ("claude", "codex")
