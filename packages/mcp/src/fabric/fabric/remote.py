"""Ray placement for :func:`fabric.run` (index#3192, phase 2 of index#3190).

The cluster contract, end to end:

- **Env handshake.** Every fabric node advertises a ``fabric_env:<tag>`` Ray
  resource (tag = ``py<maj.min>-ray<version>``, declared in Nix by
  ``lib/fabric.nix``); every fabric driver carries the same string in
  ``IX_FABRIC_ENV`` (baked onto the ix-mcp wrappers). :func:`prepare` compares
  them at submit and raises :class:`EnvSkewError` on mismatch -- cloudpickled
  bytecode only travels safely between identical python/ray pins.

- **Placement.** A node is targeted by its ``host_<name>`` resource label.
  A label no live node advertises would make Ray queue the task forever, so
  :func:`prepare` checks the cluster's resource table first and raises
  :class:`FabricError` naming the known hosts.

- **Runner.** Work lands on a per-node detached named async actor
  (``runner:<host>``, namespace ``fabric``): ``num_cpus=0`` so it never
  competes for scheduling slots, high ``max_concurrency`` so runs interleave,
  and ``max_restarts=0`` ALWAYS -- a restarted runner would silently re-run
  or orphan work; the journal, not Ray, owns retry policy (index#3190).
  ``cpus=`` opts out into a dedicated Ray task with an honest ``num_cpus``
  reservation (and ``max_retries=0``, same policy).

- **Payload.** Tasks travel as explicit cloudpickle bytes both ways (ray's
  vendored ``ray.cloudpickle``, pinned with ray itself). The daemon env on the
  nodes carries ray alone -- no fabric/weave -- so this module registers
  itself for pickle-by-value: everything it ships is self-contained.

- **Workspace.** A task may carry ``repo`` + ``rev``; the runner materializes
  a fresh clone at that rev in a per-run temp dir and hands the path to the
  function as its first argument.

No fallbacks anywhere: no reachable cluster, env skew, a missing label, or a
failed clone each raise a precise error at the earliest point they are
knowable.
"""

from __future__ import annotations

import asyncio
import inspect
import logging
import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path

__all__ = [
    "ENV_PREFIX",
    "ENV_VAR",
    "NAMESPACE",
    "EnvSkewError",
    "FabricError",
    "Placement",
    "Workspace",
    "actor_options",
    "check_env",
    "check_host_label",
    "execute",
    "local_env",
    "materialize",
    "prepare",
    "task_options",
]

# The driver-side half of the env handshake, baked onto the ix-mcp wrappers by
# packages/mcp (from `lib/fabric.nix`, the one owner of the format).
ENV_VAR = "IX_FABRIC_ENV"
# The node-side half: the resource-name prefix fabric nodes advertise.
ENV_PREFIX = "fabric_env:"

# All runner actors live in one named Ray namespace so `runner:<host>` lookup
# works from any driver (anonymous namespaces are per-driver).
NAMESPACE = "fabric"

# The runner is I/O-shaped (async fns, to_thread-wrapped sync fns), so many
# runs interleave on one actor; a CPU-bound run should use `cpus=` instead.
RUNNER_CONCURRENCY = 256

# What a placement consumes of the node's `host_<name>` capacity (1): small
# enough that hundreds of concurrent placements never exhaust the label.
HOST_SLICE = 0.001


class FabricError(RuntimeError):
    """A fabric placement could not be carried out; the message says why."""


class EnvSkewError(FabricError):
    """The target node's fabric env differs from this driver's."""


@dataclass(frozen=True)
class Workspace:
    """A repo checkout the runner materializes per run (``repo`` + ``rev``)."""

    repo: str
    rev: str


@dataclass(frozen=True)
class Placement:
    """A validated submit target: the node and its host resource label."""

    node: str
    label: str


@dataclass(frozen=True)
class _Task:
    """The cloudpickled payload shipped to the runner."""

    fn: Callable[..., object]
    args: tuple[object, ...]
    kwargs: dict[str, object]
    workspace: Workspace | None


def local_env() -> str:
    """This driver's ``fabric_env:<tag>`` resource name, from ``IX_FABRIC_ENV``."""

    value = os.environ.get(ENV_VAR)
    if not value:
        raise FabricError(
            f"fabric: {ENV_VAR} is not set; remote placement requires the pinned fabric env "
            "(the ix-mcp wrappers bake it in -- see lib/fabric.nix)"
        )
    return value


