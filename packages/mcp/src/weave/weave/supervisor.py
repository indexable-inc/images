"""Weave 2 supervisor: the only tier-2 effect runner for spawn/reply verbs."""

from __future__ import annotations

import asyncio
import contextlib
import os
import socket
import time
from pathlib import Path
from typing import Any

from ix_notebook_mcp.config import runtime_dir

from . import Weave, assert_facts, mint, query

__all__ = ["run"]

_MAX_SPAWNS = 4
_HEARTBEAT_S = 60.0
_HARNESS_TIMEOUT_S = 1800.0


def _ms() -> int:
    return int(time.time() * 1000)


class _Lock:
    """Exclusive supervisor pid-file lock.

    Uses ``os.open(..., O_CREAT|O_EXCL)`` so exactly one supervisor can own
    ``runtime_dir()/weave-supervisor.lock`` per machine. If the recorded pid no
    longer exists we remove the stale file and take over.
    """

    def __init__(self) -> None:
        self.path = runtime_dir() / "weave-supervisor.lock"
        self.fd: int | None = None

    def acquire(self) -> bool:
        while True:
            try:
                self.fd = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
                os.write(self.fd, str(os.getpid()).encode())
                return True
            except FileExistsError:
                try:
                    pid = int(self.path.read_text().strip() or "0")
                    os.kill(pid, 0)
                    return False
                except (ValueError, ProcessLookupError):
                    with contextlib.suppress(FileNotFoundError):
                        self.path.unlink()
                except PermissionError:
                    return False

    def release(self) -> None:
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None
        with contextlib.suppress(FileNotFoundError):
            if self.path.read_text().strip() == str(os.getpid()):
                self.path.unlink()


def _harness_argv(task: str) -> list[str]:
    override = os.environ.get("IX_WEAVE_HARNESS_BIN")
    if override:
        return [override, task]
    claude = os.environ.get("IX_WEAVE_CLAUDE_BIN") or "claude"
    # Matches the headless harness shape: claude -p <task> --output-format json.
    return [claude, "-p", task, "--output-format", "json"]


async def _rows(program: str) -> list[list[Any]]:
    return (await query(program))["rows"]


async def _one(program: str) -> object | None:
    rows = await _rows(program)
    return rows[0][0] if rows and rows[0] else None


async def _heartbeat(client: Weave, host_id: str) -> None:
    while True:
        # Load-bearing: heartbeat_ms advances Weave data-time ``now`` even when
        # no user facts change, keeping idle/eviction rules deterministic.
        now = _ms()
        await client.assert_facts([(host_id, "last_active_ms", now), (host_id, "heartbeat_ms", now)])
        await asyncio.sleep(_HEARTBEAT_S)


async def _retract_claim(client: Weave, claim_ids: list[str]) -> None:
    for fact_id in claim_ids:
        await client.retract(fact_id)


async def _claim(client: Weave, host_id: str, req: str) -> list[str] | None:
    """Assert this host's claim on ``req``; return the claim fact ids, or None if lost.

    The claim lands BEFORE any effect, so a supervisor crash mid-spawn leaves
    (req, claimed_by, host) in the journal and the prelude reopens the request
    once this host's heartbeat goes stale. Claims are latest-wins by journal
    order: after asserting we read claimed_by back and back off when another
    host wrote after us, retracting our superseded claim so it can never
    resurrect if the winner later retracts theirs.
    """

    acks = await client.assert_facts([(req, "claimed_by", host_id), (req, "claimed_ms", _ms())])
    claim_ids = [str(ack["id"]) for ack in acks]
    winner = await _one(f"?- latest({req}, claimed_by, H).")
    if winner != host_id:
        await _retract_claim(client, claim_ids)
        return None
    return claim_ids


