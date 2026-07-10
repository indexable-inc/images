"""Where the kernel process lives: a local child (default) or a Ray actor.

``Kernel`` (kernel.py) drives one ipykernel over jupyter ZMQ and needs a
handful of process-level primitives around it: spawn, liveness, exit code,
signals (SIGUSR1 trace / SIGUSR2 interrupt), the faulthandler trace file,
restart, shutdown. Every one of those is a same-host operation WITH THE
KERNEL, so they live behind :class:`KernelHost` with two implementations:

- :class:`LocalKernelHost` -- today's shape: the kernel as a direct child of
  this serve (``AsyncKernelManager``), signals via ``os.kill``, the trace
  file read off local disk.
- :class:`RayKernelHost` -- the kernel as the child of a small Ray actor
  (:class:`KernelActor`) scheduled on the fleet's Ray cluster, one actor per
  serve. Exec traffic still flows over jupyter ZMQ DIRECTLY from this server
  to the kernel (the connection file binds the node's routable IP, HMAC key
  shipped back through the actor); Ray carries only the control plane --
  spawn/restart/shutdown, liveness, signals, trace reads -- as actor calls.

Selection: ``Config.kernel_host`` (``"local"`` | ``"ray"``), wired from the
``IX_MCP_KERNEL`` env var by the CLI. Ray mode connects through
``fleet.connect()`` -- the fleet module's own resolution chain
(``IX_FLEET_RAY_ADDRESS`` / ``RAY_ADDRESS`` / tailnet probe, loud private
fallback) -- so ONE convention decides which cluster this machine belongs to.

Lifecycle: the actor handle is OWNED by this server process, never detached,
so Ray's ownership GC kills the actor -- and with it the kernel child -- the
moment the serve exits, crashes included. No pidfiles, no orphan reaping.
The actor reserves no CPU (``num_cpus=0``): an interactive kernel is mostly
idle, and N claude sessions must not eat N cores of fleet capacity; heavy
work inside cells fans out as ordinary Ray tasks, which do reserve.

Placement: soft node affinity to THIS node, so the kernel stays next to the
workdir and the node-local channels it inherits (``IX_MCP_STORE``, loopback
``WEAVE_URL``, ``IPYTHONDIR`` under runtime_dir). Cross-node placement only
becomes safe once those channels are network-addressed; the actor already
regenerates ``IPYTHONDIR`` when the inherited path is absent on its node, and
soft (not hard) affinity lets a respawn heal onto another node when this one
cannot host it.
"""

from __future__ import annotations

import asyncio
import contextlib
import os
import sys
from abc import ABC, abstractmethod
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .config import runtime_dir

if TYPE_CHECKING:
    from jupyter_client.asynchronous.client import AsyncKernelClient

# Env var carrying the path the kernel's faulthandler writes all-thread stacks
# to on SIGUSR1. The host sets it before launching the kernel and reads the
# file back for ``Kernel.dump_trace``; the kernel-side runtime registers the
# handler (``runtime``).
TRACE_ENV = "IX_MCP_KERNEL_TRACE"


def trace_path_for(server_pid: int) -> Path:
    """The faulthandler dump target for the serve owning ``server_pid``. One
    file per serve, not one machine-wide name: concurrent kernels sharing a
    path truncate and interleave each other's dumps (index#2355)."""
    return runtime_dir() / f"kernel-trace-{server_pid}.txt"


def _sweep_stale_traces() -> None:
    """Drop trace files orphaned by serves that are gone: a SIGKILLed serve
    never reaches shutdown(), so its file would linger in runtime_dir()
    forever. The legacy fixed-name ``kernel-trace.txt`` (no pid suffix) is
    left alone for still-running older builds."""
    for path in runtime_dir().glob("kernel-trace-*.txt"):
        try:
            pid = int(path.stem.rsplit("-", 1)[-1])
        except ValueError:
            continue
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            path.unlink(missing_ok=True)
        except PermissionError:
            continue  # a live process we cannot signal: not ours to sweep