def check_host_label(node: str, cluster_resources: Mapping[str, float]) -> str:
    """The ``host_<node>`` label, after proving some live node advertises it.

    Checked against the cluster's *total* resource table, not
    ``available_resources``: a busy node's label drops out of the available
    view while the node is still there, and existence -- not headroom -- is
    what separates "will schedule" from "queues forever".
    """

    label = f"host_{node}"
    if label not in cluster_resources:
        hosts = sorted(
            key.removeprefix("host_") for key in cluster_resources if key.startswith("host_")
        )
        raise FabricError(
            f"fabric: no live Ray node advertises {label!r} (known hosts: {hosts or 'none'}); "
            "a task on a missing label would queue forever, so this fails at submit"
        )
    return label


def check_env(node: str, node_resources: Mapping[str, float], local: str) -> None:
    """Raise :class:`EnvSkewError` unless ``node`` advertises this driver's env."""

    advertised = sorted(key for key in node_resources if key.startswith(ENV_PREFIX))
    if local not in advertised:
        raise EnvSkewError(
            f"fabric: env skew: this driver runs {local!r} but node {node!r} advertises "
            f"{advertised or 'no fabric_env resource'}; cloudpickled code only travels "
            "between identical python/ray pins -- redeploy the node or the driver"
        )


def actor_options(node: str) -> dict[str, object]:
    """Options for the ``runner:<node>`` detached async actor.

    ``max_restarts=0`` is policy, not a default: no caller input reaches this,
    so a restarted-runner path cannot be configured into existence.
    """

    return {
        "name": f"runner:{node}",
        "namespace": NAMESPACE,
        "lifetime": "detached",
        "get_if_exists": True,
        "num_cpus": 0,
        "resources": {f"host_{node}": HOST_SLICE},
        "max_concurrency": RUNNER_CONCURRENCY,
        "max_restarts": 0,
    }


def task_options(node: str, cpus: float) -> dict[str, object]:
    """Options for a dedicated CPU-bound task (``fabric.run(..., cpus=N)``)."""

    return {
        "num_cpus": cpus,
        "resources": {f"host_{node}": HOST_SLICE},
        "max_retries": 0,
    }


def materialize(workspace: Workspace) -> Path:
    """Clone ``workspace.repo`` at ``workspace.rev`` into a fresh temp dir."""

    dest = Path(tempfile.mkdtemp(prefix=f"fabric-{workspace.rev[:8]}-")) / "repo"
    for cmd in (
        ["git", "clone", "--no-checkout", workspace.repo, str(dest)],
        ["git", "-C", str(dest), "checkout", "--detach", workspace.rev],
    ):
        proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
        if proc.returncode != 0:
            shutil.rmtree(dest.parent, ignore_errors=True)
            raise FabricError(f"fabric: `{' '.join(cmd)}` failed: {proc.stderr.strip()}")
    return dest


def _dumps(obj: object) -> bytes:
    """Typed seam over ray's vendored cloudpickle (untyped, not re-exported)."""

    from ray import cloudpickle

    data: bytes = cloudpickle.dumps(obj)  # type: ignore[attr-defined, no-untyped-call]
    return data


def _scrub(path: Path) -> None:
    """Single-arg :func:`shutil.rmtree` shape: zuban's typeshed overloads
    reject ``to_thread(shutil.rmtree, ..., ignore_errors=True)``."""

    shutil.rmtree(path, ignore_errors=True)


class Runner:
    """The per-node runner actor body (shipped by value; see module doc).

    Bytes in, bytes out: the payload is a cloudpickled :class:`_Task`, the
    return a cloudpickled result, so the wire contract is one explicit
    serializer (ray's vendored cloudpickle) in both directions.
    """

    async def run(self, payload: bytes) -> bytes:
        from ray import cloudpickle

        task: _Task = cloudpickle.loads(payload)
        args = task.args
        workdir: Path | None = None
        if task.workspace is not None:
            workdir = await asyncio.to_thread(materialize, task.workspace)
            args = (workdir, *args)
        try:
            if inspect.iscoroutinefunction(task.fn):
                result = await task.fn(*args, **task.kwargs)
            else:
                result = await asyncio.to_thread(task.fn, *args, **task.kwargs)
        finally:
            # Per-run scratch: the result travels by value, so nothing may
            # outlive the run inside the clone.
            if workdir is not None:
                await asyncio.to_thread(_scrub, workdir.parent)
        return _dumps(result)


