"""Reconcile open runs against live Ray runner actors (index#3193).

The journal is the system of record and Ray is only the substrate, so the one
record that can silently go stale is a run whose runner actor died before the
worker wrapper could append a terminal fact (node poweroff, an OOM-killed
raylet, ``ray stop``). The reconciler closes exactly that gap: every open run
naming a ``runner`` that no live actor answers for gets ``state=lost``
appended -- and nothing else. It NEVER restarts work: ``max_restarts=0`` is
policy (:mod:`fabric.remote`), a lost run may have had side effects in
flight, and re-running is a fresh, explicit ``fabric.run`` by whoever reads
the ``lost`` fact.

Only runner-placed runs are reconciled: a local run (no ``runner`` fact)
lives and dies with the kernel that owns it, whose journal lease already
sweeps it from derived views. The open-runs view is one datalog query
(:data:`QUERY`) joining each run's latest state with that fact's write
time; a grace window (:data:`GRACE_S`) keeps a just-submitted run from being
marked lost in the instant between its ask facts landing and Ray creating
the actor.
"""

from __future__ import annotations

import asyncio
import time
from typing import TYPE_CHECKING

import weave

from . import remote

if TYPE_CHECKING:
    from collections.abc import Sequence, Set

__all__ = ["GRACE_S", "INTERVAL_S", "OPEN_STATES", "QUERY", "loop", "once"]

# States with no terminal fact yet; everything else is settled.
OPEN_STATES = frozenset({"submitted", "running"})

# Latest state per runner-placed run, joined with that state fact's write
# time (writer wall clock, ms): fact_id/fact_time are the journal's base
# relations (weave prelude.dl).
QUERY = "?- latest(T, runner, R), latest(T, state, S), fact_id(F, T, state, S), fact_time(F, MS)."

# A run is only lost once its latest state fact is at least this old: the
# submit batch lands before remote.execute creates the runner actor, so a
# brand-new run legitimately has no actor for a moment.
GRACE_S = 60.0

INTERVAL_S = 30.0

PING_TIMEOUT_S = 5.0


def _parse(rows: Sequence[Sequence[object]], *, now_ms: float) -> list[tuple[str, str]]:
    """The (task, runner) pairs that are open and past the grace window."""

    latest_ms: dict[tuple[str, str, str], float] = {}
    for task, runner, state, _fact, ms in rows:
        key = (str(task), str(runner), str(state))
        latest_ms[key] = max(latest_ms.get(key, float("-inf")), float(str(ms)))
    return [
        (task, runner)
        for (task, runner, state), ms in sorted(latest_ms.items())
        if state in OPEN_STATES and now_ms - ms >= GRACE_S * 1000
    ]


def _alive_runners(candidates: Set[str]) -> set[str]:
    """Which of ``candidates`` a live actor answers for. Sync: run in a thread.

    Uncertainty is alive: a ping that times out (a busy actor, a slow node)
    keeps the run open, because a false ``lost`` is worse than a late one.
    Only ``RayActorError`` (the actor is confirmed dead) and a name no actor
    holds count as dead.
    """

    import ray

    remote._connect()
    alive: set[str] = set()
    for name in sorted(candidates):
        try:
            actor = ray.get_actor(name, namespace=remote.NAMESPACE)
        except ValueError:
            continue
        try:
            ray.get(actor.__ray_ready__.remote(), timeout=PING_TIMEOUT_S)
        except ray.exceptions.RayActorError:
            continue
        except ray.exceptions.GetTimeoutError:
            pass
        alive.add(name)
    return alive


async def once() -> list[str]:
    """One reconcile pass: append ``state=lost`` per dead run, return their ids.

    Appends an ``error`` fact naming the dead runner alongside each ``lost``
    state, so the record says what happened, not just that it stopped.
    """

    rows = (await weave.query(QUERY))["rows"]
    stale = _parse(rows, now_ms=time.time() * 1000)
    if not stale:
        return []
    alive = await asyncio.to_thread(_alive_runners, {runner for _, runner in stale})
    lost = [(task, runner) for task, runner in stale if runner not in alive]
    for task, runner in lost:
        await weave.assert_facts([
            (task, "error", f"reconciler: {runner} died without a terminal fact"),
            (task, "state", "lost"),
        ])
    return [task for task, _ in lost]


async def loop(interval: float = INTERVAL_S) -> None:
    """Reconcile forever: ``jobs.spawn(fabric.reconcile.loop(), name='fabric: reconcile')``.

    Nothing is swallowed: a journal or cluster error kills the loop loudly
    (the job shows failed) rather than silently skipping passes.
    """

    while True:
        await once()
        await asyncio.sleep(interval)