def _child_pid(km: Any) -> int | None:  # noqa: ANN401 -- jupyter_client's manager has no useful static type here
    """The kernel process's pid, so a trace signal targets that process alone
    (not the kernel's process group, whose default SIGUSR1 would terminate
    user-launched subprocesses)."""
    provisioner = getattr(km, "provisioner", None)
    pid = getattr(provisioner, "pid", None)
    if pid is None:
        pid = getattr(getattr(km, "kernel", None), "pid", None)
    return pid


class KernelHost(ABC):
    """Process-level supervision of one ipykernel, wherever it runs."""

    @property
    @abstractmethod
    def pid(self) -> int | None:
        """The kernel process's pid (on the node it runs on)."""

    @property
    @abstractmethod
    def running(self) -> bool:
        """Started and not shut down (says nothing about liveness)."""

    @abstractmethod
    async def start(self, workdir: Path) -> None:
        """Launch the kernel; ready to hand out connected clients after."""

    @abstractmethod
    def client(self) -> AsyncKernelClient:
        """A fresh client wired to the kernel; caller starts the channels."""

    @abstractmethod
    async def is_alive(self) -> bool:
        """Whether the kernel process (and its supervisor) is alive."""

    @abstractmethod
    async def exit_code(self) -> int | None:
        """The dead kernel's returncode (negative: killed by that signal)."""

    @abstractmethod
    async def send_signal(self, signum: int) -> bool:
        """Deliver ``signum`` to the kernel process; False when it is gone."""

    @abstractmethod
    async def trace_size(self) -> int:
        """Current size of the faulthandler trace file (0 when absent)."""

    @abstractmethod
    async def trace_read(self, offset: int) -> str:
        """Trace file text past ``offset``."""

    @abstractmethod
    async def restart(self, workdir: Path) -> None:
        """A fresh kernel process; ``client()`` reflects it afterwards."""

    @abstractmethod
    async def shutdown(self) -> None:
        """Kill the kernel and release everything the host owns."""


class LocalKernelHost(KernelHost):
    """The kernel as a direct child of this serve (the default)."""

    def __init__(self) -> None:
        self._km: Any = None
        self._pid: int | None = None
        self._trace_path: Path | None = None

    @property
    def pid(self) -> int | None:
        return self._pid

    @property
    def running(self) -> bool:
        return self._km is not None

    async def start(self, workdir: Path) -> None:
        from jupyter_client.manager import AsyncKernelManager

        # Point the kernel's faulthandler at a private file before launch; the
        # kernel inherits this env and registers the SIGUSR1 dump handler. The
        # name carries this server's pid: every serve on the machine shares
        # runtime_dir(), and one fixed name had concurrent kernels truncating
        # and interleaving each other's dumps, so kernel_trace could return a
        # different session's stacks (index#2355).
        self._trace_path = trace_path_for(os.getpid())
        os.environ[TRACE_ENV] = str(self._trace_path)
        _sweep_stale_traces()

        self._km = AsyncKernelManager(kernel_name="python3")
        await self._km.start_kernel(cwd=str(workdir))
        self._pid = _child_pid(self._km)

    def client(self) -> AsyncKernelClient:
        return self._km.client()

    async def is_alive(self) -> bool:
        return self._km is not None and bool(await self._km.is_alive())

    async def exit_code(self) -> int | None:
        provisioner = getattr(self._km, "provisioner", None)
        process = getattr(provisioner, "process", None)
        code = getattr(process, "returncode", None)
        return code if isinstance(code, int) else None

    async def send_signal(self, signum: int) -> bool:
        if self._pid is None:
            return False
        try:
            os.kill(self._pid, signum)
        except ProcessLookupError:
            return False
        return True

    async def trace_size(self) -> int:
        path = self._trace_path
        return path.stat().st_size if path is not None and path.exists() else 0

    async def trace_read(self, offset: int) -> str:
        if self._trace_path is None:
            return ""
        return self._trace_path.read_text()[offset:]

    async def restart(self, workdir: Path) -> None:
        await self._km.restart_kernel(now=True, cwd=str(workdir))
        self._pid = _child_pid(self._km)

    async def shutdown(self) -> None:
        if self._km is not None:
            await self._km.shutdown_kernel(now=True)
            self._km = None
        # This serve owns its trace file (the name carries our pid): remove it
        # so clean exits leave nothing behind; SIGKILLed serves are covered by
        # the sweep at the next start().
        if self._trace_path is not None:
            self._trace_path.unlink(missing_ok=True)
            self._trace_path = None