def _run_task(payload: bytes) -> bytes:
    """The dedicated-task twin of :meth:`Runner.run` (``cpus=`` path): a plain
    Ray task with an honest CPU reservation, sync by design -- the reserved
    core is for the function itself."""

    from ray import cloudpickle

    task: _Task = cloudpickle.loads(payload)
    args = task.args
    workdir: Path | None = None
    if task.workspace is not None:
        workdir = materialize(task.workspace)
        args = (workdir, *args)
    try:
        if inspect.iscoroutinefunction(task.fn):
            result: object = asyncio.run(task.fn(*args, **task.kwargs))
        else:
            result = task.fn(*args, **task.kwargs)
    finally:
        if workdir is not None:
            shutil.rmtree(workdir.parent, ignore_errors=True)
    return _dumps(result)


def _register_by_value() -> None:
    """Ship this module's classes/functions by value, not module reference:
    the node daemons run the wrapped ray env alone (no fabric installed), so a
    by-reference pickle would die with ``ModuleNotFoundError`` on the worker.
    Idempotent; ray's registry is keyed by module."""

    from ray import cloudpickle

    cloudpickle.register_pickle_by_value(sys.modules[__name__])  # type: ignore[no-untyped-call]


def _connect() -> None:
    """Attach this driver to the fleet Ray cluster, or raise.

    Resolution mirrors ``fleet.connect`` (explicit env address, else the
    tailnet auto-probe) MINUS its local-Ray fallback: fabric placement names a
    specific host, and a silently started single-node Ray would turn
    ``run(node='hc1')`` into local execution -- the exact failure mode the
    submit-time checks exist to prevent.
    """

    import ray

    if ray.is_initialized():
        return
    target = os.environ.get("IX_FLEET_RAY_ADDRESS") or os.environ.get("RAY_ADDRESS")
    note = ""
    if not target:
        from fleet.cluster import _resolve_auto_target

        target, note = _resolve_auto_target()
    if not target:
        raise FabricError(
            "fabric: no Ray cluster target: set IX_FLEET_RAY_ADDRESS (ray://<head>:10001) "
            f"or RAY_ADDRESS; {note}"
        )
    try:
        ray.init(
            address=target,
            logging_level=logging.ERROR,
            configure_logging=False,
            ignore_reinit_error=True,
        )
    except Exception as error:
        raise FabricError(
            f"fabric: cannot attach to Ray at {target!r}: {type(error).__name__}: {error}"
        ) from error


def _prepare_sync(node: str) -> Placement:
    import ray

    _connect()
    label = check_host_label(node, ray.cluster_resources())
    local = local_env()
    for entry in ray.nodes():
        resources: Mapping[str, float] = entry.get("Resources") or {}
        if entry.get("Alive") and label in resources:
            check_env(node, resources, local)
            return Placement(node=node, label=label)
    # The label was in the cluster table but no alive node carries it: the
    # node died between the two reads. Same forever-queue hazard, same answer.
    raise FabricError(f"fabric: node {node!r} is no longer alive; resubmit when it rejoins")


async def prepare(node: str) -> Placement:
    """Validate ``node`` as a submit target (connect, label, env handshake).

    Raises before any work (or journal fact) exists, so a bad target fails the
    ``fabric.run`` call itself. Sync Ray driver calls run off the kernel loop.
    """

    return await asyncio.to_thread(_prepare_sync, node)


def _submit(placement: Placement, payload: bytes, cpus: float | None) -> object:
    """Ship the payload and return the Ray ObjectRef (sync driver calls)."""

    import ray

    if cpus is not None:
        return ray.remote(_run_task).options(**task_options(placement.node, cpus)).remote(payload)
    actor = ray.remote(Runner).options(**actor_options(placement.node)).remote()
    return actor.run.remote(payload)  # type: ignore[attr-defined]  # ActorHandle method proxy


async def execute(
    placement: Placement,
    fn: Callable[..., object],
    args: tuple[object, ...],
    kwargs: dict[str, object],
    *,
    cpus: float | None = None,
    workspace: Workspace | None = None,
) -> object:
    """Run ``fn`` on the placed node and return its (deserialized) result.

    Cancellation propagates: cancelling the awaiting task also cancels the Ray
    task on the node, so an interrupt is not a silent orphan.
    """

    from ray import cloudpickle

    _register_by_value()
    payload = _dumps(_Task(fn=fn, args=args, kwargs=kwargs, workspace=workspace))
    ref = await asyncio.to_thread(_submit, placement, payload, cpus)
    try:
        out: bytes = await ref  # type: ignore[misc]  # ObjectRef is awaitable
    except asyncio.CancelledError:
        import ray

        ray.cancel(ref)
        raise
    result: object = cloudpickle.loads(out)
    return result
