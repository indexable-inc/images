"""Claude Agent SDK sessions, self-recorded to the weave journal.

``s = await claude.session(prompt)`` opens a live, interruptible Claude
session on the Claude Agent SDK (streaming mode: native ``interrupt()``,
follow-up input via ``send()``), bound to one weave task entity. Every
message and the final result go to CAS; facts carry only pointers, never
payloads. Two interrupt paths converge on the SDK interrupt and both leave
``state=interrupted``: ``s.interrupt()``, and an ``interrupt=requested``
fact asserted on the task entity by anyone watching the journal.
"""

from __future__ import annotations

import asyncio
import dataclasses
import json
import platform
from typing import TYPE_CHECKING, Protocol

import weave

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Sequence

    from claude_agent_sdk import Message, PermissionMode

__all__ = ["Session", "session"]


class SdkClient(Protocol):
    """The slice of ``claude_agent_sdk.ClaudeSDKClient`` a session drives.

    A structural type so tests can stand in a fake without touching the real
    SDK subprocess transport.
    """

    async def connect(self, prompt: str | None = None) -> None: ...

    async def query(self, prompt: str, session_id: str = "default") -> None: ...

    def receive_messages(self) -> AsyncIterator[Message]: ...

    async def interrupt(self) -> None: ...

    async def disconnect(self) -> None: ...


def _sdk_client(
    *,
    system_prompt: str | None,
    model: str | None,
    cwd: str | None,
    allowed_tools: Sequence[str] | None,
    permission_mode: PermissionMode | None,
    max_turns: int | None,
) -> SdkClient:
    """Build the real SDK client; tests monkeypatch this factory."""

    from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient

    return ClaudeSDKClient(
        ClaudeAgentOptions(
            system_prompt=system_prompt,
            model=model,
            cwd=cwd,
            allowed_tools=list(allowed_tools or []),
            permission_mode=permission_mode,
            max_turns=max_turns,
        )
    )


def _turn_blob(message: Message) -> bytes:
    """Serialize one SDK message for CAS: the full payload, typed."""

    return json.dumps(
        {"type": type(message).__name__, "message": dataclasses.asdict(message)},
        default=repr,
    ).encode()


class Session:
    """One live SDK session bound to a weave task entity.

    The reader task records a ``turn`` fact (a CAS pointer) per SDK message
    and a ``result`` fact per ``ResultMessage``; the watcher task polls the
    journal for ``interrupt=requested``. Terminal state lands exactly once:
    ``interrupted`` at interrupt time, ``failed`` if the SDK stream errors,
    else ``done`` at :meth:`close` -- always the entity's last state fact.
    """

    def __init__(self, task: str, client: SdkClient) -> None:
        self.task = task
        self._client = client
        self._interrupted = False
        self._terminal_written = False
        self._result: str | None = None
        self._error: BaseException | None = None
        self._turn_done = asyncio.Event()
        self._reader: asyncio.Task[None] | None = None
        self._watcher: asyncio.Task[None] | None = None

    def _start(self) -> None:
        self._reader = asyncio.create_task(self._read(), name=f"fabric:claude:read:{self.task}")
        self._watcher = asyncio.create_task(self._watch_interrupt(), name=f"fabric:claude:watch:{self.task}")

    async def _read(self) -> None:
        from claude_agent_sdk import ResultMessage

        try:
            async for message in self._client.receive_messages():
                await weave.record([(self.task, "turn", weave.Blob(_turn_blob(message)))])
                if isinstance(message, ResultMessage):
                    self._result = message.result or ""
                    await weave.record([(self.task, "result", weave.Blob(self._result.encode()))])
                    self._turn_done.set()
        except asyncio.CancelledError:
            raise
        except BaseException as exc:
            self._error = exc
            await self._write_terminal("failed", error=f"{type(exc).__name__}: {exc}")
            self._turn_done.set()
            raise

    async def _watch_interrupt(self) -> None:
        from . import watch_interrupt

        await watch_interrupt(self.task, self._do_interrupt)

    async def _write_terminal(self, state: str, *, error: str | None = None) -> None:
        if self._terminal_written:
            return
        self._terminal_written = True
        facts: list[tuple[str, str, object]] = [] if error is None else [(self.task, "error", error)]
        await weave.record([*facts, (self.task, "state", state)])

    async def _do_interrupt(self) -> None:
        if self._interrupted:
            return
        self._interrupted = True
        await self._client.interrupt()
        await self._write_terminal("interrupted")
        self._turn_done.set()

    async def send(self, text: str) -> None:
        """Stream a follow-up user message into the live session."""

        if self._interrupted or self._terminal_written:
            raise RuntimeError(f"session is closed: {self.task}")
        payload = json.dumps({"type": "UserMessage", "message": {"content": text}}).encode()
        await weave.record([(self.task, "turn", weave.Blob(payload))])
        self._turn_done.clear()
        await self._client.query(text)

    async def result(self, *, timeout: float | None = None) -> str:
        """Wait for the current turn's result text (also set on interrupt/failure)."""

        async with asyncio.timeout(timeout):
            await self._turn_done.wait()
        if self._error is not None:
            raise self._error
        return self._result or ""

    async def interrupt(self) -> None:
        """Interrupt natively: record the request, stop the SDK, mark interrupted."""

        await weave.record([(self.task, "interrupt", "requested")])
        await self._do_interrupt()

    async def close(self) -> None:
        """Disconnect and write the terminal state (``done`` unless one landed)."""

        for task in (self._reader, self._watcher):
            if task is not None:
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
        await self._client.disconnect()
        await self._write_terminal("done")

    async def __aenter__(self) -> Session:
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.close()


async def session(
    prompt: str,
    *,
    system_prompt: str | None = None,
    model: str | None = None,
    cwd: str | None = None,
    allowed_tools: Sequence[str] | None = None,
    permission_mode: PermissionMode | None = None,
    max_turns: int | None = None,
) -> Session:
    """Open a recorded, interruptible Claude session and send ``prompt``.

    Ask facts land first (``state=submitted`` last) through ``weave.record``:
    durably spooled on local disk before the SDK subprocess spawns, delivered
    to the journal in that order whenever weave is reachable - a down weave
    server never blocks or loses the intent (index#3418). A connect or
    first-send failure still appends the ``failed`` terminal fact. On
    success the session is live (``state=running``) and returns immediately:
    ``await s.result()`` waits for the turn, ``s.send()`` streams follow-up
    input, ``s.interrupt()`` stops it.
    """

    from . import _requested_by

    task = weave.mint("task")
    facts: list[tuple[str, str, object]] = [
        (task, "type", "task"),
        (task, "fn", "claude.session"),
        (task, "node", platform.node()),
        (task, "requested_by", _requested_by()),
        (task, "prompt", weave.Blob(prompt.encode())),
    ]
    if model is not None:
        facts.append((task, "model", model))
    facts.append((task, "state", "submitted"))
    await weave.record(facts)

    client = _sdk_client(
        system_prompt=system_prompt,
        model=model,
        cwd=cwd,
        allowed_tools=allowed_tools,
        permission_mode=permission_mode,
        max_turns=max_turns,
    )
    live = Session(task, client)
    try:
        await client.connect()
        await client.query(prompt)
    except BaseException as exc:
        await live._write_terminal("failed", error=f"{type(exc).__name__}: {exc}")
        raise
    await weave.record([(task, "state", "running")])
    live._start()
    return live