class KernelActor:
    """Runs on a fleet node; supervises one ipykernel child there.

    Decorated with ``ray.remote`` at spawn time (``RayKernelHost``), never at
    import, so importing this module costs no Ray. Async methods make it an
    async actor; each does node-local work the server cannot do remotely:
    signals, trace-file reads, spawn/restart against the local jupyter
    provisioner. The connection info it returns carries the node's routable
    IP, so the server's ZMQ channels reach the kernel directly.
    """

    def __init__(self) -> None:
        self._km: Any = None
        self._pid: int | None = None
        self._trace_path: Path | None = None

    async def launch(self, workdir: str, env: dict[str, str]) -> dict:
        """Start the kernel on this node; return pid + ZMQ connection info.

        ``env`` is the serve's environment: a local kernel child would inherit
        exactly that, so the actor replays it for parity (IX_MCP_STORE, the
        weave/session vars, SSH_AUTH_SOCK). Two entries are node-local and
        re-derived here: the trace path (this actor's pid, this node's
        runtime_dir) and IPYTHONDIR when the inherited path does not exist on
        this node (the serve materialized it under ITS runtime_dir).
        """
        import ray

        os.environ.update(env)
        self._trace_path = trace_path_for(os.getpid())
        os.environ[TRACE_ENV] = str(self._trace_path)
        _sweep_stale_traces()
        ipythondir = env.get("IPYTHONDIR")
        if not ipythondir or not await asyncio.to_thread(Path(ipythondir).exists):
            from .cli import _prepare_ipython_startup

            os.environ["IPYTHONDIR"] = str(_prepare_ipython_startup(os.getpid()))

        from jupyter_client.manager import AsyncKernelManager

        await asyncio.to_thread(lambda: Path(workdir).mkdir(parents=True, exist_ok=True))
        self._km = AsyncKernelManager(kernel_name="python3")
        # Bind the kernel's ZMQ sockets on this node's routable IP (the one
        # Ray knows peers reach it by), not loopback: the serve driving this
        # kernel may sit on another node.
        self._km.ip = ray.util.get_node_ip_address()
        await self._km.start_kernel(cwd=workdir)
        self._pid = _child_pid(self._km)
        return self._info()

    def _info(self) -> dict:
        info = dict(self._km.get_connection_info(session=False))
        key = info.get("key", b"")
        # Plain-str payload: bytes round-trip through Ray fine, but a str key
        # keeps the dict JSON-safe for logs/facts; load_connection_info
        # re-encodes it.
        info["key"] = key.decode("ascii") if isinstance(key, bytes) else key
        return {"pid": self._pid, "connection": info, "node_ip": self._km.ip}

    async def relaunch(self, workdir: str) -> dict:
        """A fresh kernel child from the same actor (the respawn primitive)."""
        await self._km.restart_kernel(now=True, cwd=workdir)
        self._pid = _child_pid(self._km)
        return self._info()

    async def is_alive(self) -> bool:
        return self._km is not None and bool(await self._km.is_alive())

    async def exit_code(self) -> int | None:
        provisioner = getattr(self._km, "provisioner", None)
        process = getattr(provisioner, "process", None)
        code = getattr(process, "returncode", None)
        return code if isinstance(code, int) else None

    async def send_signal(self, signum: int) -> bool:
        if self._pid is None:
            return False
        try:
            os.kill(self._pid, signum)
        except ProcessLookupError:
            return False
        return True

    async def trace_size(self) -> int:
        path = self._trace_path
        return path.stat().st_size if path is not None and path.exists() else 0

    async def trace_read(self, offset: int) -> str:
        if self._trace_path is None:
            return ""
        return self._trace_path.read_text()[offset:]

    async def shutdown(self) -> None:
        if self._km is not None:
            await self._km.shutdown_kernel(now=True)
            self._km = None
        if self._trace_path is not None:
            self._trace_path.unlink(missing_ok=True)
            self._trace_path = None


