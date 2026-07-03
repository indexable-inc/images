"""Cluster surface for ``fleet``: discovery, distributed execution, live peek.

``fleet.scan`` (in ``__init__``) fans a *shell* command over SSH. This module is
the rest of the picture: it treats the tailnet as one cluster you can see and
compute on, three ways, each for the job it actually fits.

- **See it** -- :func:`nodes` merges ``tailscale status --json`` with
  ``ray.nodes()`` into one polars frame: every node, its tailscale address,
  whether it is online, and the Ray resources it advertises. The "what is my
  whole cluster" answer in one call.

- **Compute on it** -- :func:`run` / :func:`submit` / :func:`get` / :func:`put`
  ship a Python *callable* (Ray cloudpickles it by value, so a lambda or a
  cell-defined function travels) to one Ray cluster spanning the tailnet, and
  the Ray object store (Plasma: zero-copy on-node, peer-to-peer transfer
  between nodes, spill-to-disk under pressure) carries the args and results.
  ``run`` is the eager "do this everywhere now and hand me the results"; the
  ``submit``/``get`` pair returns object references you await later, so a long
  job overlaps with the rest of your session. We drive Ray rather than reinvent
  a distributed object store; the bundled interpreter carries it on every node.

- **Peek a live node** -- :func:`in_kernel` runs a line of code in *another
  node's live ix-mcp session* over the tailnet-gated ``/api/exec`` endpoint, so
  you can read that node's actual running state (its ``jobs``, a variable it
  holds, its hostname) rather than a fresh worker's blank namespace. This is the
  one path that sees a node's existing kernel; Ray tasks always get a clean
  worker.

Why three and not one: a Ray task runs in a fresh worker (no access to a node's
interactive state), ``in_kernel`` runs in the live kernel (no object store, text
back), and ``scan`` needs no Python on the far side at all. They do not subsume
each other.

The trust boundary is the tailnet, exactly as the dashboard's and as Ray's own
data plane (Ray has no per-call auth, so any tailnet peer can already drive the
cluster). ``in_kernel`` reaches a peer's ``/api/exec`` over that boundary; if the
cluster additionally sets a shared bearer token (``IX_MCP_EXEC_TOKEN``) for
defense in depth, ``in_kernel`` carries it automatically.
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import json
import os
import shutil
import socket
import subprocess
import sys
from pathlib import Path
from typing import Any
from collections.abc import Callable, Sequence

import polars as pl

__all__ = [
    "EXEC_PORT",
    "SPARK_CONNECT_PORT",
    "ClusterError",
    "connect",
    "get",
    "in_kernel",
    "nodes",
    "put",
    "run",
    "spark",
    "submit",
    "up",
]

# The fixed port each node's ix-mcp publishes its data API / `/api/exec` on, so
# peers can reach each other without discovering a random port. The NixOS fleet
# service pins IX_MCP_DASHBOARD_PORT to this; a dev box can override it.
EXEC_PORT = int(os.environ.get("IX_FLEET_EXEC_PORT", "8799"))

# The Spark Connect gRPC port the `services.ix-spark` master publishes; the
# `fleet.spark` client dials `sc://<master>:<this>`.
SPARK_CONNECT_PORT = int(os.environ.get("IX_FLEET_SPARK_CONNECT_PORT", "15002"))

# The Ray Client server on the fleet head (`ray://<head>:<this>`): the
# supported path for an off-cluster box (e.g. darwin, which cannot join the
# linux cluster as a raylet -- mixed-OS clusters are unsupported by Ray).
RAY_CLIENT_PORT = 10001


def _ray_client_port() -> int:
    """The Ray Client port the tailnet probe checks AND the resolved
    ``ray://`` target dials -- one port for both on purpose, because a
    connectable Client port is the liveness signal for the exact endpoint we
    would use. Probing the GCS (6379) instead was wrong: redis defaults to
    6379, so any redis on a tag:server host read as a Ray head, and two of
    them silently disabled zero-config connect fleet-wide (index#1789 review).
    Read per call, not at import, so tests can point the probe at a fake
    listener via ``IX_FLEET_RAY_CLIENT_PORT``."""
    return int(os.environ.get("IX_FLEET_RAY_CLIENT_PORT", str(RAY_CLIENT_PORT)))


class ClusterError(Exception):
    """A cluster operation could not be carried out (Ray unreachable, a peer
    rejected an exec, a fan-out had failures and ``on_error='raise'``)."""


# --- Ray bootstrap ---------------------------------------------------------


def _tailscale_status_sync(timeout: float = 1.0) -> dict[str, Any] | None:
    """``tailscale status --json`` synchronously, or None when unavailable.

    :func:`connect` is a sync function whose ``ray.init`` already blocks, so
    its pre-probe shells out directly (with a hard timeout) instead of going
    through the async helper below.
    """
    try:
        # _find_tailscale inside the try too: its /usr fallback probe raises
        # PermissionError under the darwin nix sandbox (stat on /usr is
        # denied), and the probe must degrade to "no tailscale", never crash.
        binary = _find_tailscale()
        if not binary:
            return None
        out = subprocess.run(
            [binary, "status", "--json"],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=True,
        ).stdout
        parsed: dict[str, Any] | None = json.loads(out) if out else None
        return parsed
    except Exception:
        return None


def _probe_ray_heads(budget: float = 1.5) -> list[str]:
    """Tailscale IPs of online ``tag:server`` peers with a listening Ray
    Client port.

    The zero-config leg of :func:`connect`'s resolution (index#1787): with no
    explicit address and no env var, sweep the fleet's server-tagged tailnet
    peers for a Ray Client listener, concurrently, spending at most ``budget``
    seconds in total. Only ``tag:server`` peers are probed: that is how the
    fleet marks its service hosts on the tailnet, and probing every phone and
    laptop would waste the budget on guaranteed misses.
    """
    status = _tailscale_status_sync()
    if not status:
        return []
    candidates: list[str] = []
    peers_field = status.get("Peer") or {}
    if isinstance(peers_field, dict):
        for peer in peers_field.values():
            if not isinstance(peer, dict) or not peer.get("Online"):
                continue
            if "tag:server" not in (peer.get("Tags") or []):
                continue
            ip = _ipv4(peer.get("TailscaleIPs"))
            if ip:
                candidates.append(ip)
    if not candidates:
        return []
    port = _ray_client_port()

    def client_port_open(ip: str) -> bool:
        try:
            with socket.create_connection((ip, port), timeout=budget):
                return True
        except OSError:
            return False

    # Threads, not asyncio: connect() runs synchronously (possibly ON the
    # kernel's loop, where asyncio.run is unusable), and the pool is torn down
    # without joining so a straggler socket never stretches past the budget.
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=min(len(candidates), 32))
    try:
        futures = {pool.submit(client_port_open, ip): ip for ip in candidates}
        done, _not_done = concurrent.futures.wait(futures, timeout=budget)
        return sorted(futures[f] for f in done if f.result())
    finally:
        pool.shutdown(wait=False, cancel_futures=True)


def _resolve_auto_target(budget: float = 1.5) -> tuple[str | None, str]:
    """The tailnet-probed Ray Client target, or ``(None, why-not)``.

    Split from :func:`connect` so the zero-config resolution is testable
    against a fake GCS listener without importing (or starting) Ray.

    An AMBIGUOUS probe (several heads, i.e. several clusters) raises instead
    of returning ``None``: the no-target fallback chain ends in a private
    local Ray, and silently computing on one laptop in exactly the case where
    the operator has two clusters to choose from is the worst possible guess
    (index#1789 review). The operator must pick via ``IX_FLEET_RAY_ADDRESS``.
    """
    heads = _probe_ray_heads(budget=budget)
    if len(heads) == 1:
        return f"ray://{heads[0]}:{_ray_client_port()}", ""
    if heads:
        raise RuntimeError(
            f"tailnet probe found {len(heads)} Ray heads ({', '.join(heads)}); "
            "set IX_FLEET_RAY_ADDRESS to pick one"
        )
    return None, "tailnet probe found no tag:server peer with a listening Ray Client port"


def connect(address: str | None = None, *, local: bool = False, **kw: Any) -> None:  # noqa: ANN401 -- forwarded to ray.init
    """Connect this session's kernel to the fleet's Ray cluster (idempotent).

    Target resolution, in order: an explicit ``address``; else
    ``IX_FLEET_RAY_ADDRESS`` (set this to ``ray://<head>:10001`` on an
    off-cluster box -- e.g. a laptop -- so it drives the fleet via the Ray
    Client, the supported thin cross-environment path); else ``RAY_ADDRESS``
    (the fleet's NixOS service sets this to the head GCS, since the daemon's
    non-default temp-dir defeats ``"auto"`` discovery); else a tailnet
    auto-probe (index#1787): online ``tag:server`` peers are TCP-probed on the
    Ray Client port, and exactly one hit becomes ``ray://<ip>:10001`` -- the
    Ray Client path, since an off-cluster box cannot join as a raylet; else ``"auto"``,
    which on a fleet node attaches to the local raylet. With ``local=True``, or
    if the target is unreachable, a private single-node Ray is started (with
    one loud stderr line saying so and why) so the same code still runs on a
    box with no fleet. The one hard failure: a probe that finds SEVERAL Ray
    heads raises (``RuntimeError`` naming every hit) rather than fall back --
    a silent local Ray is the worst answer to "which of my clusters?"; set
    ``IX_FLEET_RAY_ADDRESS`` to pick one. Safe to call repeatedly; the
    Ray-using functions here call it for you.
    """
    import ray

    if ray.is_initialized():
        return
    common = {"logging_level": "error", "configure_logging": False, "ignore_reinit_error": True}
    common.update(kw)
    if local:
        ray.init(**common)
        return
    target = address or os.environ.get("IX_FLEET_RAY_ADDRESS") or os.environ.get("RAY_ADDRESS")
    probe_note = ""
    if not target:
        target, probe_note = _resolve_auto_target()
    if not target:
        target = "auto"
    try:
        ray.init(address=target, **common)
    except Exception as error:
        # No cluster to attach to (a dev box, the head is down, or a `ray://`
        # client could not reach it): fall back to a local Ray so `fleet.run`
        # still works rather than hard-failing -- but LOUDLY (index#1787): a
        # silent single-node Ray looks exactly like the fleet until a job runs
        # on one machine instead of twelve.
        detail = f"; {probe_note}" if probe_note else ""
        print(
            f"[fleet] no reachable Ray cluster (address={target!r}: "
            f"{type(error).__name__}){detail}; started a private single-node Ray",
            file=sys.stderr,
            flush=True,
        )
        ray.init(**common)


# --- Discovery -------------------------------------------------------------


def _find_tailscale() -> str | None:
    """Return the path to the tailscale binary, or None if not found."""
    return shutil.which("tailscale") or next(
        (p for p in ("/usr/local/bin/tailscale", "/usr/bin/tailscale") if Path(p).exists()),
        None,
    )


async def _tailscale_status() -> dict | None:
    """``tailscale status --json`` off the event loop, or None if unavailable."""
    binary = await asyncio.to_thread(_find_tailscale)
    if not binary:
        return None
    try:
        proc = await asyncio.create_subprocess_exec(
            binary, "status", "--json",
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL,
        )
        out, _ = await asyncio.wait_for(proc.communicate(), timeout=4)
        return json.loads(out) if out else None
    except Exception:
        return None


def _ipv4(ips: Sequence[str] | None) -> str | None:
    for ip in ips or []:
        if isinstance(ip, str) and "." in ip and ":" not in ip:
            return ip
    return None


def _ray_nodes() -> list[dict]:
    """``ray.nodes()`` if Ray is reachable, else an empty list (so discovery
    still returns the tailscale view when no cluster is up)."""
    try:
        import ray

        if not ray.is_initialized():
            connect()
        return list(ray.nodes())
    except Exception:
        return []


async def nodes() -> pl.DataFrame:
    """The whole cluster as one polars frame: one row per node.

    Columns: ``host`` (tailscale DNS name or hostname), ``tailscale_ip``,
    ``online`` (tailscale reachability), ``self`` (this node), ``ray_alive``,
    ``cpu`` and ``memory_gb`` (Ray-advertised resources), and ``ray_node_id``.
    Built by joining ``tailscale status`` with ``ray.nodes()`` on the IPv4
    address, so it is informative with only one source up: tailscale-only when
    no Ray cluster runs, Ray-only off the tailnet.

    Example::

        df = await fleet.nodes()
        df.filter(pl.col("ray_alive")).select("host", "cpu", "memory_gb")
    """
    status = await _tailscale_status()
    ray_nodes = await asyncio.to_thread(_ray_nodes)
    ray_by_ip = {n.get("NodeManagerAddress"): n for n in ray_nodes}

    rows: list[dict] = []
    seen_ips: set[str] = set()

    def ray_cols(rn: dict | None) -> dict:
        if not rn:
            return {"ray_alive": None, "cpu": None, "memory_gb": None, "ray_node_id": None}
        res = rn.get("Resources", {}) or {}
        mem = res.get("memory")
        return {
            "ray_alive": bool(rn.get("Alive")),
            "cpu": res.get("CPU"),
            "memory_gb": round(mem / 1e9, 1) if mem else None,
            "ray_node_id": rn.get("NodeID"),
        }

    if status:
        entries = [("Self", status.get("Self") or {})]
        entries += [("Peer", p) for p in (status.get("Peer") or {}).values()]
        for kind, peer in entries:
            ip = _ipv4(peer.get("TailscaleIPs"))
            if ip:
                seen_ips.add(ip)
            rows.append(
                {
                    "host": (peer.get("DNSName") or peer.get("HostName") or ip or "?").rstrip("."),
                    "tailscale_ip": ip,
                    "online": bool(peer.get("Online")),
                    "self": kind == "Self",
                    **ray_cols(ray_by_ip.get(ip)),
                }
            )

    # Ray nodes with no tailscale match (e.g. running off-tailnet, or a node the
    # local tailscale cannot see): still surface them so the Ray view is complete.
    for ip, rn in ray_by_ip.items():
        if ip in seen_ips:
            continue
        rows.append(
            {
                "host": rn.get("NodeManagerHostname") or ip or "?",
                "tailscale_ip": ip,
                "online": None,
                "self": None,
                **ray_cols(rn),
            }
        )

    if not rows:
        return pl.DataFrame(
            schema={
                "host": pl.String, "tailscale_ip": pl.String, "online": pl.Boolean,
                "self": pl.Boolean, "ray_alive": pl.Boolean, "cpu": pl.Float64,
                "memory_gb": pl.Float64, "ray_node_id": pl.String,
            }
        )
    return pl.DataFrame(rows).sort("host")


# --- Distributed execution (Ray) -------------------------------------------


def _alive_ray_nodes() -> list[dict]:
    return [n for n in _ray_nodes() if n.get("Alive")]


def _node_label(rn: dict) -> str:
    return rn.get("NodeManagerHostname") or rn.get("NodeManagerAddress") or rn.get("NodeID", "?")


def _resolve_ray_targets(on: Any) -> list[tuple[str, str | None]]:  # noqa: ANN401 -- on accepts str/"all"/list
    """Resolve ``on`` to a list of ``(label, node_id)``.

    ``node_id`` is ``None`` to mean "let Ray place it anywhere". ``on`` accepts:
    ``"any"`` (one task, unpinned), ``"all"`` (one task pinned to every alive
    node), a single host/label or node-id string, or a list of those.
    """
    if on == "any":
        return [("any", None)]
    alive = _alive_ray_nodes()
    by_key: dict[str, str] = {}
    for rn in alive:
        nid = rn.get("NodeID")
        for key in (rn.get("NodeManagerHostname"), rn.get("NodeManagerAddress"), nid):
            if key:
                by_key[str(key)] = nid
    if on == "all":
        return [(_node_label(rn), rn.get("NodeID")) for rn in alive]
    wanted = [on] if isinstance(on, str) else list(on)
    targets: list[tuple[str, str | None]] = []
    for w in wanted:
        nid = by_key.get(str(w))
        if nid is None:
            raise ClusterError(f"no alive Ray node matches {w!r}; see `await fleet.nodes()`")
        targets.append((str(w), nid))
    return targets


def _remote(fn: Callable, node_id: str | None, options: dict) -> Any:  # noqa: ANN401 -- returns Ray remote fn wrapper
    import ray
    from ray.util.scheduling_strategies import NodeAffinitySchedulingStrategy

    opts = dict(options)
    if node_id is not None:
        # soft=False: a hard pin, so "on every node" really lands one task per
        # node rather than letting Ray collapse them onto the least-busy host.
        opts["scheduling_strategy"] = NodeAffinitySchedulingStrategy(node_id, soft=False)
    remote_fn = fn if hasattr(fn, "remote") else ray.remote(fn)
    return remote_fn.options(**opts) if opts else remote_fn


def submit(fn: Callable, *args: Any, on: Any = "any", **kwargs: Any) -> Any:  # noqa: ANN401 -- cluster fan-out: args/results are caller-defined
    """Schedule ``fn`` on the cluster and return Ray object reference(s) now.

    The reference is a future *and* a handle into the object store; pass it to
    :func:`get` (or straight into another ``submit`` as an argument, so Ray moves
    the data node-to-node without round-tripping through you). Returns a single
    ref when ``on`` is one target, else a ``{label: ref}`` dict. ``ray_options``
    in kwargs (e.g. ``num_cpus``, ``num_gpus``, ``memory``) is forwarded to Ray.
    """
    connect()
    options = kwargs.pop("ray_options", {})
    targets = _resolve_ray_targets(on)
    refs = {label: _remote(fn, nid, options).remote(*args, **kwargs) for label, nid in targets}
    # A single *explicit* target (`on="any"` or one host string) returns the bare
    # ref/result; a fan-out (`on="all"` or a list) always returns the keyed dict,
    # even when the cluster currently has one node -- so the shape is predictable
    # from `on`, not from how many nodes happen to be up.
    if on == "any" or (isinstance(on, str) and on != "all"):
        return next(iter(refs.values()))
    return refs


async def get(refs: Any) -> Any:  # noqa: ANN401 -- Ray object refs: shape mirrors input
    """Await Ray object reference(s) without blocking the kernel's event loop.

    Accepts a single ref, a list, or the ``{label: ref}`` dict :func:`submit`
    returns, and mirrors that shape in the result. Ray ObjectRefs are awaitable,
    so this never ties up a thread.
    """
    if isinstance(refs, dict):
        values = await asyncio.gather(*refs.values(), return_exceptions=True)
        return dict(zip(refs.keys(), values, strict=False))
    if isinstance(refs, (list, tuple)):
        return await asyncio.gather(*refs, return_exceptions=True)
    return await refs


def put(obj: Any) -> Any:  # noqa: ANN401 -- returns Ray ObjectRef, type unknown at call site
    """Place ``obj`` in the cluster object store and return its reference.

    Useful to hand one large input to many tasks: ``put`` it once, pass the ref
    to each ``submit``, and Ray serves it from the store (zero-copy on-node) and
    transfers it peer-to-peer to other nodes on demand, instead of re-shipping
    the bytes per task through you.
    """
    connect()
    import ray

    return ray.put(obj)


async def run(fn: Callable, *args: Any, on: Any = "all", **kwargs: Any) -> Any:  # noqa: ANN401 -- cluster fan-out: args/results are caller-defined
    """Run ``fn`` on the cluster and return the gathered results eagerly.

    The common case: ``await fleet.run(get_hostname, on="all")`` does it
    everywhere and hands back ``{node: result}``. ``on`` is as :func:`submit`
    (``"all"``/``"any"``/a host or list). Returns a ``{label: result}`` dict for
    multiple targets, or the bare result for a single one; a task that raised
    surfaces as the exception object in its slot (it never sinks the batch).

    Example::

        import socket
        await fleet.run(lambda: socket.gethostname(), on="all")
        # {'hc1': 'hc1', 'hc2': 'hc2', ...}
    """
    refs = submit(fn, *args, on=on, **kwargs)
    return await get(refs)


# --- Live-kernel peek (token-gated HTTP) -----------------------------------


def _exec_token() -> str | None:
    token = os.environ.get("IX_MCP_EXEC_TOKEN")
    if token:
        return token.strip()
    path = os.environ.get("IX_MCP_EXEC_TOKEN_FILE")
    if path and Path(path).exists():
        return Path(path).read_text().strip()
    return None


async def _http_targets(on: Any) -> list[tuple[str, str]]:  # noqa: ANN401 -- on accepts str/"all"/list
    """Resolve ``on`` to ``(label, base_url)`` for the ``/api/exec`` endpoint."""
    df = await nodes()
    rows = df.filter(pl.col("tailscale_ip").is_not_null()).to_dicts()
    by_host = {r["host"]: r for r in rows}
    by_ip = {r["tailscale_ip"]: r for r in rows}

    def url(ip: str) -> str:
        return f"http://{ip}:{EXEC_PORT}"

    if on == "all":
        return [(r["host"], url(r["tailscale_ip"])) for r in rows]
    wanted = [on] if isinstance(on, str) else list(on)
    out: list[tuple[str, str]] = []
    for w in wanted:
        r = by_host.get(w) or by_ip.get(w)
        if r is None:
            raise ClusterError(f"no node matches {w!r}; see `await fleet.nodes()`")
        out.append((r["host"], url(r["tailscale_ip"])))
    return out


async def in_kernel(on: Any, code: str, *, budget: float = 15.0) -> pl.DataFrame:  # noqa: ANN401 -- on accepts str/"all"/list
    """Run ``code`` in the *live* ix-mcp kernel of one or more nodes.

    Unlike a Ray task (a fresh worker), this reaches each node's existing
    interactive session, so you can read what that node is actually doing -- its
    ``jobs``, a variable it holds, its hostname. Returns a polars frame with one
    row per node: ``host``, ``ok``, ``output`` (the cell's text/stdout),
    ``result`` (the final expression's repr), and ``error``.

    The boundary is the tailnet (the peer's `/api/exec` trusts it), optionally
    plus a shared token (``IX_MCP_EXEC_TOKEN``) when the cluster sets one. ``on``
    is ``"all"``, a host, or a list of hosts.

    Example::

        await fleet.in_kernel("all", "import socket; socket.gethostname()")
        await fleet.in_kernel("hc1", "len(jobs)")
    """
    import httpx

    # Send the bearer only when we have one; a tailnet-trust cluster (the default
    # fleet config) accepts the call on tailnet membership alone.
    token = _exec_token()
    targets = await _http_targets(on)
    empty = pl.DataFrame(
        schema={"host": pl.String, "ok": pl.Boolean, "output": pl.String,
                "result": pl.String, "error": pl.String}
    )
    # No reachable peers: return the empty frame without standing up an HTTP
    # client (nothing to call, and constructing one is pointless work).
    if not targets:
        return empty
    headers = {"Authorization": f"Bearer {token}"} if token else {}

    async with httpx.AsyncClient(timeout=budget + 30) as client:
        async def one(label: str, base: str) -> dict:
            try:
                resp = await client.post(
                    f"{base}/api/exec", json={"code": code, "budget": budget}, headers=headers
                )
                if resp.status_code != 200:
                    return {"host": label, "ok": False, "output": "", "result": None,
                            "error": f"HTTP {resp.status_code}: {resp.text[:200]}"}
                data = resp.json()
                return {
                    "host": label,
                    "ok": data.get("error") is None,
                    "output": data.get("output", ""),
                    "result": data.get("result"),
                    "error": data.get("error"),
                }
            except Exception as exc:
                return {"host": label, "ok": False, "output": "", "result": None,
                        "error": f"{type(exc).__name__}: {exc}"}

        rows = await asyncio.gather(*(one(lbl, base) for lbl, base in targets))
    return pl.DataFrame(rows) if rows else empty


# --- Spark (big-data SQL / DataFrames, via Spark Connect) ------------------


def _looks_ipv4(host: str) -> bool:
    parts = host.split(".")
    return len(parts) == 4 and all(p.isdigit() and 0 <= int(p) <= 255 for p in parts)


async def _resolve_host(host: str) -> str:
    """Map a host name to its tailscale IPv4 via :func:`nodes`; pass an IP through."""
    if _looks_ipv4(host):
        return host
    df = await nodes()
    by = {r["host"]: r["tailscale_ip"] for r in df.to_dicts() if r.get("tailscale_ip")}
    if host in by:
        return by[host]
    # Exact matches returned above; here only the short hostname (first DNS label)
    # can still match, e.g. "spark-head" against "spark-head.tail.ts.net".
    for full, ip in by.items():
        if full.split(".")[0] == host:
            return ip
    raise ClusterError(f"no node matches {host!r}; see `await fleet.nodes()`")


async def spark(master: str | None = None, **config: Any) -> Any:  # noqa: ANN401 -- returns SparkSession, optional dep
    """Open a SparkSession on the fleet's Spark cluster via Spark Connect.

    The complement to Ray: Ray (:func:`run`) runs distributed *Python*; Spark is
    for big-data *SQL / DataFrames* -- e.g. querying logs collected across the
    fleet. Returns a remote ``SparkSession`` bound to ``sc://<master>:15002``;
    the client is pure gRPC (no local JVM) and heavy work runs on the cluster
    (Gluten/Velox), with results streamed back as Arrow.

    ``master`` is the Spark master's tailscale IP, or a host from
    ``await fleet.nodes()``; omitted, it falls back to ``IX_FLEET_SPARK_MASTER``.
    ``config`` entries become ``SparkSession`` config keys.

    A returned session's calls (``.sql(...).toPandas()``) run synchronously, so
    wrap heavy queries in ``asyncio.to_thread`` to keep the kernel loop free; a
    polars frame is ``pl.from_pandas(df.toPandas())``.

    Example::

        s = await fleet.spark("spark-head")
        rows = await asyncio.to_thread(
            lambda: s.sql("select level, count(*) c from logs group by level").toPandas()
        )
        pl.from_pandas(rows)
    """
    # Lazy: pyspark + its Arrow/gRPC stack is heavy, only paid when Spark is used.
    from pyspark.sql import SparkSession

    host = master or os.environ.get("IX_FLEET_SPARK_MASTER")
    if not host:
        raise ClusterError(
            "no Spark master: pass master='<tailscale-ip-or-host>' or set "
            "IX_FLEET_SPARK_MASTER. See `await fleet.nodes()`."
        )
    url = f"sc://{await _resolve_host(host)}:{SPARK_CONNECT_PORT}"

    def _build() -> Any:  # noqa: ANN401 -- returns SparkSession, optional dep
        builder = SparkSession.builder.remote(url)
        for key, value in config.items():
            builder = builder.config(key, value)
        return builder.getOrCreate()

    # getOrCreate opens the gRPC session (blocking I/O); keep it off the loop.
    return await asyncio.to_thread(_build)


# --- Manual bring-up (dev / ad-hoc; production uses the NixOS service) ------


async def up(*, head: bool = False, address: str | None = None, **kw: Any) -> str:  # noqa: ANN401 -- kw forwarded to ray start
    """Start a Ray daemon on THIS host via ``ray start`` (returns its output).

    For dev or an ad-hoc cluster; on the fleet the NixOS service does this at
    boot. ``head=True`` starts the GCS (the one head); otherwise pass
    ``address="<head-ip>:6379"`` to join an existing cluster. Binds to this
    node's tailscale IPv4 when available so peers reach it over the tailnet.
    """
    ip = _ipv4(((await _tailscale_status()) or {}).get("Self", {}).get("TailscaleIPs"))
    # No `--block`: `ray start` already returns once the daemon is up (block is an
    # opt-in foreground flag, and being a Click flag it rejects a `=value`), so we
    # let it daemonize and capture its startup output.
    cmd = ["ray", "start"]
    if head:
        cmd.append("--head")
        if ip:
            cmd += [f"--node-ip-address={ip}"]
    else:
        if not address:
            raise ClusterError("a worker needs address='<head-ip>:6379' to join")
        cmd += [f"--address={address}"]
        if ip:
            cmd += [f"--node-ip-address={ip}"]
    proc = await asyncio.create_subprocess_exec(
        *cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT
    )
    out, _ = await proc.communicate()
    text = out.decode("utf-8", "replace") if out else ""
    if proc.returncode != 0:
        raise ClusterError(f"`{' '.join(cmd)}` failed:\n{text}")
    return text