async def _spawn_request(
    client: Weave, host_id: str, req: str, prefab: str, sem: asyncio.Semaphore
) -> bool:
    async with sem:
        claim_ids = await _claim(client, host_id, req)
        if claim_ids is None:
            return False
        task = await _one(f"?- task({req}, T).") or ""
        requested_by = await _one(f"?- latest({req}, requested_by, A).") or "agent:main"
        harness = await _one(f"?- attr_of({prefab}, harness, H).") or "claude-code"
        agent = mint("agent")
        label = " ".join(str(task).split()[:5]) or agent
        started = _ms()
        argv = _harness_argv(str(task))
        try:
            proc = await asyncio.create_subprocess_exec(
                *argv,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
            )
        except OSError:
            # Launch failed: retract the claim so the request reopens now
            # instead of waiting out this host's heartbeat staleness window.
            await _retract_claim(client, claim_ids)
            raise
        await client.assert_facts(
            [
                (agent, "type", "agent"),
                (agent, "is_a", prefab),
                (agent, "label", label),
                (agent, "spawned_by", requested_by),
                (agent, "harness", harness),
                (agent, "status", "running"),
                (agent, "started_ms", started),
                (agent, "pid", proc.pid or 0),
                (agent, "fulfills", req),
                (agent, "on_host", host_id),
                (agent, "last_active_ms", started),
                (req, "status", "fulfilled"),
            ]
        )
        status = "error"
        text = ""
        try:
            out, _ = await asyncio.wait_for(proc.communicate(), timeout=_HARNESS_TIMEOUT_S)
            text = out.decode(errors="replace")
            status = "done" if proc.returncode == 0 else "error"
        except TimeoutError:
            proc.kill()
            out, _ = await proc.communicate()
            text = (out or b"").decode(errors="replace") + "\n(timeout)"
        ended = _ms()
        await client.assert_facts(
            [
                (agent, "status", status),
                (agent, "last_output", text[-200:]),
                (agent, "ended_ms", ended),
                (agent, "last_active_ms", ended),
            ]
        )
        return True


async def _watch_spawns(client: Weave, host_id: str) -> None:
    sem = asyncio.Semaphore(_MAX_SPAWNS)
    background: set[asyncio.Task[bool]] = set()
    # Dedup cache only: correctness derives from claim facts (a live-claimed
    # request is not open), so a fresh supervisor never double-spawns.
    seen: set[str] = set()
    # open_spawn_request/2 carries the prefab (prelude rule): (R, P).
    async for batch in client.watch("?- open_spawn_request(R, P)."):
        for row in batch["added"]:
            if len(row) < 2:
                continue
            req, prefab = row[0], row[1]
            if req in seen:
                continue
            seen.add(req)
            task = asyncio.create_task(_spawn_request(client, host_id, req, prefab, sem))
            background.add(task)

            def _forget(task: asyncio.Task[bool], req: str = req) -> None:
                background.discard(task)
                # Lost the claim race: forget the request so this host can
                # still reclaim it if the winner's claim ever goes stale.
                if not task.cancelled() and task.exception() is None and not task.result():
                    seen.discard(req)

            task.add_done_callback(_forget)


async def _reply(client: Weave, msg: str, agent: str) -> None:
    sender = await _one(f"?- from({msg}, S).") or "agent:main"
    text = await _one(f"?- attr_of({msg}, text, T).") or await _one(f"?- text({msg}, T).") or ""
    context_rows = await _rows(f"?- recent_msg(M), from(M, F), to(M, T), attr_of(M, text, X).")
    context = "\n".join(f"{r[1]} -> {r[2]}: {r[3]}" for r in context_rows[-12:])
    prompt = f"Context:\n{context}\n\nReply to {sender}: {text}"
    proc = await asyncio.create_subprocess_exec(
        *_harness_argv(prompt),
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    out, _ = await asyncio.wait_for(proc.communicate(), timeout=_HARNESS_TIMEOUT_S)
    body = out.decode(errors="replace")[-4000:]
    mid = mint("msg")
    now = _ms()
    thread = f"thread:{sender.removeprefix('agent:')}"
    label = await _one(f"?- label({agent}, L).") or agent
    # Message facts are asserted in contract order, with text LAST.
    await client.assert_facts(
        [
            (mid, "type", "message"),
            (mid, "thread", thread),
            (mid, "from", agent),
            (mid, "to", sender),
            (mid, "role", "agent"),
            (mid, "author", label),
            (mid, "reply_to", msg),
            (agent, "last_output", body[-200:]),
            (agent, "last_active_ms", now),
            (mid, "text", body),
        ]
    )


async def _watch_replies(client: Weave, *, answer_main: bool) -> None:
    seen: set[str] = set()
    background: set[asyncio.Task[None]] = set()
    async for batch in client.watch("?- needs_reply(M, A)."):
        for msg, agent in batch["added"]:
            if msg in seen or (agent == "agent:main" and not answer_main):
                continue
            seen.add(msg)
            task = asyncio.create_task(_reply(client, msg, agent))
            background.add(task)
            task.add_done_callback(background.discard)


async def run(weave_url: str | None = None, host: str | None = None, *, answer_main: bool = False) -> None:
    """Run the singleton Weave supervisor until cancelled."""

    if weave_url is not None:
        os.environ["WEAVE_URL"] = weave_url
    lock = _Lock()
    if not lock.acquire():
        return
    client = Weave()
    host_id = host or f"host:{socket.gethostname()}"
    try:
        await client.assert_facts([(host_id, "type", "host"), (host_id, "label", socket.gethostname())])
        await asyncio.gather(_heartbeat(client, host_id), _watch_spawns(client, host_id), _watch_replies(client, answer_main=answer_main))
    finally:
        lock.release()