class RayKernelHost(KernelHost):
    """The kernel behind a :class:`KernelActor` on the fleet's Ray cluster."""

    def __init__(self) -> None:
        self._actor: Any = None
        self._connection: dict | None = None
        self._pid: int | None = None
        self._node_ip: str | None = None

    @property
    def pid(self) -> int | None:
        return self._pid

    @property
    def running(self) -> bool:
        return self._actor is not None

    async def start(self, workdir: Path) -> None:
        # fleet.connect() blocks (probe + ray.init): keep it off the loop. Ray
        # state is process-global, so initializing from a worker thread is fine.
        actor = await asyncio.to_thread(self._spawn_actor)
        try:
            info = await actor.launch.remote(str(workdir), dict(os.environ))
        except BaseException:
            # A launch that failed (worker env missing a dep, workdir refused)
            # must not leak a live actor.
            import ray

            with contextlib.suppress(Exception):
                ray.kill(actor)
            raise
        self._actor = actor
        self._apply(info)
        print(
            f"[ix-mcp] kernel hosted on ray (node {self._node_ip}, pid {self._pid})",
            file=sys.stderr,
            flush=True,
        )

    def _spawn_actor(self) -> Any:  # noqa: ANN401 -- a ray actor handle has no importable static type
        import ray
        from fleet.cluster import connect
        from ray.util.scheduling_strategies import NodeAffinitySchedulingStrategy

        connect()
        remote_cls = ray.remote(num_cpus=0, max_restarts=0)(KernelActor)
        return remote_cls.options(
            scheduling_strategy=NodeAffinitySchedulingStrategy(
                node_id=ray.get_runtime_context().get_node_id(), soft=True
            ),
        ).remote()

    def _apply(self, info: dict) -> None:
        self._connection = dict(info["connection"])
        self._pid = info["pid"]
        self._node_ip = info.get("node_ip")

    def client(self) -> AsyncKernelClient:
        from jupyter_client.asynchronous.client import AsyncKernelClient

        kc = AsyncKernelClient()
        kc.load_connection_info(self._connection or {})
        return kc

    async def is_alive(self) -> bool:
        if self._actor is None:
            return False
        try:
            return bool(await self._actor.is_alive.remote())
        except Exception:
            # The actor itself is gone (killed, node lost, cluster down): the
            # kernel died with it. restart() below respawns actor and kernel.
            return False

    async def exit_code(self) -> int | None:
        if self._actor is None:
            return None
        try:
            return await self._actor.exit_code.remote()
        except Exception:
            return None

    async def send_signal(self, signum: int) -> bool:
        if self._actor is None:
            return False
        try:
            return bool(await self._actor.send_signal.remote(signum))
        except Exception:
            return False

    async def trace_size(self) -> int:
        if self._actor is None:
            return 0
        try:
            return int(await self._actor.trace_size.remote())
        except Exception:
            return 0

    async def trace_read(self, offset: int) -> str:
        if self._actor is None:
            return ""
        try:
            return str(await self._actor.trace_read.remote(offset))
        except Exception:
            return ""

    async def restart(self, workdir: Path) -> None:
        if self._actor is not None:
            try:
                self._apply(await self._actor.relaunch.remote(str(workdir)))
                return
            except Exception as exc:
                # The actor died with its node (or the kernel manager inside it
                # is unusable): respawn both. Soft affinity lets the fresh actor
                # land wherever the cluster can host it.
                print(
                    f"[ix-mcp] kernel actor lost ({type(exc).__name__}); respawning actor",
                    file=sys.stderr,
                    flush=True,
                )
                await self.shutdown()
        await self.start(workdir)

    async def shutdown(self) -> None:
        actor, self._actor = self._actor, None
        self._connection = None
        if actor is None:
            return
        with contextlib.suppress(Exception):
            await actor.shutdown.remote()
        import ray

        with contextlib.suppress(Exception):
            ray.kill(actor)


def make_kernel_host(kind: str) -> KernelHost:
    """The host ``Config.kernel_host`` names; unknown values fail loudly."""
    if kind == "local":
        return LocalKernelHost()
    if kind == "ray":
        return RayKernelHost()
    raise ValueError(f"unknown kernel host {kind!r} (expected 'local' or 'ray')")
