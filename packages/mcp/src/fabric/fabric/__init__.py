"""Call-first delegation recorded to the weave journal (index#3191, #3192).

Calls create work; facts record it. ``await fabric.run(fn, *args)`` executes
``fn`` on this node; ``await fabric.run(fn, *args, node="hc1")`` ships it to
that fleet node's runner actor over Ray (phase 2 of index#3190). Either way
the record lands on the shared weave journal via the bundled :mod:`weave`
client: the ask-fact at submit (so the intent is never invisible), started and
terminal facts from the worker side, and the function's source text in CAS.
The journal never dispatches: the ask state is ``submitted``, distinct from
the ``pending`` state that the weave app fulfills.

Remote placement (:mod:`fabric.remote`) fails loud at submit: no reachable
cluster, an env-hash mismatch with the target node, or a host label no node
advertises each raise from the ``run`` call itself, before any journal fact.

``fabric.claude`` holds the Claude Agent SDK session helper: an agent is a
library call (``await claude.session(prompt)``), not a task type.
"""

from __future__ import annotations

import asyncio
import inspect
import os
import platform
import textwrap
from collections.abc import Callable, Generator
from dataclasses import dataclass

import weave

from . import claude, remote
from .remote import EnvSkewError, FabricError, Workspace

__all__ = [
    "EnvSkewError",
    "FabricError",
    "RunHandle",
    "Workspace",
    "claude",
    "remote",
    "run",
]

__version__ = "0.2.0"

# The ask state. Deliberately NOT the weave app's "pending": pending is what
# its fulfiller loop dispatches, and fabric's model is that the call already
# created the work -- the journal only records it (index#3190).
ASK_STATE = "submitted"


def _requested_by() -> str:
    return os.environ.get("IX_WEAVE_AGENT") or "agent:main"


@dataclass
class RunHandle:
    """One live fabric run: the journal task entity plus the execution."""

    task: str
    _work: asyncio.Task[object]

    async def wait(self) -> object:
        """Wait for the run and return the function's value (re-raises its error)."""

        return await self._work

    def __await__(self) -> Generator[object, None, object]:
        return self._work.__await__()

    async def interrupt(self) -> None:
        """Cancel the run; the worker wrapper records ``state=interrupted``.

        Returns once the run has settled, so the interrupted fact is on the
        journal when this returns. A remote run's Ray task is cancelled on the
        node too (see :func:`fabric.remote.execute`). The run's own outcome
        (the cancellation, or an error it already failed with) surfaces via
        :meth:`wait`, not here.
        """

        self._work.cancel()
        await asyncio.gather(self._work, return_exceptions=True)


def _check_arguments(
    *,
    node: str | None,
    local: bool | None,
    cpus: float | None,
    repo: str | None,
    rev: str | None,
) -> None:
    """Reject contradictory placement arguments before anything runs."""

    if node is not None and local:
        raise ValueError(f"fabric.run: node={node!r} contradicts local=True")
    if local is False and node is None:
        raise ValueError("fabric.run: local=False requires node='<host>' (see fleet.nodes())")
    if (repo is None) != (rev is None):
        raise ValueError("fabric.run: repo and rev come together (a task pins an exact rev)")
    if node is None and (cpus is not None or repo is not None):
        raise ValueError(
            "fabric.run: cpus/repo/rev are remote placement arguments; add node='<host>'"
        )


async def run(
    fn: Callable[..., object],
    *args: object,
    node: str | None = None,
    local: bool | None = None,
    cpus: float | None = None,
    repo: str | None = None,
    rev: str | None = None,
    **kwargs: object,
) -> RunHandle:
    """Execute ``fn(*args, **kwargs)``, recorded on the journal.

    Placement: with no ``node`` the run is local to this kernel; with
    ``node='<host>'`` it ships (cloudpickled) to that fleet node's detached
    ``runner:<host>`` actor, validated at submit (env handshake + host label,
    see :mod:`fabric.remote`) so a bad target raises here, not as a
    forever-queued task. ``cpus=N`` opts a CPU-bound function out of the
    shared runner into a dedicated Ray task with an honest ``num_cpus``
    reservation. ``repo=``/``rev=`` make the runner materialize that checkout
    in a per-run temp dir and pass its path as the function's first argument.

    At submit, one ask-facts batch describes the task entity: type,
    requested_by, node, the function's qualname, and its source text stored in
    CAS (``put_blob``) with the hash on the ``source`` fact -- with
    ``state=submitted`` written strictly last, so a half-written record is
    never read as a live task. The worker wrapper then appends ``running`` and
    the terminal fact: ``done`` (result repr in CAS), ``failed`` (with the
    error detail), or ``interrupted``. A ``fn`` that raises before its first
    line (a bad signature bind, a first-statement raise) still leaves the ask
    and ``failed`` facts.

    Locally, sync functions run in ``asyncio.to_thread`` so the kernel loop
    never blocks; async functions are awaited natively (the runner actor does
    the same on the node). Note a cancelled sync ``fn`` settles the run (and
    records ``interrupted``) while its thread finishes in the background:
    threads cannot be killed.
    """

    _check_arguments(node=node, local=local, cpus=cpus, repo=repo, rev=rev)
    placement = await remote.prepare(node) if node is not None else None
    workspace = Workspace(repo=repo, rev=rev) if repo is not None and rev is not None else None
    source = textwrap.dedent(inspect.getsource(fn))
    task = weave.mint("task")
    source_hash = await weave.put_blob(source.encode())
    await weave.assert_facts([
        (task, "type", "task"),
        (task, "fn", fn.__qualname__),
        (task, "node", node if node is not None else platform.node()),
        (task, "requested_by", _requested_by()),
        (task, "source", weave.hashref(source_hash)),
        (task, "state", ASK_STATE),
    ])

    async def _invoke() -> object:
        if placement is not None:
            return await remote.execute(
                placement, fn, args, kwargs, cpus=cpus, workspace=workspace
            )
        if inspect.iscoroutinefunction(fn):
            return await fn(*args, **kwargs)
        return await asyncio.to_thread(fn, *args, **kwargs)

    async def _work() -> object:
        await weave.assert_fact(task, "state", "running")
        try:
            result = await _invoke()
        except asyncio.CancelledError:
            await weave.assert_fact(task, "state", "interrupted")
            raise
        except BaseException as exc:
            await weave.assert_facts([
                (task, "error", f"{type(exc).__name__}: {exc}"),
                (task, "state", "failed"),
            ])
            raise
        result_hash = await weave.put_blob(repr(result).encode())
        await weave.assert_facts([
            (task, "result", weave.hashref(result_hash)),
            (task, "state", "done"),
        ])
        return result

    return RunHandle(task, asyncio.create_task(_work(), name=f"fabric:{task}"))
