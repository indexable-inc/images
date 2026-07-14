"""Call-first local delegation recorded to the weave journal (index#3191).

Calls create work; facts record it. ``await fabric.run(fn, *args)`` executes
``fn`` on this node (phase 1 of index#3190: no Ray placement yet) and appends
the record to the shared weave journal via the bundled :mod:`weave` client:
the ask-fact at submit (so the intent is never invisible), started and
terminal facts from the worker side, and the function's source text in CAS.
The journal never dispatches: the ask state is ``submitted``, distinct from
the ``pending`` state that the weave app fulfills.

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

from . import claude

__all__ = [
    "RunHandle",
    "claude",
    "run",
]

__version__ = "0.1.0"

# The ask state. Deliberately NOT the weave app's "pending": pending is what
# its fulfiller loop dispatches, and fabric's model is that the call already
# created the work -- the journal only records it (index#3190).
ASK_STATE = "submitted"


def _requested_by() -> str:
    return os.environ.get("IX_WEAVE_AGENT") or "agent:main"


@dataclass
class RunHandle:
    """One live fabric run: the journal task entity plus the local execution."""

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
        journal when this returns. The run's own outcome (the cancellation, or
        an error it already failed with) surfaces via :meth:`wait`, not here.
        """

        self._work.cancel()
        await asyncio.gather(self._work, return_exceptions=True)


async def run(fn: Callable[..., object], *args: object, local: bool = True, **kwargs: object) -> RunHandle:
    """Execute ``fn(*args, **kwargs)`` on this node, recorded on the journal.

    At submit, one ask-facts batch describes the task entity: type,
    requested_by, node, the function's qualname, and its source text stored in
    CAS (``put_blob``) with the hash on the ``source`` fact -- with
    ``state=submitted`` written strictly last, so a half-written record is
    never read as a live task. The worker wrapper then appends ``running`` and
    the terminal fact: ``done`` (result repr in CAS), ``failed`` (with the
    error detail), or ``interrupted``. A ``fn`` that raises before its first
    line (a bad signature bind, a first-statement raise) still leaves the ask
    and ``failed`` facts.

    Sync functions run in ``asyncio.to_thread`` so the kernel loop never
    blocks; async functions are awaited natively. Note a cancelled sync ``fn``
    settles the run (and records ``interrupted``) while its thread finishes in
    the background: threads cannot be killed.

    ``local=False`` (Ray placement, ``node=``) is index#3190 phase 2+.
    """

    if not local:
        raise NotImplementedError(
            "fabric phase 1 is local-only: run(fn, *args, local=True); "
            "Ray placement across the fleet is index#3190 phase 2"
        )
    source = textwrap.dedent(inspect.getsource(fn))
    task = weave.mint("task")
    source_hash = await weave.put_blob(source.encode())
    await weave.assert_facts([
        (task, "type", "task"),
        (task, "fn", fn.__qualname__),
        (task, "node", platform.node()),
        (task, "requested_by", _requested_by()),
        (task, "source", weave.hashref(source_hash)),
        (task, "state", ASK_STATE),
    ])

    async def _work() -> object:
        await weave.assert_fact(task, "state", "running")
        try:
            if inspect.iscoroutinefunction(fn):
                result = await fn(*args, **kwargs)
            else:
                result = await asyncio.to_thread(fn, *args, **kwargs)
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
