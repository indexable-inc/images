"""Call-first delegation recorded to the weave journal (index#3191, #3192, #3193).

Calls create work; facts record it. ``await fabric.run(fn, *args)`` executes
``fn`` on this node; ``await fabric.run(fn, *args, node="hc1")`` ships it to
that fleet node's runner actor over Ray (phase 2 of index#3190). Either way
the record lands on the shared weave journal via the bundled :mod:`weave`
client: the ask-fact at submit (so the intent is never invisible), started and
terminal facts from the worker side, and the function's source text in CAS.
The journal never dispatches: the ask state is ``submitted``, and there is no
dispatcher loop anywhere -- the call that created the work owns it.

Remote placement (:mod:`fabric.remote`) fails loud at submit: no reachable
cluster, an env-hash mismatch with the target node, or a host label no node
advertises each raise from the ``run`` call itself, before any journal fact.

Phase 3 (index#3193) closes the loop journal-side: anyone can stop a run by
asserting one ``interrupt=requested`` fact on its entity (the owner watches
via :func:`watch_interrupt`); :mod:`fabric.reconcile` appends ``state=lost``
for runs whose runner actor died without a terminal fact, and NEVER restarts
them; :mod:`fabric.activity` is the "what is happening on every node" view,
one datalog query over the journal instead of Ray's dashboard.

``fabric.claude`` holds the Claude Agent SDK session helper: an agent is a
library call (``await claude.session(prompt)``), not a task type.
"""

from __future__ import annotations

import asyncio
import inspect
import platform
import textwrap
from collections.abc import Awaitable, Callable, Generator
from dataclasses import dataclass

import weave

from . import activity, claude, reconcile, remote, resources
from .remote import EnvSkewError, FabricError, Workspace

__all__ = [
    "EnvSkewError",
    "FabricError",
    "RunHandle",
    "Workspace",
    "activity",
    "claude",
    "reconcile",
    "remote",
    "resources",
    "run",
    "watch_interrupt",
]

__version__ = "0.4.0"

# The ask state. Deliberately NOT "pending" (the state a dispatcher loop
# would fulfill): fabric's model is that the call already created the work --
# the journal only records it (index#3190).
ASK_STATE = "submitted"

# How often a run's owner polls its journal entity for an externally asserted
# interrupt=requested fact. Module-level so tests can shrink it.
INTERRUPT_POLL_S = 0.5


def _requested_by() -> str:
    return resources.current_owner()


async def watch_interrupt(task: str, on_requested: Callable[[], Awaitable[None]]) -> None:
    """Fire ``on_requested`` once when ``task`` gains an ``interrupt=requested`` fact.

    The interrupt bridge (index#3193): anyone watching the journal stops a run
    by asserting one fact on its entity; the process that owns the run polls
    its own entity and routes the request into its own interrupt path (the
    phase 1 contract: the SDK ``interrupt()`` for claude sessions, asyncio
    cancellation otherwise). No dispatcher loop: each run watches only itself,
    from wherever it lives.
    """

    while True:
        rows = (await weave.query(f'?- latest("{task}", interrupt, I).'))["rows"]
        if rows and rows[0][0] == "requested":
            await on_requested()
            return
        await asyncio.sleep(INTERRUPT_POLL_S)


@dataclass
class RunHandle:
    """One live fabric run: the journal task entity plus the execution."""

    task: str
    resource_claim: resources.ResourceClaim | None
    _work: asyncio.Task[object]
    _watcher: asyncio.Task[None]

    async def wait(self) -> object:
        """Wait for the run and return the function's value (re-raises its error)."""

        return await self._work

    def __await__(self) -> Generator[object, None, object]:
        return self._work.__await__()

    async def interrupt(self) -> None:
        """Record ``interrupt=requested``, cancel the run, wait for it to settle.

        The worker wrapper records ``state=interrupted``, so the terminal fact
        is on the journal when this returns; the request fact makes the handle
        path and the journal-fact path (:func:`watch_interrupt`) leave the
        same record. A remote run's Ray task is cancelled on the node too
        (see :func:`fabric.remote.execute`). The run's own outcome (the
        cancellation, or an error it already failed with) surfaces via
        :meth:`wait`, not here.
        """

        await weave.assert_fact(self.task, "interrupt", "requested")
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
    resource_key: resources.ResourceKey | None = None,
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
    requested_by, node, the function's qualname, its source text stored in
    CAS (``put_blob``) with the hash on the ``source`` fact, and -- for remote
    placement -- the ``runner:<host>`` actor that owns the execution (what
    :mod:`fabric.reconcile` diffs against live actors) -- with
    ``state=submitted`` written strictly last, so a half-written record is
    never read as a live task. The worker wrapper then appends ``running`` and
    the terminal fact: ``done`` (result repr in CAS), ``failed`` (with the
    error detail), or ``interrupted``. A ``fn`` that raises before its first
    line (a bad signature bind, a first-statement raise) still leaves the ask
    and ``failed`` facts.

    ``resource_key`` declares the pull request or worktree this task may
    mutate. Fabric claims it atomically after recording the ask and before
    starting the worker. A live second owner raises ``ResourceOwnedError`` and
    leaves this task failed; a terminal owner's claim can be reclaimed.

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
    facts: list[tuple[str, str, object]] = [
        (task, "type", "task"),
        (task, "fn", fn.__qualname__),
        (task, "node", node if node is not None else platform.node()),
        (task, "requested_by", _requested_by()),
        (task, "source", weave.hashref(source_hash)),
    ]
    if node is not None:
        facts.append((task, "runner", f"runner:{node}"))
    facts.append((task, "state", ASK_STATE))
    await weave.assert_facts(facts)
    try:
        resource_claim = (
            None
            if resource_key is None
            else await resources.claim(resource_key, owner=task)
        )
    except BaseException as exc:
        await weave.assert_facts([
            (task, "error", f"{type(exc).__name__}: {exc}"),
            (task, "state", "failed"),
        ])
        raise

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

    work = asyncio.create_task(_work(), name=f"fabric:{task}")

    async def _cancel() -> None:
        work.cancel()
        await asyncio.gather(work, return_exceptions=True)

    watcher = asyncio.create_task(watch_interrupt(task, _cancel), name=f"fabric:watch:{task}")
    work.add_done_callback(lambda _t: watcher.cancel())
    return RunHandle(
        task=task,
        resource_claim=resource_claim,
        _work=work,
        _watcher=watcher,
    )
